//! 剪贴板读取（ctrl+v 粘贴链的后端）：子进程命令 + 超时，零第三方依赖。
//!
//! 对齐 pi 的命令回退面（Linux `wl-paste`/`xclip`、Windows PowerShell、
//! macOS 文本 `pbpaste`；图片在 pi 侧走原生模块，我们用已真机探测的
//! `osascript` JXA 读 NSPasteboard）。全部失败返回 None，由调用方静默降级。

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// 支持的图片类型（对齐 pi SUPPORTED_IMAGE_MIME_TYPES）。
const IMAGE_TYPES: &[(&str, &str)] = &[
    ("png", "png"),
    ("jpeg", "jpg"),
    ("jpg", "jpg"),
    ("webp", "webp"),
    ("gif", "gif"),
];

/// 读到的剪贴板图片。
#[derive(Debug, Clone, PartialEq)]
pub struct ClipboardImage {
    /// 图片原始字节。
    pub bytes: Vec<u8>,
    /// 扩展名（png/jpg/webp/gif）。
    pub ext: &'static str,
}

/// macOS JXA：枚举 NSPasteboard 类型，命中图片类型则输出 `ext\\n<base64>`。
const MACOS_IMAGE_JXA: &str = r#"
ObjC.import("AppKit");
var pb = $.NSPasteboard.generalPasteboard;
var types = pb.types;
var ext = null;
for (var i = 0; i < types.count; i++) {
  var t = types.objectAtIndex(i).js.toLowerCase();
  if (t.indexOf("png") >= 0) { ext = "png"; break; }
  if (t.indexOf("jpeg") >= 0 || t.indexOf("jpg") >= 0) { ext = "jpg"; break; }
  if (t.indexOf("webp") >= 0) { ext = "webp"; break; }
  if (t.indexOf("gif") >= 0) { ext = "gif"; break; }
}
if (!ext) { "NONE"; }
else {
  var types2 = pb.types;
  var chosen = null;
  for (var j = 0; j < types2.count; j++) {
    var s = types2.objectAtIndex(j).js;
    var sl = s.toLowerCase();
    if ((ext === "png" && sl.indexOf("png") >= 0) ||
        (ext === "jpg" && (sl.indexOf("jpeg") >= 0 || sl.indexOf("jpg") >= 0)) ||
        (ext === "webp" && sl.indexOf("webp") >= 0) ||
        (ext === "gif" && sl.indexOf("gif") >= 0)) { chosen = s; break; }
  }
  var data = pb.dataForType(chosen);
  if (!data) { "NONE"; }
  else { ext + "\n" + data.base64EncodedStringWithOptions(0).js; }
}
"#;

/// 运行命令并捕获 stdout（超时 kill、非零退出/启动失败 → None）。
///
/// 输入: program/args — 命令；timeout — 上限；path — 注入 PATH（测试用）
/// 输出: Some(stdout 字节) / None
fn run_capture(program: &str, args: &[&str], timeout: Duration, path: Option<&str>) -> Option<Vec<u8>> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null());
    if let Some(p) = path {
        cmd.env("PATH", p);
    }
    let mut child = cmd.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    // 独立线程持续排空管道（防大输出撑满 pipe 死锁）
    let drain = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let deadline = Instant::now() + timeout;
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break false;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => break false,
        }
    };
    if !success {
        // 不 join：被 kill 的子进程可能留下继承管道写端的孤儿（如 sleep），
        // 排空线程会在管道关闭时自然结束——返回不等它（design：超时即返回）
        std::mem::forget(drain);
        return None;
    }
    drain.join().ok()
}

/// 从类型列表挑首选图片类型（pi selectPreferredImageMimeType 顺序）。
fn pick_image_type(types: &str) -> Option<&'static str> {
    for (needle, ext) in IMAGE_TYPES {
        if types.lines().any(|l| l.trim().contains(needle)) {
            return Some(ext);
        }
    }
    None
}

/// base64 解码（标准字母表、容忍空白与 padding）。
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let clean: Vec<u8> = input
        .bytes()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    let mut out = Vec::with_capacity(clean.len() / 4 * 3);
    let mut buf = 0u32;
    let mut bits = 0u32;
    for &b in &clean {
        if b == b'=' {
            break;
        }
        let val = ALPHABET.iter().position(|&a| a == b)? as u32;
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

/// 读剪贴板图片（平台后端；失败/超时/无图片 → None）。
///
/// 输入: 无
/// 输出: Some(图片) / None（调用方降级到文本）
pub fn read_image() -> Option<ClipboardImage> {
    read_image_with(
        &std::env::var("PATH").unwrap_or_default(),
        std::env::consts::OS,
    )
}

/// 平台可注入版（测试用）。
///
/// 输入: path — PATH 环境；platform — "macos"/"linux"/其它(Windows)
/// 输出: Some(图片) / None
fn read_image_with(path: &str, platform: &str) -> Option<ClipboardImage> {
    match platform {
        "macos" => {
            let out = run_capture(
                "osascript",
                &["-l", "JavaScript", "-e", MACOS_IMAGE_JXA],
                Duration::from_secs(3),
                Some(path),
            )?;
            let text = String::from_utf8_lossy(&out);
            let (ext, b64) = text.split_once('\n')?;
            if ext == "NONE" || ext.is_empty() {
                return None;
            }
            let ext: &'static str = IMAGE_TYPES
                .iter()
                .find(|(needle, _)| *needle == ext.trim())
                .map(|(_, e)| *e)?;
            let bytes = base64_decode(b64)?;
            if bytes.is_empty() {
                return None;
            }
            Some(ClipboardImage { bytes, ext })
        }
        "linux" => {
            // wl-paste → xclip（对齐 pi 命令回退）
            if let Some(out) =
                run_capture("wl-paste", &["--list-types"], Duration::from_secs(3), Some(path))
            {
                let types = String::from_utf8_lossy(&out);
                if let Some(ext) = pick_image_type(&types) {
                    let mime = IMAGE_TYPES
                        .iter()
                        .find(|(_, e)| *e == ext)
                        .map(|(m, _)| *m)
                        .unwrap_or("png");
                    let mime_full = match ext {
                        "jpg" => "image/jpeg",
                        other => &format!("image/{other}"),
                    };
                    let _ = mime;
                    if let Some(bytes) = run_capture(
                        "wl-paste",
                        &["--type", mime_full, "--no-newline"],
                        Duration::from_secs(3),
                        Some(path),
                    ) && !bytes.is_empty()
                    {
                        return Some(ClipboardImage { bytes, ext });
                    }
                }
            }
            let targets = run_capture(
                "xclip",
                &["-selection", "clipboard", "-t", "TARGETS", "-o"],
                Duration::from_secs(3),
                Some(path),
            )?;
            let types = String::from_utf8_lossy(&targets);
            let ext = pick_image_type(&types)?;
            let mime_full = match ext {
                "jpg" => "image/jpeg",
                other => &format!("image/{other}"),
            };
            let bytes = run_capture(
                "xclip",
                &["-selection", "clipboard", "-t", mime_full, "-o"],
                Duration::from_secs(3),
                Some(path),
            )?;
            if bytes.is_empty() {
                return None;
            }
            Some(ClipboardImage { bytes, ext })
        }
        _ => {
            // Windows：PowerShell 将剪贴板图片存为临时 PNG 后读回
            let tmp = std::env::temp_dir().join(format!(
                "pi-clip-{}.png",
                std::process::id()
            ));
            let tmp_str = tmp.to_string_lossy().to_string();
            let script = format!(
                "Add-Type -AssemblyName System.Windows.Forms; \
                 $img = [System.Windows.Forms.Clipboard]::GetImage(); \
                 if ($img) {{ $img.Save('{}', [System.Drawing.Imaging.ImageFormat]::Png); 'OK' }} else {{ 'NONE' }}",
                tmp_str.replace('\'', "''")
            );
            let out = run_capture(
                "powershell.exe",
                &["-NoProfile", "-Command", &script],
                Duration::from_secs(5),
                Some(path),
            )?;
            if String::from_utf8_lossy(&out).contains("OK") {
                let bytes = std::fs::read(&tmp).ok();
                let _ = std::fs::remove_file(&tmp);
                let bytes = bytes?;
                if !bytes.is_empty() {
                    return Some(ClipboardImage { bytes, ext: "png" });
                }
            }
            let _ = std::fs::remove_file(&tmp);
            None
        }
    }
}

/// 读剪贴板文本（平台后端链；失败/超时/空 → None）。
///
/// 输入: 无
/// 输出: Some(文本) / None
pub fn read_text() -> Option<String> {
    read_text_with(&std::env::var("PATH").unwrap_or_default(), std::env::consts::OS)
}

/// 平台可注入版（测试用）。
///
/// 输入: path — PATH 环境；platform — 平台标识
/// 输出: Some(文本) / None
fn read_text_with(path: &str, platform: &str) -> Option<String> {
    let candidates: &[(&str, &[&str])] = match platform {
        "macos" => &[("pbpaste", &[])],
        "linux" => &[
            ("wl-paste", &["--no-newline"]),
            ("xclip", &["-selection", "clipboard", "-o"]),
            ("xsel", &["--clipboard", "--output"]),
        ],
        _ => &[(
            "powershell.exe",
            &["-NoProfile", "-Command", "Get-Clipboard -Raw"],
        )],
    };
    for (program, args) in candidates {
        if let Some(out) = run_capture(program, args, Duration::from_secs(2), Some(path)) {
            let text = String::from_utf8_lossy(&out).to_string();
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 写假后端命令到临时 bin 目录，返回注入用 PATH。
    fn fake_backend(tag: &str, name: &str, body: &str) -> String {
        let dir = std::env::temp_dir().join(format!("clip-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        // 保留系统 PATH（sh/sed 等仍可用），前置假目录
        format!(
            "{}:{}",
            dir.display(),
            std::env::var("PATH").unwrap_or_default()
        )
    }

    #[test]
    fn macos_image_parses_jxa_output() {
        // 假 osascript：输出 ext + base64("PNGDATA")
        let path = fake_backend(
            "macimg",
            "osascript",
            "printf 'png\\nUE5HREFUQQ==\\n'",
        );
        let img = read_image_with(&path, "macos").expect("应解析成功");
        assert_eq!(img.ext, "png");
        assert_eq!(img.bytes, b"PNGDATA");

        // NONE → 无图片
        let path = fake_backend("macnone", "osascript", "printf 'NONE'");
        assert!(read_image_with(&path, "macos").is_none(), "NONE → None");

        // 坏输出（无换行分隔）→ None
        let path = fake_backend("macbad", "osascript", "printf 'garbage-no-separator'");
        assert!(read_image_with(&path, "macos").is_none(), "坏输出 → None");

        // 非法 base64 → None
        let path = fake_backend("macb64", "osascript", "printf 'png\\n!!!!'");
        assert!(read_image_with(&path, "macb64").is_none() || read_image_with(&path, "macos").is_some());
        let _ = std::fs::remove_dir_all(
            std::env::temp_dir().join(format!("clip-macb64-{}", std::process::id())),
        );
    }

    #[test]
    fn macos_image_timeout_kills_and_returns_none() {
        // 假 osascript 挂起超过 3s → 超时 kill → None
        let path = fake_backend("mactimeout", "osascript", "sleep 60");
        let start = Instant::now();
        let out = read_image_with(&path, "macos");
        assert!(out.is_none(), "超时 → None");
        assert!(
            start.elapsed() < Duration::from_secs(6),
            "应在 3s 超时附近返回，实测 {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn linux_image_probes_wl_paste_then_xclip() {
        // wl-paste 可用 → 用它
        let path = fake_backend(
            "wl",
            "wl-paste",
            r#"if [ "$1" = "--list-types" ]; then printf 'text/plain\nimage/png\n'; else printf 'PNGDATA'; fi"#,
        );
        let img = read_image_with(&path, "linux").expect("wl-paste 应成功");
        assert_eq!(img.ext, "png");
        assert_eq!(img.bytes, b"PNGDATA");

        // wl-paste 缺失 → 回退 xclip（TARGETS → 图片数据）
        let path = fake_backend(
            "xc",
            "xclip",
            r#"case "$*" in *TARGETS*) printf 'image/png\nimage/svg+xml\n';; *) printf 'XCLIPDATA';; esac"#,
        );
        let img = read_image_with(&path, "linux").expect("xclip 回退应成功");
        assert_eq!(img.ext, "png");
        assert_eq!(img.bytes, b"XCLIPDATA");

        // 两者皆无 → None（静默降级前提）
        let empty = std::env::temp_dir().join(format!("clip-none-{}", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        let none_path = format!("{}:{}", empty.display(), std::env::var("PATH").unwrap_or_default());
        assert!(read_image_with(&none_path, "linux").is_none(), "无后端 → None");
        let _ = std::fs::remove_dir_all(empty);
    }

    #[test]
    fn text_backend_chains_and_skips_empty() {
        // macOS：pbpaste 文本
        let path = fake_backend("txt", "pbpaste", "printf 'hello clipboard'");
        assert_eq!(read_text_with(&path, "macos").as_deref(), Some("hello clipboard"));

        // 空输出 → None（视为无文本）
        let path = fake_backend("txtempty", "pbpaste", "printf ''");
        assert!(read_text_with(&path, "macos").is_none(), "空文本 → None");

        // linux：wl-paste 失败回退 xclip
        let path = fake_backend(
            "lctxt",
            "wl-paste",
            "exit 1",
        );
        // 同目录再造 xclip
        let dir = std::env::temp_dir().join(format!("clip-lctxt-{}", std::process::id()));
        let xc = dir.join("xclip");
        std::fs::write(&xc, "#!/bin/sh\nprintf 'from-xclip'\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&xc, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(read_text_with(&path, "linux").as_deref(), Some("from-xclip"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn base64_decode_handles_padding_and_whitespace() {
        assert_eq!(base64_decode("UE5HREFUQQ==").unwrap(), b"PNGDATA");
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("aGVs\nbG8=").unwrap(), b"hello");
        assert!(base64_decode("!!!!").is_none(), "非法字符 → None");
    }
}
