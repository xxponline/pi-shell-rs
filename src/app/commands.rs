//! 本地斜杠命令分发：输入以 `/` 开头且命中命令表时本地执行（不发送模型）。
//!
//! 未命中的 `/xxx` 返回 false，由调用方按普通文本发送（与 pi 一致）。
//! 命令随 4b 任务逐个落地——表即命令集清单。

use std::sync::mpsc::Sender;

use pi_rust_tui::rpc::CommandSender;

use crate::app::event::UiEvent;
use crate::app::App;

/// 本地命令表（命中此表才拦截，其余 `/xxx` 作为普通文本发送）。
const KNOWN: &[&str] = &[
    "quit", "name", "resume", "new", "clone", "tree", "fork", "session", "compact",
];

/// 分发一条提交文本。
///
/// 输入: app — 应用状态（副作用宿主）；commander/report_tx — 发送命令与
///       回报通道（部分命令需要发 RPC）；text — trim 后的完整提交文本
/// 输出: true = 已本地处理（调用方不再发送、输入已清空或状态已变）；
///       false = 未命中本地命令，应按普通 prompt 发送
pub fn dispatch(
    app: &mut App,
    commander: &CommandSender,
    report_tx: &Sender<UiEvent>,
    text: &str,
) -> bool {
    let Some(rest) = text.strip_prefix('/') else {
        return false;
    };
    if rest.is_empty() {
        // 单独 "/" 不命中任何命令 → 普通发送
        return false;
    }
    let (name, args) = match rest.find(char::is_whitespace) {
        Some(i) => (&rest[..i], rest[i..].trim()),
        None => (rest, ""),
    };
    if !KNOWN.contains(&name) {
        return false; // 未知斜杠 → 普通文本发送
    }
    // 命中即清空输入（spec：命令执行后输入清空；submit_as 路径已提前清，
    // 直接调用 dispatch 的路径由此处保证）
    app.editor.replace("");
    match name {
        "quit" => {
            app.cmd_quit();
            true
        }
        "name" => {
            app.cmd_set_name(args, commander, report_tx);
            true
        }
        "resume" => {
            app.open_session_selector(report_tx);
            true
        }
        "new" => {
            App::spawn_session_chain(
                commander,
                report_tx,
                serde_json::json!({"type": "new_session"}),
                "new_session",
                Some("✓ 新会话已开始"),
            );
            true
        }
        "clone" => {
            App::spawn_session_chain(
                commander,
                report_tx,
                serde_json::json!({"type": "clone"}),
                "clone",
                Some("✓ 已克隆当前会话"),
            );
            true
        }
        "tree" | "fork" => {
            // /fork 与 /tree 同入口（树内回车即分支；pi 的消息选择器简化为树）
            app.open_tree(commander, report_tx);
            true
        }
        "session" => {
            app.open_session_info(commander, report_tx);
            true
        }
        "compact" => {
            app.cmd_compact(args, commander, report_tx);
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个只应答不动作的假 pi 与回报通道（dispatch 需要）。
    fn ctx() -> (App, CommandSender, Sender<UiEvent>, std::path::PathBuf) {
        let script = r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
  printf '{"type":"response","id":"%s","success":true}\n' "$id"
done
"#;
        let dir = std::env::temp_dir().join(format!(
            "slash-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pi.sh");
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let client = pi_rust_tui::rpc::RpcClient::spawn(path.to_str().unwrap(), &[]).unwrap();
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        (App::new(), commander, tx, dir)
    }

    #[test]
    fn non_slash_text_is_not_dispatched() {
        let (mut app, cmd, tx, dir) = ctx();
        assert!(!dispatch(&mut app, &cmd, &tx, "hello world"));
        assert!(!dispatch(&mut app, &cmd, &tx, "not-a-command"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bare_slash_is_not_dispatched() {
        let (mut app, cmd, tx, dir) = ctx();
        assert!(!dispatch(&mut app, &cmd, &tx, "/"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quit_command_sets_exit_and_clears_input() {
        let (mut app, cmd, tx, dir) = ctx();
        app.editor.replace("draft /quit");
        assert!(dispatch(&mut app, &cmd, &tx, "/quit"));
        assert!(app.should_quit);
        assert!(app.editor.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn name_command_sends_and_clears_input() {
        let (mut app, cmd, tx, dir) = ctx();
        app.editor.replace("/name my session");
        assert!(dispatch(&mut app, &cmd, &tx, "/name my session"));
        assert!(app.editor.is_empty(), "命令执行后输入清空");
        // RPC 已异步发出（假 pi 应答 success，无 SendFailed 即可）；参数解析
        // 已由 cmd_set_name 内部完成，断言路径见 PTY（任务 4/5 组）。
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn name_without_args_shows_usage_not_sends() {
        let (mut app, cmd, tx, dir) = ctx();
        app.editor.replace("/name");
        assert!(dispatch(&mut app, &cmd, &tx, "/name"));
        let (text, color) = app.notice.clone().expect("usage notice");
        assert!(text.contains("用法"), "{text}");
        assert!(color == ratatui::style::Color::Yellow);
        assert!(app.editor.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_slash_returns_false_for_normal_send() {
        let (mut app, cmd, tx, dir) = ctx();
        assert!(!dispatch(&mut app, &cmd, &tx, "/some-unknown-command"));
        assert!(!dispatch(&mut app, &cmd, &tx, "/model gpt-x"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
