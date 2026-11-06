//! 外部编辑器（ctrl+g）：命令解析与"写临时文件 → 启动编辑器 → 读回"纯逻辑。
//!
//! 对齐 pi `modes/interactive/external-editor.ts`：tmpdir `pi-editor-*/prompt.md`、
//! 继承 stdio 与 cwd、退出码 0 读回去 BOM/末尾换行、失败提示且 prompt 不变、
//! 临时目录 finally 清理。

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 解析外部编辑器命令：settings.json `externalEditor` → `$VISUAL` →
/// `$EDITOR` → 平台默认（nano / Windows notepad）。
///
/// 输入: settings_path — 全局 settings.json 路径
/// 输出: 可执行命令串（按空白切分为程序+参数）
pub fn resolve_command(settings_path: &Path) -> String {
    if let Some(cmd) = crate::app::settings_io::read_external_editor(settings_path) {
        return cmd;
    }
    for key in ["VISUAL", "EDITOR"] {
        if let Ok(v) = std::env::var(key) {
            let v = v.trim().to_string();
            if !v.is_empty() {
                return v;
            }
        }
    }
    if cfg!(windows) { "notepad".into() } else { "nano".into() }
}

/// 在外部编辑器中编辑一段文本。
///
/// 输入: command — 编辑器命令（空白切分）；content — 当前输入全文
/// 输出: Ok(编辑后的全文) / Err(失败原因，调用方黄条提示且输入不变)
pub fn edit_in_external(command: &str, content: &str) -> Result<String, String> {
    let dir = temp_editor_dir().map_err(|e| format!("创建临时目录失败: {e}"))?;
    let file_path = dir.join("prompt.md");
    let result = (|| {
        std::fs::write(&file_path, content).map_err(|e| format!("写入临时文件失败: {e}"))?;
        let mut parts = command.split_whitespace();
        let program = parts.next().ok_or("编辑器命令为空")?;
        let args: Vec<&str> = parts.collect();
        let status = Command::new(program)
            .args(&args)
            .arg(&file_path)
            .status()
            .map_err(|e| format!("Failed to launch external editor: {command} ({e})"))?;
        if !status.success() {
            let code = status
                .code()
                .map(|c| format!("code {c}"))
                .unwrap_or_else(|| "signal".into());
            return Err(format!(
                "External editor exited with {code}; prompt unchanged"
            ));
        }
        let raw = std::fs::read_to_string(&file_path)
            .map_err(|e| format!("读取编辑结果失败: {e}"))?;
        let stripped = raw.strip_prefix('\u{feff}').unwrap_or(&raw);
        Ok(stripped.strip_suffix('\n').unwrap_or(stripped).to_string())
    })();
    // finally：清理临时目录（尽力而为）
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// 创建 `pi-editor-*` 临时目录。
fn temp_editor_dir() -> io::Result<PathBuf> {
    let base = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = base.join(format!("pi-editor-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    /// 串行化本模块测试：全局"临时目录已清理"断言不能与并行用例互扰。
    static LOCK: Mutex<()> = Mutex::new(());

    /// 获取测试锁（中毒则继续——单测内无 panic 依赖）。
    fn test_lock() -> MutexGuard<'static, ()> {
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 写一个可执行假编辑器脚本。
    fn fake_editor(tag: &str, body: &str) -> String {
        let dir = std::env::temp_dir().join(format!("fake-editor-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ed.sh");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path.to_string_lossy().to_string()
    }

    #[test]
    fn resolve_command_priority_order() {
        let dir = std::env::temp_dir().join(format!("ext-ed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let settings = dir.join("settings.json");

        // 1) settings 优先
        std::fs::write(&settings, r#"{"externalEditor":"vim -u NONE"}"#).unwrap();
        assert_eq!(resolve_command(&settings), "vim -u NONE");

        // 2) settings 缺失 → 环境变量（VISUAL 优先于 EDITOR）
        let settings_missing = dir.join("missing.json");
        unsafe { std::env::set_var("VISUAL", "my-visual") };
        unsafe { std::env::set_var("EDITOR", "my-editor") };
        assert_eq!(resolve_command(&settings_missing), "my-visual");
        unsafe { std::env::remove_var("VISUAL") };
        assert_eq!(resolve_command(&settings_missing), "my-editor");
        unsafe { std::env::remove_var("EDITOR") };

        // 3) 全缺 → 平台默认
        let expected = if cfg!(windows) { "notepad" } else { "nano" };
        assert_eq!(resolve_command(&settings_missing), expected);

        // 空白值视为未配置
        std::fs::write(&settings, r#"{"externalEditor":"   "}"#).unwrap();
        unsafe { std::env::set_var("VISUAL", "v2") };
        assert_eq!(resolve_command(&settings), "v2");
        unsafe { std::env::remove_var("VISUAL") };
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_success_replaces_content_and_cleans_tmp() {
        let _guard = test_lock();
        // 假编辑器：换行分隔追加一行后退出 0
        let ed = fake_editor("ok", r#"printf '\nedited\n' >> "$1""#);
        let out = edit_in_external(&ed, "original").expect("应成功");
        assert_eq!(out, "original\nedited", "读回并去末尾换行语义: {out:?}");

        // 末尾换行剥离（vim 保存的单个尾换行）
        let ed2 = fake_editor("nl", r#"printf 'line1\nline2\n' > "$1""#);
        let out = edit_in_external(&ed2, "x").expect("应成功");
        assert_eq!(out, "line1\nline2", "仅去一个末尾换行");

        // BOM 剥离
        let ed3 = fake_editor("bom", r#"printf '\357\273\277bommed' > "$1""#);
        let out = edit_in_external(&ed3, "").expect("应成功");
        assert_eq!(out, "bommed", "去 BOM");

        // 临时目录已清理（只查本测试进程创建的——其它 pid 为历史残留，不归本用例管）
        let prefix = format!("pi-editor-{}-", std::process::id());
        let entries: Vec<_> = std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
            .collect();
        assert!(entries.is_empty(), "临时目录应被清理: {entries:?}");
    }

    #[test]
    fn edit_failure_keeps_prompt_and_reports_reason() {
        let _guard = test_lock();
        // 非零退出
        let ed = fake_editor("fail", r#"exit 3"#);
        let err = edit_in_external(&ed, "keep me").expect_err("应失败");
        assert!(err.contains("exited with code 3"), "{err}");
        assert!(err.contains("prompt unchanged"), "{err}");

        // 命令不存在
        let err = edit_in_external("/nonexistent/editor-xyz", "keep").expect_err("应失败");
        assert!(err.contains("Failed to launch external editor"), "{err}");

        // 空命令
        let err = edit_in_external("   ", "keep").expect_err("应失败");
        assert!(err.contains("编辑器命令为空"), "{err}");

        // 失败后临时目录同样清理（只查本进程）
        let prefix = format!("pi-editor-{}-", std::process::id());
        let entries: Vec<_> = std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
            .collect();
        assert!(entries.is_empty(), "失败路径也清理: {entries:?}");
    }
}
