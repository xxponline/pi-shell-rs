//! Child-process transport for `pi --mode rpc` (or `PI_BIN` override).
//!
//! Three OS threads:
//!   - writer thread:  sends JSONL lines to child stdin
//!   - stdout reader:  parses each line into [`WireMessage`]
//!   - stderr reader:  forwards raw timestamped lines on a separate channel
//!
//! Generic over the spawned program so tests can point it at a fake "pi"
//! shell script that replays fixture lines.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

/// Windows 上 `Command::creation_flags` 所需的 trait；该 trait 在非 Windows
/// 平台不存在，故整个 use 用 `cfg(windows)` 罩住。
#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::rpc::jsonl::{OutgoingCommand, WireMessage};

/// Something observed on the child's stdout.
#[derive(Debug, Clone)]
pub enum TransportEvent {
    /// A decoded stdout line (JSON or an unparseable verbatim line).
    Message(WireMessage),
    /// The stdout reader thread ended (child closed stdout / exited).
    StdoutClosed,
}

/// A raw stderr line with a timestamp relative to transport start.
#[derive(Debug, Clone)]
pub struct StderrLine {
    pub at: Duration,
    pub text: String,
}

/// Handle used to send commands and receive events.
pub struct Transport {
    pub events: Receiver<TransportEvent>,
    pub stderr: Receiver<StderrLine>,
    write_tx: Sender<String>,
    child: Child,
}

/// 解析要启动的 pi 可执行文件（所有 `pi --mode rpc` 启动路径的入口）。
///
/// 功能: 按优先级给出可直接 `Command::new` 的 pi 路径。
/// 输入: 无（读进程环境变量 `PI_BIN` 与 `PATH`）。
/// 输出: `PI_BIN` 显式非空值 → 原样返回；否则走 [`resolve_binary_in`]，
///       最终回退 `"pi"`（保留原有的缺装报错路径，不吞掉错误信息）。
pub fn resolve_binary() -> String {
    resolve_binary_in(std::env::var("PI_BIN").ok(), std::env::var_os("PATH"))
}

/// [`resolve_binary`] 的纯函数核心（注入环境，便于无副作用单测）。
///
/// 功能: 同 [`resolve_binary`] 的解析顺序。
/// 输入: pi_bin — `PI_BIN` 的取值（`None` = 未设置；空串视为未设置，
///       避免 spawn `""`）；path — `PATH` 的取值（仅 Windows 参与解析）。
/// 输出: 可执行文件路径字符串；Windows 上依次尝试
///       `pi.exe` → `pi.cmd` → `pi.bat`（cmd 的 PATHEXT 语义，目录优先），
///       找不到回退 `"pi"`。非 Windows 恒为 `PI_BIN` 或 `"pi"`。
fn resolve_binary_in(pi_bin: Option<String>, path: Option<std::ffi::OsString>) -> String {
    if let Some(bin) = pi_bin
        && !bin.is_empty()
    {
        return bin;
    }
    #[cfg(windows)]
    if let Some(found) = find_pi_on_path(path.as_deref()) {
        return found;
    }
    #[cfg(not(windows))]
    let _ = &path;
    "pi".to_string()
}

/// Windows 上按 PATHEXT 语义在 `PATH` 逐目录找 pi 可执行文件。
///
/// 为什么需要: `Command::new("pi")` 走 CreateProcess 只尝试 `pi.exe`，
/// 既不读 PATHEXT，也不执行安装器放在 `~\.pi\agent\bin\` 下的无扩展名
/// sh 脚本 `pi` — 启动报 `failed to spawn \`pi --mode rpc\`: program not found`
/// （`cargo run` 实测）。而 `pi.cmd` 可被 CreateProcess 直接拉起
/// （内部转 cmd.exe，实测 RPC 握手通过）。
///
/// 输入: path — `PATH` 原始值（`None` 直接返回 `None`）。
/// 输出: 首个存在的 `pi.exe` / `pi.cmd` / `pi.bat` 的拼接路径；
///       找不到返回 `None`（调用方回退 `"pi"` 保留原报错路径）。
#[cfg(windows)]
fn find_pi_on_path(path: Option<&std::ffi::OsStr>) -> Option<String> {
    let path = path?;
    for dir in std::env::split_paths(path) {
        for ext in ["exe", "cmd", "bat"] {
            let cand = dir.join(format!("pi.{ext}"));
            if cand.is_file() {
                return Some(cand.to_string_lossy().into_owned());
            }
        }
    }
    None
}

impl Transport {
    /// 以管道 stdio 启动 `<bin> --mode rpc`（子进程继承本进程工作目录）。
    ///
    /// 输入: binary — 可执行文件路径；extra_args — 追加到 `--mode rpc` 之后的参数。
    /// 输出: 就绪的 [`Transport`]；spawn 失败返回带上下文的 `io::Error`。
    pub fn spawn_rpc(binary: &str, extra_args: &[String]) -> std::io::Result<Self> {
        Self::spawn_rpc_in(binary, extra_args, None)
    }

    /// 同 [`spawn_rpc`]，但可为子进程指定工作目录（embed 模式：会话 workspace 路径）。
    ///
    /// 输入: binary / extra_args 同上；cwd — 子进程工作目录（`None` 继承本进程）。
    /// 输出: 就绪的 [`Transport`]；spawn 失败返回带上下文的 `io::Error`。
    pub fn spawn_rpc_in(
        binary: &str,
        extra_args: &[String],
        cwd: Option<&std::path::Path>,
    ) -> std::io::Result<Self> {
        let mut cmd = Command::new(binary);
        cmd.arg("--mode")
            .arg("rpc")
            .args(extra_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = cwd {
            cmd.current_dir(cwd);
        }
        // GUI 宿主 (windows_subsystem="windows") 无控制台: 不加此标志时 Windows 会为
        // pi 子进程新建可见控制台窗口 (实测 WT 标题变为 "pi"); debug 宿主有控制台,
        // 子进程继承故不触发。stdio 已全部 piped, 不需要控制台; 实测加标志后
        // RPC get_state 探活照常通过。
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW

        let mut child = cmd.spawn().map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!("failed to spawn `{binary} --mode rpc`: {e}"),
            )
        })?;

        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");

        let (event_tx, event_rx) = mpsc::channel::<TransportEvent>();
        let (err_tx, err_rx) = mpsc::channel::<StderrLine>();
        let (write_tx, write_rx) = mpsc::channel::<String>();

        let started = Instant::now();

        // Writer thread.
        thread::spawn(move || {
            let mut stdin = stdin;
            while let Ok(line) = write_rx.recv() {
                if stdin.write_all(line.as_bytes()).is_err()
                    || stdin.write_all(b"\n").is_err()
                    || stdin.flush().is_err()
                {
                    break;
                }
            }
        });

        // Stdout reader thread.
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(text) => {
                        let msg = WireMessage::parse_line(&text);
                        if event_tx.send(TransportEvent::Message(msg)).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = event_tx.send(TransportEvent::StdoutClosed);
        });

        // Stderr reader thread (separate channel; never mixed with stdout).
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                if err_tx
                    .send(StderrLine {
                        at: started.elapsed(),
                        text: line,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });

        Ok(Transport {
            events: event_rx,
            stderr: err_rx,
            write_tx,
            child,
        })
    }

    /// Send a command envelope (id inserted by [`OutgoingCommand`]).
    pub fn send(&self, cmd: &OutgoingCommand) -> std::io::Result<()> {
        self.send_raw(&cmd.to_line())
    }

    /// Send one pre-serialized JSON object line (no id injection).
    pub fn send_raw(&self, line: &str) -> std::io::Result<()> {
        self.write_tx
            .send(line.to_string())
            .map_err(|_| std::io::Error::other("writer thread gone"))
    }

    /// Kill the child and best-effort reap it.
    pub fn shutdown(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drain events until timeout/closed, collecting every stdout line.
    fn drain(transport: &Transport) -> Vec<WireMessage> {
        let mut out = Vec::new();
        while let Ok(TransportEvent::Message(m)) =
            transport.events.recv_timeout(Duration::from_secs(5))
        {
            out.push(m);
        }
        out
    }

    fn write_fake(script: &str, name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("probe-lib-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fake-pi.sh");
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    #[test]
    fn fake_pi_replays_lines_over_transport() {
        let script = "#!/bin/sh\nprintf '%s\n' '{\"type\":\"response\",\"id\":\"1\"}' 'not json at all'\n";
        let path = write_fake(script, "replay");
        let transport = Transport::spawn_rpc(path.to_str().unwrap(), &[]).unwrap();
        let msgs = drain(&transport);
        assert_eq!(msgs.len(), 2, "expected two lines, got {msgs:?}");
        match &msgs[0] {
            WireMessage::Json(v) => {
                assert_eq!(v.get("type").and_then(|t| t.as_str()), Some("response"));
                assert_eq!(v.get("id").and_then(|t| t.as_str()), Some("1"));
            }
            other => panic!("expected json, got {other:?}"),
        }
        match &msgs[1] {
            WireMessage::ParseError { raw } => assert_eq!(raw, "not json at all"),
            other => panic!("expected parse_error, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// `PI_BIN` 显式覆盖优先，且空串视为未设置（不 spawn `""`）。
    #[test]
    fn resolve_binary_prefers_nonempty_pi_bin() {
        assert_eq!(
            resolve_binary_in(Some("/x/pi".into()), None),
            "/x/pi",
            "PI_BIN 必须优先"
        );
        assert_eq!(
            resolve_binary_in(Some(String::new()), None),
            "pi",
            "空 PI_BIN 视为未设置，回退 pi"
        );
    }

    /// 未设置 `PI_BIN` 且 PATH 解析不到时回退 `"pi"`（保留原报错路径）。
    #[test]
    fn resolve_binary_falls_back_to_pi_name() {
        assert_eq!(resolve_binary_in(None, None), "pi", "无 PATH 时回退 pi");
        #[cfg(not(windows))]
        assert_eq!(
            resolve_binary_in(None, Some(std::ffi::OsString::from("/nowhere"))),
            "pi",
            "非 Windows 不做 PATH 解析，行为不变"
        );
    }

    /// Windows: PATH 上必须解析到已存在且带可执行后缀的文件，
    /// 否则 `Command::new("pi")` 报 program not found（启动回归测试）。
    #[cfg(windows)]
    #[test]
    fn resolve_binary_finds_executable_on_windows_path() {
        use std::fs;
        let dir = std::env::temp_dir().join(format!(
            "resolve-bin-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = std::env::join_paths([dir.as_path()]).unwrap();

        // 空目录: 回退 "pi"
        assert_eq!(
            resolve_binary_in(None, Some(path.clone())),
            "pi",
            "PATH 上没有 pi 时必须回退"
        );

        // 只有 pi.cmd: 必须解析到它（安装器布局）
        fs::write(dir.join("pi.cmd"), "@echo off\n").unwrap();
        let got = resolve_binary_in(None, Some(path.clone()));
        assert_eq!(got, dir.join("pi.cmd").to_string_lossy());

        // pi.exe 出现后优先于 pi.cmd（PATHEXT 优先级）
        fs::write(dir.join("pi.exe"), b"MZ").unwrap();
        let got = resolve_binary_in(None, Some(path));
        assert!(got.ends_with("pi.exe"), "exe 优先: {got}");
        assert!(std::path::Path::new(&got).is_file(), "结果必须存在: {got}");

        let _ = fs::remove_dir_all(&dir);
    }

    /// 生产路径端到端: 复刻 `cargo run` 启动序列 — `resolve_binary()` →
    /// `RpcClient::spawn` → `get_state` 握手成功，即 program not found
    /// 启动故障的回归测试。未装 pi 则跳过。
    #[test]
    fn production_spawn_resolves_real_pi() {
        use crate::rpc::client::{ResponsePolicy, RpcClient};

        let binary = resolve_binary();
        let real = find_real_pi();
        if binary == "pi" && real.is_none() {
            eprintln!("pi 不在 PATH 上, 跳过");
            return;
        }
        // PI_BIN / Windows 解析给出的路径优先；否则用 PATH 上找到的真实路径
        let target = if binary == "pi" {
            real.expect("已确认 PATH 上有 pi")
        } else {
            binary
        };
        let client =
            RpcClient::spawn(&target, &[]).expect("生产路径必须能启动 pi（不得报 program not found）");
        client
            .send_command(
                serde_json::json!({"type": "get_state"}),
                std::time::Duration::from_secs(30),
                ResponsePolicy::RequireSuccess,
            )
            .expect("pi --mode rpc 必须能完成启动握手");
        client.shutdown();
    }

    /// 在 PATH 上找真实存在的 pi（跨平台，仅测试用: 判断是否跳过端到端测试）。
    fn find_real_pi() -> Option<String> {
        #[cfg(windows)]
        {
            find_pi_on_path(std::env::var_os("PATH").as_deref())
        }
        #[cfg(not(windows))]
        {
            let path = std::env::var_os("PATH")?;
            for dir in std::env::split_paths(&path) {
                let cand = dir.join("pi");
                if cand.is_file() {
                    return Some(cand.to_string_lossy().into_owned());
                }
            }
            None
        }
    }

    #[test]
    fn missing_binary_errors_clearly() {
        let err = match Transport::spawn_rpc("/nonexistent/pi-binary-xyz", &[]) {
            Ok(_) => panic!("expected spawn to fail"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("failed to spawn"));
    }

    #[test]
    fn stderr_is_separate_from_stdout() {
        let script = "#!/bin/sh\nprintf '%s\n' 'stdout-json-line-placeholder'\necho 'a stderr warning' >&2\nprintf '%s\n' '{\"type\":\"x\"}'\n";
        let path = write_fake(script, "stderr");
        let transport = Transport::spawn_rpc(path.to_str().unwrap(), &[]).unwrap();

        let mut stderr_text = String::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if let Ok(line) = transport.stderr.recv_timeout(Duration::from_millis(200)) {
                stderr_text.push_str(&line.text);
                stderr_text.push('\n');
                if stderr_text.contains("a stderr warning") {
                    break;
                }
            }
        }
        assert!(stderr_text.contains("a stderr warning"), "stderr: {stderr_text:?}");

        let stdout_msgs = drain(&transport);
        let stdout_dump: String = stdout_msgs
            .iter()
            .map(|m| match m {
                WireMessage::Json(v) => v.to_string(),
                WireMessage::ParseError { raw } => raw.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(stdout_dump.contains("stdout-json-line-placeholder"));
        assert!(!stdout_dump.contains("a stderr warning"));

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
