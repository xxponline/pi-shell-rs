//! 本地斜杠命令分发：输入以 `/` 开头且命中命令表时本地执行（不发送模型）。
//!
//! 未命中的 `/xxx` 返回 false，由调用方按普通文本发送（与 pi 一致）。
//! `COMMANDS` 是命令元数据的**单源**：分发命中判定、补全候选、参数补全开关
//! 都从这里读（对照 pi `core/slash-commands.ts` 的 BUILTIN_SLASH_COMMANDS）。

use std::sync::mpsc::Sender;

use pi_shell_rs::rpc::CommandSender;

use crate::app::event::UiEvent;
use crate::app::App;

/// 命令参数补全类别（决定 `/cmd ` 空格后的候选来源）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgCompletion {
    /// 无参数补全（空格后不弹菜单）。
    None,
    /// 可用模型列表（`/model `）。
    Models,
    /// 可用 thinking 级别（`/thinking `）。
    ThinkingLevels,
}

/// 一条本地命令的元数据。
#[derive(Debug, Clone, Copy)]
pub struct CommandSpec {
    /// 命令名（不含 `/`）。
    pub name: &'static str,
    /// 补全菜单里的说明文本。
    pub description: &'static str,
    /// 参数提示（如 `<level>`；显示为 `参数提示 — 说明`）。
    pub argument_hint: Option<&'static str>,
    /// 空格后的参数补全类别。
    pub arg_completion: ArgCompletion,
}

/// pi 全部 24 个内置命令（名称与描述对照 pi BUILTIN_SLASH_COMMANDS）。
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec { name: "settings", description: "Open settings menu", argument_hint: None, arg_completion: ArgCompletion::None },
    CommandSpec { name: "model", description: "Select model (opens selector UI)", argument_hint: Some("<provider/model>"), arg_completion: ArgCompletion::Models },
    CommandSpec { name: "tree", description: "Navigate session tree (switch branches)", argument_hint: None, arg_completion: ArgCompletion::None },
    CommandSpec { name: "thinking", description: "Set thinking level", argument_hint: Some("<level>"), arg_completion: ArgCompletion::ThinkingLevels },
    CommandSpec { name: "scoped-models", description: "Enable/disable models for Ctrl+P cycling", argument_hint: None, arg_completion: ArgCompletion::None },
    CommandSpec { name: "export", description: "Export session (HTML default, or specify path: .html/.jsonl)", argument_hint: Some("[path]"), arg_completion: ArgCompletion::None },
    CommandSpec { name: "import", description: "Import a JSONL session, preserving its ID and working directory", argument_hint: Some("<path.jsonl>"), arg_completion: ArgCompletion::None },
    CommandSpec { name: "share", description: "Share session via a Radius artifact or a GitHub gist", argument_hint: None, arg_completion: ArgCompletion::None },
    CommandSpec { name: "bug", description: "Report a bug to the Pi developers", argument_hint: Some("<description>"), arg_completion: ArgCompletion::None },
    CommandSpec { name: "copy", description: "Copy last agent message to clipboard", argument_hint: None, arg_completion: ArgCompletion::None },
    CommandSpec { name: "name", description: "Set session display name", argument_hint: Some("<name>"), arg_completion: ArgCompletion::None },
    CommandSpec { name: "session", description: "Show session info and stats", argument_hint: None, arg_completion: ArgCompletion::None },
    CommandSpec { name: "changelog", description: "Show changelog entries", argument_hint: None, arg_completion: ArgCompletion::None },
    CommandSpec { name: "hotkeys", description: "Show all keyboard shortcuts", argument_hint: None, arg_completion: ArgCompletion::None },
    CommandSpec { name: "fork", description: "Create a new fork from a previous user message", argument_hint: None, arg_completion: ArgCompletion::None },
    CommandSpec { name: "clone", description: "Duplicate the current session at the current position", argument_hint: None, arg_completion: ArgCompletion::None },
    CommandSpec { name: "trust", description: "Save project trust decision for future sessions", argument_hint: None, arg_completion: ArgCompletion::None },
    CommandSpec { name: "login", description: "Configure provider authentication", argument_hint: Some("<provider>"), arg_completion: ArgCompletion::None },
    CommandSpec { name: "logout", description: "Remove provider authentication", argument_hint: None, arg_completion: ArgCompletion::None },
    CommandSpec { name: "new", description: "Start a new session", argument_hint: None, arg_completion: ArgCompletion::None },
    CommandSpec { name: "compact", description: "Manually compact the session context", argument_hint: Some("[instructions]"), arg_completion: ArgCompletion::None },
    CommandSpec { name: "resume", description: "Resume another session", argument_hint: None, arg_completion: ArgCompletion::None },
    CommandSpec { name: "reload", description: "Reload keybindings, extensions, skills, prompts, themes, and context files", argument_hint: None, arg_completion: ArgCompletion::None },
    CommandSpec { name: "quit", description: "Quit pi", argument_hint: None, arg_completion: ArgCompletion::None },
];

/// 命令名是否在本地命令表中（命中才拦截）。
///
/// 输入: name — 不含 `/` 的命令名
/// 输出: true = 命中本地命令表
pub fn is_known(name: &str) -> bool {
    COMMANDS.iter().any(|c| c.name == name)
}

/// 按命令名取元数据（补全候选与参数补全用）。
///
/// 输入: name — 不含 `/` 的命令名
/// 输出: Some(元数据) / None（不在表中）
pub fn find(name: &str) -> Option<&'static CommandSpec> {
    COMMANDS.iter().find(|c| c.name == name)
}

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
    if !is_known(name) {
        return false; // 未知斜杠 → 普通文本发送
    }
    // 命中即清空输入（spec：命令执行后输入清空；submit_as 路径已提前清，
    // 直接调用 dispatch 的路径由此处保证）
    app.editor.replace("");
    match name {
        // ── 会话管理（4b） ──
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
        // ── 选择器入口（spec：model/thinking 斜杠入口与精确匹配） ──
        "model" => {
            app.cmd_model(args, commander, report_tx);
            true
        }
        "thinking" => {
            app.cmd_thinking(args, commander, report_tx);
            true
        }
        // ── 本地信息与剪贴板（本 change 组 4） ──
        "copy" => {
            app.cmd_copy(commander, report_tx);
            true
        }
        "hotkeys" => {
            app.open_hotkeys();
            true
        }
        "changelog" => {
            app.cmd_changelog();
            true
        }
        "trust" => {
            app.open_trust();
            true
        }
        "settings" => {
            app.open_settings(commander, report_tx);
            true
        }
        "scoped-models" => {
            app.open_scoped_models(commander, report_tx);
            true
        }
        "export" => {
            app.cmd_export(args, commander, report_tx);
            true
        }
        "import" => {
            app.cmd_import(args);
            true
        }
        "reload" => {
            app.cmd_reload(commander, report_tx);
            true
        }
        // ── 桩命令（spec：未支持命令提示桩） ──
        "share" | "bug" | "login" | "logout" => {
            app.cmd_stub(name);
            true
        }
        // ── 其余命令随本 change 后续任务接线 ──
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
        let client = pi_shell_rs::rpc::RpcClient::spawn(path.to_str().unwrap(), &[]).unwrap();
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        (App::new(), commander, tx, dir)
    }

    #[test]
    fn commands_table_covers_pi_builtins() {
        // 单源断言：24 个 pi 内置命令一个不少（对照 pi BUILTIN_SLASH_COMMANDS）
        let expected = [
            "settings", "model", "tree", "thinking", "scoped-models", "export", "import",
            "share", "bug", "copy", "name", "session", "changelog", "hotkeys", "fork",
            "clone", "trust", "login", "logout", "new", "compact", "resume", "reload", "quit",
        ];
        assert_eq!(COMMANDS.len(), 24, "命令总数应为 24");
        for name in expected {
            assert!(is_known(name), "缺少命令: {name}");
            assert!(find(name).is_some(), "元数据缺失: {name}");
        }
        // 描述非空（补全菜单依赖说明列）
        for spec in COMMANDS {
            assert!(!spec.description.is_empty(), "{} 缺描述", spec.name);
        }
        // 参数补全类别只给 model/thinking
        assert_eq!(find("model").unwrap().arg_completion, ArgCompletion::Models);
        assert_eq!(find("thinking").unwrap().arg_completion, ArgCompletion::ThinkingLevels);
        assert_eq!(find("copy").unwrap().arg_completion, ArgCompletion::None);
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
        let _ = std::fs::remove_dir_all(&dir);
    }
}
