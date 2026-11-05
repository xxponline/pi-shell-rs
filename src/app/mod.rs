//! Minimal ratatui TUI application (roadmap stage 2).
//!
//! Owns the terminal lifecycle, bridges keyboard and RPC events onto one
//! channel, reduces RPC events into a [`SessionState`], and renders a three-
//! region layout: header / scrollable chat / input editor.

mod chat;
mod event;
mod markdown;
mod terminal;
mod tools;
mod view;
mod wrap;

use std::io;
use std::sync::mpsc::{Receiver, Sender};
use std::thread;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pi_rust_tui::rpc::{CommandSender, ResponsePolicy, RpcClient};
use pi_rust_tui::session::{Phase, SessionState};

use crate::app::event::UiEvent;
use crate::app::terminal::TerminalGuard;

/// 按键意图（`key_intent` 的纯映射产物，便于单测）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyIntent {
    /// 退出程序（ctrl+c / 空闲 esc/q）
    Quit,
    /// 切换 thinking 展开（ctrl+t）
    ToggleThinking,
    /// 切换工具输出展开（ctrl+o）
    ToggleTools,
    /// 发送 prompt（Enter）
    Submit,
    /// 发送 follow_up（alt+enter，流式中排队）
    SendFollowUp,
    /// 取回全部排队消息（alt+up → clear_queue）
    Dequeue,
    /// 中止生成（流式 esc）
    Abort,
    /// 普通编辑（可打印字符 / 退格）
    Edit,
    /// 忽略
    Ignore,
}

/// 应用顶层状态（输入缓冲、会话状态、瞬时提示、退出标志）。
pub struct App {
    state: SessionState,
    input: String,
    /// 最近的瞬时提示（parse 错误、发送失败等）。
    notice: Option<String>,
    should_quit: bool,
    /// thinking 是否展开（ctrl+t 切换，默认折叠）。
    show_thinking: bool,
    /// 工具输出是否展开（ctrl+o 切换，默认折叠）。
    expand_tools: bool,
    /// 诊断日志句柄；环境变量 `PI_RUST_TUI_LOG=<path>` 开启时逐条记录
    /// 收到的 UiEvent，用于排查“回复不显示”类问题；None 表示未开启。
    debug_log: Option<std::fs::File>,
}

impl App {
    /// 创建应用状态。
    ///
    /// 输入: 无
    /// 输出: 空闲态 App；若设置 PI_RUST_TUI_LOG 则同时创建/截断该日志文件
    pub fn new() -> Self {
        let debug_log = std::env::var("PI_RUST_TUI_LOG")
            .ok()
            .and_then(|path| std::fs::File::create(path).ok());
        Self {
            state: SessionState::new(),
            input: String::new(),
            notice: None,
            should_quit: false,
            show_thinking: false,
            expand_tools: false,
            debug_log,
        }
    }

    /// 向诊断日志追加一行 `kind detail`；未开启或写失败时静默忽略。
    ///
    /// 输入: kind — 事件类别（如 rpc/key/send）；detail — 详情文本（超 1500 字符截断）
    /// 输出: 无返回值；副作用为向日志文件追加一行并 flush
    fn log_event(&mut self, kind: &str, detail: &str) {
        if let Some(f) = self.debug_log.as_mut() {
            use std::io::Write;
            let detail: String = detail.chars().take(1500).collect();
            let _ = writeln!(f, "{kind} {detail}");
            let _ = f.flush();
        }
    }

    /// Run the full TUI against a spawned pi RPC client.
    pub fn run(mut self) -> io::Result<()> {
        let binary = pi_rust_tui::rpc::resolve_binary();
        let mut guard = TerminalGuard::enter().map_err(|e| {
            io::Error::other(format!(
                "无法进入终端 raw/alt-screen 模式（是否在真实交互终端中运行？）: {e}"
            ))
        })?;

        let mut client = match RpcClient::spawn(&binary, &[]) {
            Ok(c) => c,
            Err(e) => Self::fail_early(
                &mut guard,
                &format!("无法启动 pi ({binary}): {e}"),
                "请确认已安装 pi，并用原生 pi 完成登录（~/.pi/agent/auth.json）。",
            ),
        };

        match client.send_command(
            serde_json::json!({"type": "get_state"}),
            Duration::from_secs(15),
            ResponsePolicy::RequireSuccess,
        ) {
            // 探活成功：把 model/sessionName/thinkingLevel 应用到状态（状态行显示）。
            Ok(data) => self.state.apply_get_state(&data),
            Err(e) => Self::fail_early(
                &mut guard,
                &format!("pi 探活失败 (get_state): {e}"),
                "请先用原生 pi 完成登录后再启动本 TUI。",
            ),
        }

        let (ui_rx, report_tx) = event::spawn_event_sources(&mut client);
        let mut terminal = guard.take_terminal();

        let commander = client.command_sender();
        let result = self.event_loop(&mut terminal, &ui_rx, &commander, &report_tx);

        drop(terminal);
        guard.finish();
        result
    }

    fn event_loop(
        &mut self,
        terminal: &mut ratatui::DefaultTerminal,
        ui_rx: &Receiver<UiEvent>,
        commander: &CommandSender,
        report_tx: &Sender<UiEvent>,
    ) -> io::Result<()> {
        self.draw(terminal)?;

        while !self.should_quit {
            match ui_rx.recv_timeout(Duration::from_millis(200)) {
                Ok(UiEvent::Key(key)) => {
                    self.log_event("key", &format!("{key:?}"));
                    self.handle_key(key, commander, report_tx);
                    self.draw(terminal)?;
                }
                Ok(UiEvent::Rpc(raw)) => {
                    self.log_event("rpc", &raw.to_string());
                    self.state.handle(&raw);
                    self.draw(terminal)?;
                }
                Ok(UiEvent::ParseError(raw)) => {
                    self.log_event("parse_error", &raw);
                    self.notice = Some(format!("[parse_error] {raw}"));
                    self.draw(terminal)?;
                }
                Ok(UiEvent::Closed) => {
                    self.log_event("closed", "stdout closed");
                    self.notice = Some("pi 连接已关闭".to_string());
                    self.should_quit = true;
                }
                Ok(UiEvent::SendFailed(err)) => {
                    self.log_event("send_failed", &err);
                    self.notice = Some(err);
                    self.draw(terminal)?;
                }
                Ok(UiEvent::RestoreQueued(data)) => {
                    self.log_event("dequeue", &data.to_string());
                    self.restore_queued(&data);
                    self.draw(terminal)?;
                }
                // Idle timeout: no state changed, so no repaint.
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => self.should_quit = true,
            }
        }
        Ok(())
    }

    /// 处理一个按键：先做纯意图映射，再应用状态，最后按需走命令通道。
    ///
    /// 输入: key — 原始按键；commander/report_tx — 发送 prompt/abort 所需
    /// 输出: 无返回值；副作用为更新输入/标志/退出或异步发命令
    fn handle_key(
        &mut self,
        key: KeyEvent,
        commander: &CommandSender,
        report_tx: &Sender<UiEvent>,
    ) {
        let intent = Self::key_intent(key, self.state.phase == Phase::Streaming);
        match self.apply_intent(key, intent) {
            Some(KeyIntent::Submit) => self.submit_as(commander, report_tx, "prompt"),
            Some(KeyIntent::SendFollowUp) => self.submit_as(commander, report_tx, "follow_up"),
            Some(KeyIntent::Abort) => self.send_command(
                commander,
                report_tx,
                serde_json::json!({"type": "abort"}),
                false,
            ),
            Some(KeyIntent::Dequeue) => self.request_dequeue(commander, report_tx),
            _ => {}
        }
    }

    /// 纯按键 → 意图映射（不触碰状态，便于单测）。
    ///
    /// 输入: key — 原始按键；streaming — 当前是否在流式生成中
    /// 输出: KeyIntent（Quit/Toggle*/Submit/Abort/Edit/Ignore）
    fn key_intent(key: KeyEvent, streaming: bool) -> KeyIntent {
        use KeyCode::*;
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == Char('c') {
            return KeyIntent::Quit;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            Esc if streaming => KeyIntent::Abort,
            Esc => KeyIntent::Quit,
            Char('q') if !streaming => KeyIntent::Quit,
            Char('t') if ctrl => KeyIntent::ToggleThinking,
            Char('o') if ctrl => KeyIntent::ToggleTools,
            Enter if alt => KeyIntent::SendFollowUp,
            Enter => KeyIntent::Submit,
            Up if alt => KeyIntent::Dequeue,
            Backspace | Char(_) => KeyIntent::Edit,
            _ => KeyIntent::Ignore,
        }
    }

    /// 应用纯状态意图；需要命令通道的意图（Submit/Abort）原样返回。
    ///
    /// 输入: key — 原始按键（Edit 分支需要）；intent — key_intent 产物
    /// 输出: Some(intent) 表示需调用方走命令通道；否则 None
    fn apply_intent(&mut self, key: KeyEvent, intent: KeyIntent) -> Option<KeyIntent> {
        match intent {
            KeyIntent::Quit => {
                self.should_quit = true;
                None
            }
            KeyIntent::ToggleThinking => {
                self.show_thinking = !self.show_thinking;
                self.log_event(
                    "key",
                    &format!("thinking toggled -> {}", self.show_thinking),
                );
                None
            }
            KeyIntent::ToggleTools => {
                self.expand_tools = !self.expand_tools;
                self.log_event("key", &format!("tools toggled -> {}", self.expand_tools));
                None
            }
            KeyIntent::Edit => {
                match key.code {
                    KeyCode::Char(c) => self.input.push(c),
                    KeyCode::Backspace => pop_last_char(&mut self.input),
                    _ => {}
                }
                None
            }
            KeyIntent::Ignore => None,
            KeyIntent::Submit
            | KeyIntent::Abort
            | KeyIntent::SendFollowUp
            | KeyIntent::Dequeue => Some(intent),
        }
    }

    /// 判断当前输入是否构成可发送的消息（去空白后非空）。
    ///
    /// 输入: self — 当前应用状态（读 input）
    /// 输出: true 表示可以发送；false 表示应忽略 Enter
    fn can_submit(&self) -> bool {
        !self.input.trim().is_empty()
    }

    /// 把当前输入作为 prompt 发出（空消息直接忽略）。
    ///
    /// 输入: commander — RPC 命令句柄；report_tx — 失败回报通道
    /// 输出: 无返回值；副作用为清空输入、记录日志、异步发送 prompt
    /// 把当前输入作为指定命令（prompt/follow_up）发出；空输入忽略。
    ///
    /// 输入: commander — RPC 命令句柄；report_tx — 失败回报通道；
    ///       command — "prompt" 或 "follow_up"
    /// 输出: 无返回值；副作用为清空输入、记日志、异步发送
    fn submit_as(&mut self, commander: &CommandSender, report_tx: &Sender<UiEvent>, command: &str) {
        if !self.can_submit() {
            self.log_event("submit", &format!("skipped: empty input ({command})"));
            return;
        }
        let message = self.input.trim().to_string();
        self.input.clear();
        self.log_event("submit", &format!("{command} sent: {message}"));
        self.send_command(
            commander,
            report_tx,
            serde_json::json!({ "type": command, "message": message }),
            true,
        );
    }

    /// 把 clear_queue 返回的排队消息回填输入框（非空则换行追加，避免丢字）。
    ///
    /// 输入: data — clear_queue 响应 {steering:[..], followUp:[..]}
    /// 输出: 无返回值；副作用为更新 input
    fn restore_queued(&mut self, data: &serde_json::Value) {
        let mut restored: Vec<String> = Vec::new();
        for key in ["steering", "followUp"] {
            if let Some(arr) = data.get(key).and_then(|v| v.as_array()) {
                for item in arr {
                    if let Some(text) = item.as_str() {
                        restored.push(text.to_string());
                    }
                }
            }
        }
        if restored.is_empty() {
            return;
        }
        let joined = restored.join("\n");
        if self.input.is_empty() {
            self.input = joined;
        } else {
            self.input.push('\n');
            self.input.push_str(&joined);
        }
    }

    /// alt+up：发送 clear_queue，返回的排队消息经 RestoreQueued 事件回填输入框。
    ///
    /// 输入: commander — RPC 命令句柄；report_tx — 结果/失败回报通道
    /// 输出: 无返回值；副作用为异步发 clear_queue（队列空时仅记日志跳过）
    fn request_dequeue(&mut self, commander: &CommandSender, report_tx: &Sender<UiEvent>) {
        if self.state.steering_queue.is_empty() && self.state.follow_up_queue.is_empty() {
            self.log_event("dequeue", "skipped: queue empty");
            return;
        }
        self.log_event("dequeue", "clear_queue requested");
        let report = report_tx.clone();
        let cmd = commander.clone();
        thread::spawn(move || {
            match cmd.send_command(
                serde_json::json!({"type": "clear_queue"}),
                Duration::from_secs(10),
                ResponsePolicy::RequireSuccess,
            ) {
                Ok(data) => {
                    let _ = report.send(UiEvent::RestoreQueued(data));
                }
                Err(e) => {
                    let _ = report.send(UiEvent::SendFailed(e.to_string()));
                }
            }
        });
    }

    /// Fire a command on a detached thread so waiting never blocks rendering.
    fn send_command(
        &self,
        commander: &CommandSender,
        report_tx: &Sender<UiEvent>,
        value: serde_json::Value,
        require_success: bool,
    ) {
        let report = report_tx.clone();
        let cmd = commander.clone();
        thread::spawn(move || {
            let policy = if require_success {
                ResponsePolicy::RequireSuccess
            } else {
                ResponsePolicy::AcceptError
            };
            if let Err(e) = cmd.send_command(value, Duration::from_secs(60), policy) {
                let _ = report.send(UiEvent::SendFailed(e.to_string()));
            }
        });
    }

    fn draw(&self, terminal: &mut ratatui::DefaultTerminal) -> io::Result<()> {
        let flags = crate::app::chat::RenderFlags {
            show_thinking: self.show_thinking,
            expand_tools: self.expand_tools,
        };
        terminal.draw(|frame| {
            view::render(frame, &self.state, &self.input, self.notice.as_deref(), flags);
        })?;
        Ok(())
    }

}

use std::sync::mpsc;

/// Remove the last character (char-safe, not byte).
fn pop_last_char(s: &mut String) {
    if let Some(idx) = s.char_indices().next_back().map(|(i, _)| i) {
        s.truncate(idx);
    }
}

#[cfg(test)]
mod input_tests {
    use super::*;

    #[test]
    fn append_and_backspace_are_char_safe() {
        let mut s = String::new();
        s.push('a');
        s.push('b');
        s.push('中');
        assert_eq!(s, "ab中");
        pop_last_char(&mut s);
        assert_eq!(s, "ab");
        pop_last_char(&mut s);
        assert_eq!(s, "a");
        pop_last_char(&mut s);
        assert!(s.is_empty());
        pop_last_char(&mut s); // no panic on empty
        assert!(s.is_empty());
    }

    #[test]
    fn blank_input_does_not_submit() {
        let mut app = App::new();
        app.input = "   \n  ".to_string();
        assert!(!app.can_submit());
        app.input = "hello".to_string();
        assert!(app.can_submit());
    }

    #[test]
    fn ctrl_t_toggles_thinking_and_ctrl_o_toggles_tools() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = App::new();
        assert!(!app.show_thinking);
        assert!(!app.expand_tools);

        // 无 CONTROL 的 t → 意图为 Edit → 进输入框，不影响折叠态
        let plain = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE);
        let intent = App::key_intent(plain, false);
        assert_eq!(intent, KeyIntent::Edit);
        assert_eq!(app.apply_intent(plain, intent), None);
        assert!(!app.show_thinking);
        assert_eq!(app.input, "t");
        app.input.clear();

        // ctrl+t → ToggleThinking → 取反，不进输入框
        let ctrl_t = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL);
        assert_eq!(App::key_intent(ctrl_t, false), KeyIntent::ToggleThinking);
        app.apply_intent(ctrl_t, KeyIntent::ToggleThinking);
        assert!(app.show_thinking);
        app.apply_intent(ctrl_t, KeyIntent::ToggleThinking);
        assert!(!app.show_thinking);

        // ctrl+o → ToggleTools
        let ctrl_o = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL);
        assert_eq!(App::key_intent(ctrl_o, false), KeyIntent::ToggleTools);
        app.apply_intent(ctrl_o, KeyIntent::ToggleTools);
        assert!(app.expand_tools);
    }

    #[test]
    fn key_intent_mapping_matches_spec() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let k = |code, mods| KeyEvent::new(code, mods);
        // ctrl+c 随时退出
        assert_eq!(App::key_intent(k(KeyCode::Char('c'), KeyModifiers::CONTROL), true), KeyIntent::Quit);
        // esc: 流式=中止, 空闲=退出
        assert_eq!(App::key_intent(k(KeyCode::Esc, KeyModifiers::NONE), true), KeyIntent::Abort);
        assert_eq!(App::key_intent(k(KeyCode::Esc, KeyModifiers::NONE), false), KeyIntent::Quit);
        // 空闲 q=退出, 流式 q=编辑
        assert_eq!(App::key_intent(k(KeyCode::Char('q'), KeyModifiers::NONE), false), KeyIntent::Quit);
        assert_eq!(App::key_intent(k(KeyCode::Char('q'), KeyModifiers::NONE), true), KeyIntent::Edit);
        // Enter=提交, 退格/字符=编辑, 其它=忽略
        assert_eq!(App::key_intent(k(KeyCode::Enter, KeyModifiers::NONE), true), KeyIntent::Submit);
        assert_eq!(App::key_intent(k(KeyCode::Backspace, KeyModifiers::NONE), false), KeyIntent::Edit);
        assert_eq!(App::key_intent(k(KeyCode::F(5), KeyModifiers::NONE), false), KeyIntent::Ignore);
    }

    #[test]
    fn alt_keys_map_to_follow_up_and_dequeue() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let k = |code, mods| KeyEvent::new(code, mods);
        // alt+enter → SendFollowUp；普通 enter → Submit（行为不变）
        assert_eq!(
            App::key_intent(k(KeyCode::Enter, KeyModifiers::ALT), true),
            KeyIntent::SendFollowUp
        );
        assert_eq!(
            App::key_intent(k(KeyCode::Enter, KeyModifiers::NONE), true),
            KeyIntent::Submit
        );
        // alt+up → Dequeue；普通 up → Ignore（行为不变）
        assert_eq!(
            App::key_intent(k(KeyCode::Up, KeyModifiers::ALT), true),
            KeyIntent::Dequeue
        );
        assert_eq!(
            App::key_intent(k(KeyCode::Up, KeyModifiers::NONE), false),
            KeyIntent::Ignore
        );
    }

    #[test]
    fn dequeue_on_empty_queue_is_noop() {
        let mut app = App::new();
        // 空队列：不产生任何状态变化（发送层在 request_dequeue 内拦截）
        assert!(app.state.steering_queue.is_empty());
        assert!(app.state.follow_up_queue.is_empty());
        // restore_queued 对空数据/空列表无副作用
        app.restore_queued(&serde_json::json!({"steering": [], "followUp": []}));
        assert!(app.input.is_empty());
    }

    #[test]
    fn restore_queued_appends_to_existing_input() {
        let mut app = App::new();
        app.input = "existing".into();
        app.restore_queued(&serde_json::json!({
            "steering": ["s1"],
            "followUp": ["f1", "f2"]
        }));
        assert_eq!(app.input, "existing\ns1\nf1\nf2");
    }

    #[test]
    fn follow_up_and_prompt_submit_clear_input() {
        let mut app = App::new();
        app.input = "  hello  ".into();
        assert!(app.can_submit());
        app.input = "   ".into();
        assert!(!app.can_submit());
    }
}
