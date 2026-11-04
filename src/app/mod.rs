//! Minimal ratatui TUI application (roadmap stage 2).
//!
//! Owns the terminal lifecycle, bridges keyboard and RPC events onto one
//! channel, reduces RPC events into a [`SessionState`], and renders a three-
//! region layout: header / scrollable chat / input editor.

mod chat;
mod event;
mod terminal;
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

/// 应用顶层状态（输入缓冲、会话状态、瞬时提示、退出标志）。
pub struct App {
    state: SessionState,
    input: String,
    /// 最近的瞬时提示（parse 错误、发送失败等）。
    notice: Option<String>,
    should_quit: bool,
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

        if let Err(e) = client.send_command(
            serde_json::json!({"type": "get_state"}),
            Duration::from_secs(15),
            ResponsePolicy::RequireSuccess,
        ) {
            Self::fail_early(
                &mut guard,
                &format!("pi 探活失败 (get_state): {e}"),
                "请先用原生 pi 完成登录后再启动本 TUI。",
            );
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
                // Idle timeout: no state changed, so no repaint.
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => self.should_quit = true,
            }
        }
        Ok(())
    }

    fn handle_key(
        &mut self,
        key: KeyEvent,
        commander: &CommandSender,
        report_tx: &Sender<UiEvent>,
    ) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        match key.code {
            KeyCode::Esc => {
                if self.state.phase == Phase::Streaming {
                    self.send_command(
                        commander,
                        report_tx,
                        serde_json::json!({"type": "abort"}),
                        false,
                    );
                } else {
                    self.should_quit = true;
                }
            }
            KeyCode::Char('q') if self.state.phase != Phase::Streaming => {
                self.should_quit = true;
            }
            KeyCode::Enter => self.submit(commander, report_tx),
            KeyCode::Backspace => {
                pop_last_char(&mut self.input);
            }
            KeyCode::Char(c) => self.input.push(c),
            _ => {}
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
    fn submit(&mut self, commander: &CommandSender, report_tx: &Sender<UiEvent>) {
        if !self.can_submit() {
            self.log_event("submit", "skipped: empty input");
            return;
        }
        let message = self.input.trim().to_string();
        self.input.clear();
        self.log_event("submit", &format!("prompt sent: {message}"));
        self.send_command(
            commander,
            report_tx,
            serde_json::json!({"type": "prompt", "message": message}),
            true,
        );
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
        terminal.draw(|frame| {
            view::render(frame, &self.state, &self.input, self.notice.as_deref());
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
}
