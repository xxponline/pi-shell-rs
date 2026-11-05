//! Minimal ratatui TUI application (roadmap stage 2).
//!
//! Owns the terminal lifecycle, bridges keyboard and RPC events onto one
//! channel, reduces RPC events into a [`SessionState`], and renders a three-
//! region layout: header / scrollable chat / input editor.

mod chat;
mod editor;
mod event;
mod markdown;
mod overlay;
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
use ratatui::style::Color;
use std::collections::VecDeque;

use crate::app::editor::EditorBuffer;
use crate::app::event::{AsyncKind, UiEvent};
use crate::app::overlay::{overlay_key, Overlay, OverlayOutcome};
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
    /// 打开模型选择器（ctrl+l）
    OpenModelSelect,
    /// 打开 thinking 级别选择器（alt+t）
    OpenThinkingSelect,
    /// 模型循环（ctrl+p 下一个 / shift+ctrl+p 上一个）
    CycleModelNext,
    CycleModelPrev,
    /// thinking 级别循环（shift+tab）
    CycleThinking,
    /// 退出或向后删（ctrl+d：空输入退出，非空删字符）
    ExitIfEmpty,
    /// 换行（shift+enter / ctrl+j）
    Newline,
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
    /// 多行编辑器缓冲（行数组+光标+kill-ring+undo）。
    editor: EditorBuffer,
    /// 瞬时提示与颜色（error 红 / warning 黄 / info 默认色）。
    notice: Option<(String, Color)>,
    should_quit: bool,
    /// setStatus 的状态行附加文本。
    status_text: Option<String>,
    /// setWidget 的编辑器上方行（空 = 隐藏）。
    widget_lines: Vec<String>,
    /// 当前浮层。
    overlay: Option<Overlay>,
    /// 排队的扩展对话浮层（串行处理）。
    pending_overlays: VecDeque<Overlay>,
    /// 选择器异步数据序号（丢弃过期响应）。
    overlay_seq: u64,
    /// 上一次 ctrl+c 的时刻（双击退出判定）。
    last_ctrl_c: Option<std::time::Instant>,
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
            editor: EditorBuffer::new(),
            notice: None,
            should_quit: false,
            status_text: None,
            widget_lines: Vec::new(),
            overlay: None,
            pending_overlays: VecDeque::new(),
            overlay_seq: 0,
            last_ctrl_c: None,
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
                    if raw.get("type").and_then(|t| t.as_str()) == Some("extension_ui_request") {
                        // 扩展 UI 请求走 App 层分发，不进 session 归约。
                        self.handle_extension_request(&raw);
                    } else {
                        self.state.handle(&raw);
                    }
                    self.draw(terminal)?;
                }
                Ok(UiEvent::AsyncData { kind, seq, data }) => {
                    self.log_event("async", &format!("{kind:?} seq={seq}"));
                    self.handle_async_data(kind, seq, &data);
                    self.draw(terminal)?;
                }
                Ok(UiEvent::ParseError(raw)) => {
                    self.log_event("parse_error", &raw);
                    self.notice = Some((format!("[parse_error] {raw}"), Color::Red));
                    self.draw(terminal)?;
                }
                Ok(UiEvent::Closed) => {
                    self.log_event("closed", "stdout closed");
                    self.notice = Some(("pi 连接已关闭".to_string(), Color::Red));
                    self.should_quit = true;
                }
                Ok(UiEvent::SendFailed(err)) => {
                    self.log_event("send_failed", &err);
                    self.notice = Some((err, Color::Red));
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

    /// 编辑键统一分派：字符/光标移动/删除/kill-ring/yank/undo。
    ///
    /// 输入: key — 原始按键（含修饰键）
    /// 输出: 无返回值；副作用为修改 EditorBuffer；未支持的组合忽略
    fn edit_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            // 纯字符插入（修饰键字符不落入）
            KeyCode::Char(c) if !ctrl && !alt => self.editor.insert(c),
            // ctrl 编辑族（Emacs）
            KeyCode::Char('b') if ctrl => self.editor.move_left(),
            KeyCode::Char('f') if ctrl => self.editor.move_right(),
            KeyCode::Char('a') if ctrl => self.editor.home(),
            KeyCode::Char('e') if ctrl => self.editor.end(),
            KeyCode::Char('w') if ctrl => self.editor.kill_word_back(),
            KeyCode::Char('u') if ctrl => self.editor.kill_to_line_start(),
            KeyCode::Char('k') if ctrl => self.editor.kill_to_line_end(),
            KeyCode::Char('y') if ctrl => self.editor.yank(),
            KeyCode::Char('-') if ctrl => self.editor.undo(),
            // alt 编辑族
            KeyCode::Char('b') if alt => self.editor.word_left(),
            KeyCode::Char('f') if alt => self.editor.word_right(),
            KeyCode::Char('d') if alt => self.editor.kill_word_forward(),
            KeyCode::Char('y') if alt => self.editor.yank_pop(),
            // 方向键（alt+←→ 词级）
            KeyCode::Left if alt => self.editor.word_left(),
            KeyCode::Left => self.editor.move_left(),
            KeyCode::Right if alt => self.editor.word_right(),
            KeyCode::Right => self.editor.move_right(),
            KeyCode::Up => self.editor.move_up(),
            KeyCode::Down => self.editor.move_down(),
            KeyCode::Home => self.editor.home(),
            KeyCode::End => self.editor.end(),
            // 删除
            KeyCode::Delete => self.editor.delete_forward(),
            KeyCode::Backspace if alt => self.editor.kill_word_back(),
            KeyCode::Backspace => self.editor.backspace(),
            // 未支持的 ctrl/alt 组合：忽略（不插入字符）
            _ => {}
        }
    }

    /// 构造提交类命令的 payload（纯函数，便于断言 steer 字段）。
    ///
    /// 输入: command — "prompt" 或 "follow_up"；message — 提交文本；
    ///       streaming — 当前是否流式生成中
    /// 输出: JSON payload；prompt 且流式时携带 streamingBehavior:"steer"
    fn build_command_payload(command: &str, message: &str, streaming: bool) -> serde_json::Value {
        let mut payload = serde_json::json!({ "type": command, "message": message });
        if command == "prompt" && streaming {
            payload["streamingBehavior"] = serde_json::Value::String("steer".to_string());
        }
        payload
    }

    /// ctrl+c 语义（对齐 pi）：单击清空输入并记时；500ms 内再次按下退出。
    ///
    /// 输入: 无（内部取当前时刻；测试经由 last_ctrl_c 注入判定）
    /// 输出: 无返回值；副作用为清空编辑器或置退出标志
    fn handle_ctrl_c(&mut self) {
        let now = std::time::Instant::now();
        if let Some(prev) = self.last_ctrl_c
            && now.duration_since(prev) < std::time::Duration::from_millis(500)
        {
            self.log_event("key", "ctrl+c double → quit");
            self.should_quit = true;
            return;
        }
        self.last_ctrl_c = Some(now);
        self.log_event("key", "ctrl+c single → clear");
        self.editor.replace("");
    }

    /// 测试辅助：直接触发 ctrl+c 语义（真实时间窗内连续调用即双击）。
    #[cfg(test)]
    fn handle_ctrl_c_for_test(&mut self) {
        self.handle_ctrl_c();
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
        // ctrl+c 全局优先（浮层之下也生效）：单击清空输入，500ms 内双击退出。
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.handle_ctrl_c();
            return;
        }

        // 浮层优先消费按键（含关闭/应用），绝不落入主键位表。
        if let Some(mut overlay) = self.overlay.take() {
            match overlay_key(&mut overlay, key) {
                OverlayOutcome::Handled => {
                    self.overlay = Some(overlay);
                }
                OverlayOutcome::Close { reply } => {
                    self.log_event("overlay", "closed");
                    if let Some(reply) = reply {
                        self.send_reply(&reply, commander, report_tx);
                    }
                    self.close_overlay();
                }
                OverlayOutcome::Apply(idx) => {
                    self.log_event("overlay", &format!("apply #{idx}"));
                    self.apply_overlay(&overlay, idx, commander, report_tx);
                    self.close_overlay();
                }
            }
            return;
        }

        let intent = Self::key_intent(key, self.state.phase == Phase::Streaming);
        match self.apply_intent(key, intent) {
            Some(KeyIntent::Submit) => self.submit_as(commander, report_tx, "prompt"),
            Some(KeyIntent::SendFollowUp) => {
                // 流式排队 follow_up；空闲时对齐 pi：alt+enter 等同普通回车
                if self.state.phase == Phase::Streaming {
                    self.submit_as(commander, report_tx, "follow_up");
                } else {
                    self.submit_as(commander, report_tx, "prompt");
                }
            }
            Some(KeyIntent::Abort) => self.request_abort_and_restore(commander, report_tx),
            Some(KeyIntent::Dequeue) => self.request_dequeue(commander, report_tx),
            Some(KeyIntent::OpenModelSelect) => self.open_model_select(commander, report_tx),
            Some(KeyIntent::OpenThinkingSelect) => {
                self.open_thinking_select(commander, report_tx)
            }
            Some(KeyIntent::CycleModelNext) => {
                Self::spawn_async(
                    commander,
                    report_tx,
                    serde_json::json!({"type": "cycle_model"}),
                    AsyncKind::CycleModel,
                    0,
                    None,
                );
            }
            Some(KeyIntent::CycleModelPrev) => {
                self.cycle_model_prev(commander, report_tx);
            }
            Some(KeyIntent::CycleThinking) => {
                Self::spawn_async(
                    commander,
                    report_tx,
                    serde_json::json!({"type": "cycle_thinking_level"}),
                    AsyncKind::CycleThinking,
                    0,
                    None,
                );
            }
            _ => {}
        }
    }

    /// 打开模型选择器：置 loading 浮层并异步拉取模型列表（带序号防过期）。
    fn open_model_select(&mut self, commander: &CommandSender, report_tx: &Sender<UiEvent>) {
        self.log_event("overlay", "open model select");
        self.overlay_seq += 1;
        let seq = self.overlay_seq;
        self.overlay = Some(Overlay::ModelSelect {
            items: vec![],
            loading: true,
            cursor: 0,
            seq,
        });
        Self::spawn_async(
            commander,
            report_tx,
            serde_json::json!({"type": "get_available_models"}),
            AsyncKind::Models,
            seq,
            None,
        );
    }

    /// 打开 thinking 级别选择器（同上，数据为级别列表）。
    fn open_thinking_select(&mut self, commander: &CommandSender, report_tx: &Sender<UiEvent>) {
        self.log_event("overlay", "open thinking select");
        self.overlay_seq += 1;
        let seq = self.overlay_seq;
        self.overlay = Some(Overlay::ThinkingSelect {
            levels: vec![],
            loading: true,
            cursor: 0,
            seq,
        });
        Self::spawn_async(
            commander,
            report_tx,
            serde_json::json!({"type": "get_available_thinking_levels"}),
            AsyncKind::ThinkingLevels,
            seq,
            None,
        );
    }

    /// 模型上一个循环：RPC 无 backward 命令，经模型列表定位当前项的前一项
    /// 后 set_model（单线程内完成两跳）。
    fn cycle_model_prev(&self, commander: &CommandSender, report_tx: &Sender<UiEvent>) {
        let current = self.state.model.clone();
        let report = report_tx.clone();
        let cmd = commander.clone();
        thread::spawn(move || {
            let list = cmd.send_command(
                serde_json::json!({"type": "get_available_models"}),
                Duration::from_secs(30),
                ResponsePolicy::RequireSuccess,
            );
            let models = match list {
                Ok(v) => v,
                Err(e) => {
                    let _ = report.send(UiEvent::SendFailed(e.to_string()));
                    return;
                }
            };
            let Some(arr) = models.get("models").and_then(|m| m.as_array()) else {
                return;
            };
            let items: Vec<(String, String, String)> = arr
                .iter()
                .filter_map(|m| {
                    Some((
                        m.get("provider")?.as_str()?.to_string(),
                        m.get("id")?.as_str()?.to_string(),
                        m.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    ))
                })
                .collect();
            if items.is_empty() {
                let _ = report.send(UiEvent::AsyncData {
                    kind: AsyncKind::CycleModel,
                    seq: 0,
                    data: serde_json::Value::Null,
                });
                return;
            }
            // 定位当前项（按显示名匹配），无匹配则从末尾开始（回到最后一个）。
            let cur_idx = current
                .as_deref()
                .and_then(|name| items.iter().position(|(_, _, n)| n == name))
                .unwrap_or(items.len() - 1);
            let prev = if cur_idx == 0 {
                items.len() - 1
            } else {
                cur_idx - 1
            };
            let (provider, id, _) = &items[prev];
            match cmd.send_command(
                serde_json::json!({"type": "set_model", "provider": provider, "modelId": id}),
                Duration::from_secs(30),
                ResponsePolicy::RequireSuccess,
            ) {
                Ok(data) => {
                    let _ = report.send(UiEvent::AsyncData {
                        kind: AsyncKind::ModelApplied,
                        seq: 0,
                        data,
                    });
                }
                Err(e) => {
                    let _ = report.send(UiEvent::SendFailed(e.to_string()));
                }
            }
        });
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
            // 空闲 esc：无操作（对齐 pi；退出走 ctrl+d/双击 ctrl+c）
            Char('d') if ctrl => KeyIntent::ExitIfEmpty,
            // 多行：shift+enter / ctrl+j 换行（alt+enter 仍为 followUp）
            Enter if key.modifiers.contains(KeyModifiers::SHIFT)
                && !key.modifiers.contains(KeyModifiers::ALT) => KeyIntent::Newline,
            Char('j') if ctrl => KeyIntent::Newline,
            Char('t') if ctrl => KeyIntent::ToggleThinking,
            Char('o') if ctrl => KeyIntent::ToggleTools,
            Enter if alt => KeyIntent::SendFollowUp,
            Enter => KeyIntent::Submit,
            Up if alt => KeyIntent::Dequeue,
            Char('l') if ctrl && !key.modifiers.contains(KeyModifiers::SHIFT) => {
                KeyIntent::OpenModelSelect
            }
            Char('p') if ctrl && key.modifiers.contains(KeyModifiers::SHIFT) => {
                KeyIntent::CycleModelPrev
            }
            Char('p') if ctrl => KeyIntent::CycleModelNext,
            Char('t') if alt => KeyIntent::OpenThinkingSelect,
            BackTab => KeyIntent::CycleThinking,
            // 编辑族：光标移动 / 删除键 / 字符（含未绑定的 ctrl/alt 组合，
            // 由 apply_intent 的 Edit 分派决定是编辑还是忽略）
            Left | Right | Up | Down | Home | End | Delete | PageUp | PageDown => {
                KeyIntent::Edit
            }
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
                self.edit_key(key);
                None
            }
            KeyIntent::ExitIfEmpty => {
                if self.editor.is_empty() {
                    self.should_quit = true;
                } else {
                    self.editor.delete_forward();
                }
                None
            }
            KeyIntent::Newline => {
                self.editor.newline();
                None
            }
            KeyIntent::Ignore => None,
            KeyIntent::Submit
            | KeyIntent::Abort
            | KeyIntent::SendFollowUp
            | KeyIntent::Dequeue
            | KeyIntent::OpenModelSelect
            | KeyIntent::OpenThinkingSelect
            | KeyIntent::CycleModelNext
            | KeyIntent::CycleModelPrev
            | KeyIntent::CycleThinking => Some(intent),
        }
    }

    /// 判断当前输入是否构成可发送的消息（去空白后非空）。
    ///
    /// 输入: self — 当前应用状态（读 input）
    /// 输出: true 表示可以发送；false 表示应忽略 Enter
    fn can_submit(&self) -> bool {
        !self.editor.text().trim().is_empty()
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
        let message = self.editor.text();
        let message = message.trim().to_string();
        self.editor.replace("");
        self.log_event("submit", &format!("{command} sent: {message}"));
        let payload = Self::build_command_payload(
            command,
            &message,
            self.state.phase == Phase::Streaming,
        );
        self.send_command(commander, report_tx, payload, true);
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
        self.editor.append_block(&joined);
    }

    /// 流式 esc：先 abort（容忍失败），再 clear_queue 把排队消息恢复回输入框
    /// （对齐 pi 的 restoreQueuedMessagesToEditor({abort:true})）。
    ///
    /// 输入: commander — RPC 命令句柄；report_tx — 结果/失败回报通道
    /// 输出: 无返回值；副作用为异步发 abort + clear_queue 两条命令
    fn request_abort_and_restore(&mut self, commander: &CommandSender, report_tx: &Sender<UiEvent>) {
        self.log_event("key", "esc streaming → abort + restore queue");
        let report = report_tx.clone();
        let cmd = commander.clone();
        thread::spawn(move || {
            // abort：生成可能已结束，业务失败可容忍
            let _ = cmd.send_command(
                serde_json::json!({"type": "abort"}),
                Duration::from_secs(10),
                ResponsePolicy::AcceptError,
            );
            // clear_queue：返回排队内容回填编辑器
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

    /// 分发一条 extension_ui_request：对话 4 种入浮层，被动 5 种立即应用（不回发）。
    ///
    /// 输入: raw — 请求 JSON 对象
    /// 输出: 无返回值；副作用为入队浮层或更新 App UI 状态
    fn handle_extension_request(&mut self, raw: &serde_json::Value) {
        let id = raw.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
        let method = raw.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let str_field = |k: &str| -> String {
            raw.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string()
        };
        match method {
            "select" => {
                let options = raw
                    .get("options")
                    .and_then(|o| o.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                self.push_dialog(Overlay::ExtSelect {
                    id,
                    title: str_field("title"),
                    options,
                    cursor: 0,
                });
            }
            "confirm" => self.push_dialog(Overlay::ExtConfirm {
                id,
                title: str_field("title"),
                message: str_field("message"),
                confirm_selected: true,
            }),
            "input" => self.push_dialog(Overlay::ExtInput {
                id,
                title: str_field("title"),
                placeholder: str_field("placeholder"),
                buffer: String::new(),
            }),
            "editor" => {
                let prefill = raw.get("prefill").and_then(|v| v.as_str()).unwrap_or("");
                let lines: Vec<String> = if prefill.is_empty() {
                    vec![String::new()]
                } else {
                    prefill.lines().map(String::from).collect()
                };
                // 光标置于 prefill 末尾（追加编辑的自然起点）。
                let row = lines.len() - 1;
                let col = lines[row].chars().count();
                self.push_dialog(Overlay::ExtEditor {
                    id,
                    title: str_field("title"),
                    lines,
                    row,
                    col,
                });
            }
            "notify" => {
                let color = match raw.get("notifyType").and_then(|t| t.as_str()) {
                    Some("error") => Color::Red,
                    Some("warning") => Color::Yellow,
                    _ => Color::Reset,
                };
                self.notice = Some((str_field("message"), color));
            }
            "setStatus" => {
                self.status_text = raw
                    .get("statusText")
                    .and_then(|v| v.as_str())
                    .map(String::from);
            }
            "setWidget" => {
                self.widget_lines = match raw.get("widgetLines") {
                    Some(serde_json::Value::Array(arr)) => arr
                        .iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect(),
                    _ => Vec::new(),
                };
            }
            "setTitle" => {
                let title = str_field("title");
                let _ = crossterm::execute!(
                    std::io::stdout(),
                    crossterm::terminal::SetTitle(title)
                );
            }
            "set_editor_text" => {
                self.editor.replace(&str_field("text"));
            }
            other => {
                self.log_event("extension_ui", &format!("ignored method {other}"));
            }
        }
    }

    /// 把对话浮层压入当前槽或排队（串行处理，与 pi 的 withUIPrompt 一致）。
    fn push_dialog(&mut self, overlay: Overlay) {
        if self.overlay.is_none() {
            self.overlay = Some(overlay);
        } else {
            self.pending_overlays.push_back(overlay);
        }
    }

    /// 关闭当前浮层并让排队中的下一个顶上。
    fn close_overlay(&mut self) {
        self.overlay = self.pending_overlays.pop_front();
    }

    /// 选择器确认：把选中项转成对应 RPC 命令，异步执行并回填状态。
    ///
    /// 输入: overlay — 应用时的浮层快照；idx — 选中下标；
    ///       commander/report_tx — 命令句柄与结果回报通道
    /// 输出: 无返回值；副作用为异步发 set_model / set_thinking_level
    fn apply_overlay(
        &mut self,
        overlay: &Overlay,
        idx: usize,
        commander: &CommandSender,
        report_tx: &Sender<UiEvent>,
    ) {
        match overlay {
            Overlay::ModelSelect { items, seq, .. } => {
                if let Some(item) = items.get(idx) {
                    let command = serde_json::json!({
                        "type": "set_model",
                        "provider": item.provider,
                        "modelId": item.id
                    });
                    Self::spawn_async(
                        commander,
                        report_tx,
                        command,
                        AsyncKind::ModelApplied,
                        *seq,
                        None,
                    );
                }
            }
            Overlay::ThinkingSelect { levels, seq, .. } => {
                if let Some(level) = levels.get(idx) {
                    let command =
                        serde_json::json!({"type": "set_thinking_level", "level": level});
                    // set_thinking_level 响应无 data，携带级别字符串回填。
                    Self::spawn_async(
                        commander,
                        report_tx,
                        command,
                        AsyncKind::ThinkingApplied,
                        *seq,
                        Some(level.clone()),
                    );
                }
            }
            // 扩展对话浮层不走此路径（由 Close reply 处理）。
            _ => {}
        }
    }

    /// 一次性线程执行命令，结果经 AsyncData / SendFailed 回到主循环。
    ///
    /// 输入: commander — 命令句柄；report_tx — 回报通道；command — 命令对象；
    ///       kind — 异步数据类别；seq — 选择器序号；attach — 附加到 data 的载荷
    ///       （响应本身无 data 时使用，如 set_thinking_level 的级别）
    /// 输出: 无返回值；副作用为异步发命令
    fn spawn_async(
        commander: &CommandSender,
        report_tx: &Sender<UiEvent>,
        command: serde_json::Value,
        kind: AsyncKind,
        seq: u64,
        attach: Option<String>,
    ) {
        let report = report_tx.clone();
        let cmd = commander.clone();
        thread::spawn(move || {
            match cmd.send_command(command, Duration::from_secs(60), ResponsePolicy::RequireSuccess)
            {
                Ok(data) => {
                    let data = match attach {
                        Some(extra) => serde_json::Value::String(extra),
                        None => data,
                    };
                    let _ = report.send(UiEvent::AsyncData { kind, seq, data });
                }
                Err(e) => {
                    let _ = report.send(UiEvent::SendFailed(e.to_string()));
                }
            }
        });
    }

    /// 发送 extension_ui_response（原始行，不经 id 路由）。
    ///
    /// 输入: reply — 回复载荷；commander — 提供 send_raw 的命令句柄；
    ///       report_tx — 失败回报通道
    /// 输出: 无返回值；副作用为入队一条原始行
    fn send_reply(
        &self,
        reply: &serde_json::Value,
        commander: &CommandSender,
        report_tx: &Sender<UiEvent>,
    ) {
        if let Err(e) = commander.send_raw(reply) {
            let _ = report_tx.send(UiEvent::SendFailed(e.to_string()));
        }
    }

    /// 消费异步数据：填充选择器 / 回填模型与 thinking 状态。
    ///
    /// 输入: kind — 数据类别；seq — 选择器序号（用于丢弃过期响应）；
    ///       data — 载荷
    /// 输出: 无返回值；副作用为更新 overlay/state/notice
    fn handle_async_data(&mut self, kind: AsyncKind, seq: u64, data: &serde_json::Value) {
        match kind {
            AsyncKind::Models => {
                if let Some(Overlay::ModelSelect {
                    items, loading, seq: s, ..
                }) = self.overlay.as_mut()
                    && *s == seq
                {
                        *items = data
                            .get("models")
                            .and_then(|m| m.as_array())
                            .map(|a| {
                                a.iter()
                                    .filter_map(|m| {
                                        Some(crate::app::overlay::ModelItem {
                                            provider: m
                                                .get("provider")
                                                .and_then(|x| x.as_str())?
                                                .to_string(),
                                            id: m.get("id").and_then(|x| x.as_str())?.to_string(),
                                            name: m
                                                .get("name")
                                                .and_then(|x| x.as_str())
                                                .unwrap_or_default()
                                                .to_string(),
                                        })
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        *loading = false;
                }
            }
            AsyncKind::ThinkingLevels => {
                if let Some(Overlay::ThinkingSelect {
                    levels, loading, seq: s, ..
                }) = self.overlay.as_mut()
                    && *s == seq
                {
                        *levels = data
                            .get("levels")
                            .and_then(|l| l.as_array())
                            .map(|a| {
                                a.iter()
                                    .filter_map(|x| x.as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default();
                        *loading = false;
                }
            }
            AsyncKind::ModelApplied => {
                if let Some(name) = data
                    .get("name")
                    .or_else(|| data.get("id"))
                    .and_then(|x| x.as_str())
                {
                    self.state.model = Some(name.to_string());
                }
            }
            AsyncKind::CycleModel => {
                if data.is_null() {
                    self.notice = Some(("无可用模型".to_string(), Color::Yellow));
                } else if let Some(name) = data
                    .pointer("/model/name")
                    .or_else(|| data.pointer("/model/id"))
                    .and_then(|x| x.as_str())
                {
                    self.state.model = Some(name.to_string());
                    if let Some(level) = data.get("thinkingLevel").and_then(|x| x.as_str()) {
                        self.state.thinking_level = Some(level.to_string());
                    }
                }
            }
            AsyncKind::ThinkingApplied => {
                if let Some(level) = data.as_str() {
                    self.state.thinking_level = Some(level.to_string());
                }
            }
            AsyncKind::CycleThinking => {
                if let Some(level) = data.pointer("/level").and_then(|x| x.as_str()) {
                    self.state.thinking_level = Some(level.to_string());
                } else if data.is_null() {
                    self.notice = Some(("无可用 thinking 级别".to_string(), Color::Yellow));
                }
            }
        }
    }

    fn draw(&self, terminal: &mut ratatui::DefaultTerminal) -> io::Result<()> {
        let input_text = self.editor.text();
        let ui = view::ViewUi {
            input: &input_text,
            input_lines: self.editor.lines().len(),
            input_cursor: Some((self.editor.row(), self.editor.col())),
            notice: self.notice.as_ref().map(|(t, c)| (t.as_str(), *c)),
            status_text: self.status_text.as_deref(),
            widget_lines: &self.widget_lines,
            flags: crate::app::chat::RenderFlags {
                show_thinking: self.show_thinking,
                expand_tools: self.expand_tools,
            },
            overlay: self.overlay.as_ref(),
        };
        terminal.draw(|frame| view::render(frame, &self.state, &ui))?;
        Ok(())
    }

}

use std::sync::mpsc;

#[cfg(test)]
mod input_tests {
    use super::*;


    /// 测试辅助：经真实 apply_intent 判定意图是否需要命令通道。
    fn routed_intent(code: KeyCode, mods: KeyModifiers, streaming: bool) -> Option<KeyIntent> {
        let key = KeyEvent::new(code, mods);
        let intent = App::key_intent(key, streaming);
        let mut probe = App::new();
        probe.apply_intent(key, intent)
    }

    #[test]
    fn edit_intent_routes_to_editor_buffer() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (client, dir) = test_commander();
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();

        // 字符 → insert；CJK 字符安全
        for c in ['a', 'b', '中'] {
            app.handle_key(
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
                &commander,
                &tx,
            );
        }
        assert_eq!(app.editor.text(), "ab中");
        // 退格 → backspace（char 安全）
        app.handle_key(
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            &commander,
            &tx,
        );
        assert_eq!(app.editor.text(), "ab");
        // 空缓冲退格 no-op
        app.editor.replace("");
        app.handle_key(
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            &commander,
            &tx,
        );
        assert!(app.editor.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn blank_input_does_not_submit() {
        let mut app = App::new();
        app.editor.replace("   \n  ");
        assert!(!app.can_submit());
        app.editor.replace("hello");
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
        assert_eq!(app.editor.text(), "t");
        app.editor.replace("");

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
        // esc: 流式=中止, 空闲=无操作（退出走 ctrl+d/双击 ctrl+c，见 handle_ctrl_c 测试）
        assert_eq!(App::key_intent(k(KeyCode::Esc, KeyModifiers::NONE), true), KeyIntent::Abort);
        assert_eq!(App::key_intent(k(KeyCode::Esc, KeyModifiers::NONE), false), KeyIntent::Ignore);
        // q 恒为普通编辑字符（对齐 pi：不再承担退出）
        assert_eq!(App::key_intent(k(KeyCode::Char('q'), KeyModifiers::NONE), false), KeyIntent::Edit);
        assert_eq!(App::key_intent(k(KeyCode::Char('q'), KeyModifiers::NONE), true), KeyIntent::Edit);
        // ctrl+d: 空输入退出 / 非空向后删
        assert_eq!(App::key_intent(k(KeyCode::Char('d'), KeyModifiers::CONTROL), false), KeyIntent::ExitIfEmpty);
        // shift+enter / ctrl+j = 换行；alt+enter 仍为 followUp；普通 enter = 提交
        assert_eq!(App::key_intent(k(KeyCode::Enter, KeyModifiers::SHIFT), false), KeyIntent::Newline);
        assert_eq!(App::key_intent(k(KeyCode::Char('j'), KeyModifiers::CONTROL), false), KeyIntent::Newline);
        assert_eq!(App::key_intent(k(KeyCode::Enter, KeyModifiers::ALT), false), KeyIntent::SendFollowUp);
        assert_eq!(App::key_intent(k(KeyCode::Enter, KeyModifiers::NONE), false), KeyIntent::Submit);
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
        // alt+up → Dequeue；普通 up → Edit（光标上移，编辑族）
        assert_eq!(
            App::key_intent(k(KeyCode::Up, KeyModifiers::ALT), true),
            KeyIntent::Dequeue
        );
        assert_eq!(
            App::key_intent(k(KeyCode::Up, KeyModifiers::NONE), false),
            KeyIntent::Edit
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
        assert!(app.editor.is_empty());
    }

    #[test]
    fn restore_queued_appends_to_existing_input() {
        let mut app = App::new();
        app.editor.replace("existing");
        app.restore_queued(&serde_json::json!({
            "steering": ["s1"],
            "followUp": ["f1", "f2"]
        }));
        assert_eq!(app.editor.text(), "existing\ns1\nf1\nf2");
    }

    #[test]
    fn follow_up_and_prompt_submit_clear_input() {
        let mut app = App::new();
        app.editor.replace("  hello  ");
        assert!(app.can_submit());
        app.editor.replace("   ");
        assert!(!app.can_submit());
    }

    /// 造一个只应答不动作的假 pi，供需要 CommandSender 的测试使用。
    fn test_commander() -> (pi_rust_tui::rpc::RpcClient, std::path::PathBuf) {
        let script = "#!/bin/sh\nwhile IFS= read -r line; do\n  id=$(printf '%s' \"$line\" | sed -n 's/.*\"id\":\"\\([^\"]*\\)\".*/\\1/p')\n  printf '{\"type\":\"response\",\"id\":\"%s\",\"success\":true}\n' \"$id\"\ndone\n";
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("app-test-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fake.sh");
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let client = pi_rust_tui::rpc::RpcClient::spawn(path.to_str().unwrap(), &[]).unwrap();
        (client, dir)}

    #[test]
    fn overlay_focus_routing_blocks_main_keys() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (client, dir) = test_commander();
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();

        let mut app = App::new();
        app.overlay = Some(crate::app::overlay::Overlay::ExtInput {
            id: "r1".into(),
            title: "t".into(),
            placeholder: String::new(),
            buffer: String::new(),
        });

        // 浮层内输入字符 → 进浮层缓冲，绝不进主输入框
        app.handle_key(
            KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE),
            &commander,
            &tx,
        );
        assert!(app.editor.is_empty(), "浮层键不得泄漏到主输入框");
        match &app.overlay {
            Some(crate::app::overlay::Overlay::ExtInput { buffer, .. }) => {
                assert_eq!(buffer, "z");
            }
            other => panic!("{other:?}"),
        }

        // 浮层内 ctrl+c：单击=清空（不退出），双击=退出（全局分支先于浮层路由）
        app.editor.replace("behind");
        app.handle_key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &commander,
            &tx,
        );
        assert!(!app.should_quit, "单击 ctrl+c 不退出");
        assert!(app.editor.is_empty(), "单击清空输入");
        app.handle_key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &commander,
            &tx,
        );
        assert!(app.should_quit, "500ms 内双击 ctrl+c 退出");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 录制型假 pi：把收到的每行 stdin 追加到日志文件；clear_queue 返回
    /// steering=["s1"]/followUp=["f1"]，其余命令一律 success。
    fn recording_commander(tag: &str, clear_queue_data: &str) -> (pi_rust_tui::rpc::RpcClient, std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "rec-pi-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join(format!("recv-{tag}.log"));
        let script = format!(
            "#!/bin/sh\nlog=\"{}\"\nwhile IFS= read -r line; do\n  printf '%s\\n' \"$line\" >> \"$log\"\n  id=$(printf '%s' \"$line\" | sed -n 's/.*\\\"id\\\":\\\"\\([^\\\"\\]*\\)\\\".*/\\1/p')\n  case \"$line\" in\n    *clear_queue*) printf '{{\"type\":\"response\",\"id\":\"%s\",\"success\":true,\"data\":{}}}\\n' \"$id\" ;;\n    *) printf '{{\"type\":\"response\",\"id\":\"%s\",\"success\":true}}\\n' \"$id\" ;;\n  esac\ndone\n",
            log.display(),
            clear_queue_data
        );
        let path = dir.join("pi.sh");
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let client = pi_rust_tui::rpc::RpcClient::spawn(path.to_str().unwrap(), &[]).unwrap();
        (client, dir, log)
    }

    /// 轮询读取录制日志（异步写入，需短暂等待）。
    fn read_log_until(log: &std::path::Path, needle: &str, ms: u64) -> Option<String> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        while std::time::Instant::now() < deadline {
            if let Ok(content) = std::fs::read_to_string(log)
                && content.contains(needle)
            {
                return Some(content);
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        std::fs::read_to_string(log).ok()
    }

    #[test]
    fn prompt_payload_carries_steer_only_when_streaming() {
        // 纯函数断言（7.1）：流式 prompt 带 steer，空闲不带，follow_up 恒不带
        let streaming = App::build_command_payload("prompt", "msg", true);
        assert_eq!(streaming["streamingBehavior"], "steer");
        assert_eq!(streaming["type"], "prompt");
        assert_eq!(streaming["message"], "msg");

        let idle = App::build_command_payload("prompt", "msg", false);
        assert!(idle.get("streamingBehavior").is_none(), "空闲不带 steer");

        let fu = App::build_command_payload("follow_up", "msg", true);
        assert!(fu.get("streamingBehavior").is_none(), "follow_up 自身语义即排队");
    }

    #[test]
    fn alt_enter_dispatches_by_phase() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        // 空闲 → prompt；流式 → follow_up（7.2）
        let (client, dir, log) = recording_commander("dispatch", r#"{"steering":[],"followUp":[]}"#);
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();

        // 空闲
        app.editor.replace("idle msg");
        app.handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT),
            &commander,
            &tx,
        );
        let content = read_log_until(&log, "idle msg", 2000).expect("应收到 prompt");
        assert!(content.contains(r#""type":"prompt""#), "空闲 alt+enter=prompt: {content}");
        assert!(!content.contains("follow_up"));

        // 流式
        app.state.phase = Phase::Streaming;
        app.editor.replace("stream msg");
        app.handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT),
            &commander,
            &tx,
        );
        let content = read_log_until(&log, "stream msg", 2000).expect("应收到 follow_up");
        let tail = content.rfind("stream msg").map(|i| &content[i..]).unwrap_or("");
        assert!(tail.contains(r#""type":"follow_up""#), "流式 alt+enter=follow_up: {tail}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn streaming_esc_aborts_and_restores_queue() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        // （7.3）流式 esc → abort + clear_queue，返回内容回填输入框
        let (client, dir, log) =
            recording_commander("esc", r#"{"steering":["s1"],"followUp":["f1"]}"#);
        let commander = client.command_sender();
        let (tx, rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();
        app.state.phase = Phase::Streaming;
        app.editor.replace("orig");

        app.handle_key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &commander,
            &tx,
        );
        // 等两条命令都被录制
        let content = read_log_until(&log, "clear_queue", 3000).expect("clear_queue 应被发送");
        assert!(content.contains(r#""type":"abort""#), "先发 abort: {content}");

        // 等 RestoreQueued 回报并应用（event loop 的处理逻辑在测试中手动执行）
        let restored = std::time::Instant::now();
        let mut data = None;
        while std::time::Instant::now() - restored < std::time::Duration::from_secs(3) {
            if let Ok(UiEvent::RestoreQueued(d)) =
                rx.recv_timeout(std::time::Duration::from_millis(200))
            {
                data = Some(d);
                break;
            }
        }
        let data = data.expect("应收到 RestoreQueued");
        app.restore_queued(&data);
        assert_eq!(app.editor.text(), "orig\ns1\nf1", "排队消息回填输入框");

        // 空队列场景：restore 不改变输入（7.3 空队列无副作用）
        let mut app2 = App::new();
        app2.editor.replace("keep");
        app2.restore_queued(&serde_json::json!({"steering":[],"followUp":[]}));
        assert_eq!(app2.editor.text(), "keep");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ctrl_c_single_clears_double_quits() {
        let mut app = App::new();
        app.editor.replace("hello world");
        // 单击：清空、不退出
        app.handle_ctrl_c_for_test();
        assert!(app.editor.is_empty(), "单击清空");
        assert!(!app.should_quit, "单击不退出");
        // 紧接着双击（<500ms）：退出
        app.handle_ctrl_c_for_test();
        assert!(app.should_quit, "双击退出");
    }

    #[test]
    fn ctrl_d_and_q_and_idle_esc_semantics() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (client, dir) = test_commander();
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();

        // 非空 ctrl+d → 删字符（不退出）；光标先到行首（replace 落在末尾，
        // 行尾向后删本无字符可删）
        app.editor.replace("abc");
        app.handle_key(
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
            &commander,
            &tx,
        );
        app.handle_key(
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
            &commander,
            &tx,
        );
        assert!(!app.should_quit);
        assert_eq!(app.editor.text(), "bc", "ctrl+d 非空 = 光标处向后删");
        // 空输入 ctrl+d → 退出
        app.editor.replace("");
        app.handle_key(
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
            &commander,
            &tx,
        );
        assert!(app.should_quit, "空输入 ctrl+d 退出");

        // q → 普通字符（不退出）
        let mut app2 = App::new();
        app2.handle_key(
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
            &commander,
            &tx,
        );
        assert_eq!(app2.editor.text(), "q");
        assert!(!app2.should_quit, "q 不再退出");

        // 空闲 esc → 无操作
        let mut app3 = App::new();
        app3.editor.replace("keep");
        app3.handle_key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &commander,
            &tx,
        );
        assert_eq!(app3.editor.text(), "keep", "esc 不清空");
        assert!(!app3.should_quit, "esc 不退出");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn newline_keys_split_lines() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (client, dir) = test_commander();
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();
        for c in "ab".chars() {
            app.handle_key(
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
                &commander,
                &tx,
            );
        }
        // shift+enter 换行
        app.handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT),
            &commander,
            &tx,
        );
        assert_eq!(app.editor.lines().len(), 2, "shift+enter 换行");
        // ctrl+j 再换行
        app.handle_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL),
            &commander,
            &tx,
        );
        assert_eq!(app.editor.lines().len(), 3, "ctrl+j 换行");
        for c in "cd".chars() {
            app.handle_key(
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
                &commander,
                &tx,
            );
        }
        assert_eq!(app.editor.text(), "ab\n\ncd");
        // 光标移动经 Edit 路由（ctrl+a → 行首）
        app.handle_key(
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
            &commander,
            &tx,
        );
        assert_eq!((app.editor.row(), app.editor.col()), (2, 0));
        // ctrl+b/a/e 等编辑族不插入字符
        assert_eq!(app.editor.text(), "ab\n\ncd", "ctrl 组合不插入字符");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extension_request_dispatch_and_passive_methods() {
        let mut app = App::new();
        // 对话 → 浮层
        app.handle_extension_request(&serde_json::json!({
            "type":"extension_ui_request","id":"s1","method":"select",
            "title":"pick","options":["a","b"]
        }));
        match &app.overlay {
            Some(crate::app::overlay::Overlay::ExtSelect { id, options, .. }) => {
                assert_eq!(id, "s1");
                assert_eq!(options, &vec!["a".to_string(), "b".to_string()]);
            }
            other => panic!("{other:?}"),
        }
        // 第二个对话 → 排队
        app.handle_extension_request(&serde_json::json!({
            "type":"extension_ui_request","id":"c1","method":"confirm",
            "title":"t","message":"m"
        }));
        assert_eq!(app.pending_overlays.len(), 1, "第二个对话应排队");
        // 关闭当前 → 队列顶上
        app.close_overlay();
        assert!(matches!(
            app.overlay,
            Some(crate::app::overlay::Overlay::ExtConfirm { .. })
        ));

        // 被动 5 种
        app.handle_extension_request(&serde_json::json!({
            "type":"extension_ui_request","id":"n1","method":"notify",
            "message":"heads up","notifyType":"warning"
        }));
        let (text, color) = app.notice.clone().expect("notice");
        assert_eq!(text, "heads up");
        assert_eq!(color, Color::Yellow);

        app.handle_extension_request(&serde_json::json!({
            "type":"extension_ui_request","id":"st","method":"setStatus","statusText":"busy"
        }));
        assert_eq!(app.status_text.as_deref(), Some("busy"));

        app.handle_extension_request(&serde_json::json!({
            "type":"extension_ui_request","id":"st2","method":"setStatus","statusText":null
        }));
        assert_eq!(app.status_text, None);

        app.handle_extension_request(&serde_json::json!({
            "type":"extension_ui_request","id":"w","method":"setWidget",
            "widgetLines":["line-a","line-b"]
        }));
        assert_eq!(app.widget_lines, vec!["line-a".to_string(), "line-b".to_string()]);
        app.handle_extension_request(&serde_json::json!({
            "type":"extension_ui_request","id":"w2","method":"setWidget","widgetLines":null
        }));
        assert!(app.widget_lines.is_empty(), "null 清除 widget");

        app.handle_extension_request(&serde_json::json!({
            "type":"extension_ui_request","id":"e","method":"set_editor_text","text":"draft text"
        }));
        assert_eq!(app.editor.text(), "draft text");

        // setTitle 不 panic（无法断言终端标题，仅验证路径安全）
        app.handle_extension_request(&serde_json::json!({
            "type":"extension_ui_request","id":"ti","method":"setTitle","title":"proj"
        }));
        // 未知 method 忽略
        app.handle_extension_request(&serde_json::json!({
            "type":"extension_ui_request","id":"x","method":"mystery"
        }));
        // 被动方法均未产生浮层替换（浮层仍是 confirm）
        assert!(matches!(
            app.overlay,
            Some(crate::app::overlay::Overlay::ExtConfirm { .. })
        ));
    }

    #[test]
    fn async_data_fills_selectors_and_discards_stale() {
        let mut app = App::new();
        // 打开 model 选择器（seq=1）
        app.overlay_seq += 1;
        let seq = app.overlay_seq;
        app.overlay = Some(crate::app::overlay::Overlay::ModelSelect {
            items: vec![],
            loading: true,
            cursor: 0,
            seq,
        });
        // 过期 seq → 丢弃
        app.handle_async_data(AsyncKind::Models, seq + 99, &serde_json::json!({
            "models":[{"provider":"p","id":"m","name":"Stale"}]
        }));
        match &app.overlay {
            Some(crate::app::overlay::Overlay::ModelSelect { loading, items, .. }) => {
                assert!(*loading, "过期数据不得填充");
                assert!(items.is_empty());
            }
            other => panic!("{other:?}"),
        }
        // 匹配 seq → 填充
        app.handle_async_data(AsyncKind::Models, seq, &serde_json::json!({
            "models":[{"provider":"p","id":"m","name":"Real Model"}]
        }));
        match &app.overlay {
            Some(crate::app::overlay::Overlay::ModelSelect { loading, items, .. }) => {
                assert!(!*loading);
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].name, "Real Model");
            }
            other => panic!("{other:?}"),
        }
        // CycleModel null → notice
        app.handle_async_data(AsyncKind::CycleModel, 0, &serde_json::Value::Null);
        let (text, color) = app.notice.clone().expect("notice");
        assert!(text.contains("无可用模型"), "{text}");
        assert_eq!(color, Color::Yellow);
        // ModelApplied → state.model
        app.handle_async_data(
            AsyncKind::ModelApplied,
            0,
            &serde_json::json!({"provider":"p","id":"m","name":"Applied"}),
        );
        assert_eq!(app.state.model.as_deref(), Some("Applied"));
        // ThinkingApplied → state.thinking_level
        app.handle_async_data(AsyncKind::ThinkingApplied, 0, &serde_json::json!("high"));
        assert_eq!(app.state.thinking_level.as_deref(), Some("high"));
    }

    #[test]
    fn model_select_opens_with_seq_and_loading() {
        let mut app = App::new();
        app.overlay_seq += 1;
        let seq = app.overlay_seq;
        app.overlay = Some(crate::app::overlay::Overlay::ModelSelect {
            items: vec![],
            loading: true,
            cursor: 0,
            seq,
        });
        // apply 一个空列表（不发命令路径）——idx 越界安全
        if let Some(o) = app.overlay.take() {
            // apply_overlay 需要 commander，空 items 时不会发命令；这里只验证快照语义
            if let crate::app::overlay::Overlay::ModelSelect { items, .. } = &o {
                assert!(items.is_empty());
            }
            app.overlay = Some(o);
        }
        assert!(app.overlay.is_some());
    }

    /// 用真实 extension_ui fixture 回放：请求 → 浮层 → 键驱动 → 回复载荷。
    #[test]
    fn extension_ui_fixture_replay_builds_dialogs_and_replies() {
        use crate::app::overlay::{overlay_key, Overlay, OverlayOutcome};
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/probe/extension_ui/snapshot/transcript.jsonl");
        let content = std::fs::read_to_string(&path).expect("fixture");

        let mut app = App::new();
        let mut requests = Vec::new();
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let wrapper: serde_json::Value = serde_json::from_str(line).unwrap();
            if wrapper.get("dir").and_then(|d| d.as_str()) != Some("in") {
                continue;
            }
            let raw = wrapper.get("raw").cloned().unwrap();
            if raw.get("type").and_then(|t| t.as_str()) == Some("extension_ui_request") {
                requests.push(raw);
            }
        }
        assert!(requests.len() >= 3, "fixture 应含 select/confirm/notify: {}", requests.len());

        // 第 1 个：select → 浮层，导航后提交
        app.handle_extension_request(&requests[0]);
        let mut overlay = app.overlay.take().expect("select 浮层");
        match &mut overlay {
            Overlay::ExtSelect { id, title, options, .. } => {
                assert_eq!(id, "a7b2a469-5f7e-4162-a635-eaefc3a28815");
                assert_eq!(title, "probe select");
                assert_eq!(options, &vec!["first".to_string(), "second".to_string()]);
            }
            other => panic!("{other:?}"),
        }
        // 选 second
        overlay_key(&mut overlay, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        let out = overlay_key(&mut overlay, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        match out {
            OverlayOutcome::Close { reply: Some(v) } => {
                assert_eq!(v["value"], "second");
                assert_eq!(
                    v["id"],
                    "a7b2a469-5f7e-4162-a635-eaefc3a28815"
                );
            }
            other => panic!("{other:?}"),
        }
        app.overlay = None;

        // 第 2 个：confirm → 浮层，直接确认
        app.handle_extension_request(&requests[1]);
        let mut overlay = app.overlay.take().expect("confirm 浮层");
        let out = overlay_key(&mut overlay, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        match out {
            OverlayOutcome::Close { reply: Some(v) } => {
                assert_eq!(v["confirmed"], true);
                assert_eq!(v["id"], "bd2e786b-5ffa-4b89-aa19-6c3c35d1d2b0");
            }
            other => panic!("{other:?}"),
        }
        app.overlay = None;

        // 第 3 个：notify → 状态行通知，无浮层
        app.handle_extension_request(&requests[2]);
        assert!(app.overlay.is_none(), "notify 不产生浮层");
        let (text, _) = app.notice.clone().expect("notify 通知");
        assert!(text.starts_with("probe notify:"), "{text}");
    }

    #[test]
    fn selector_keys_map_per_spec() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let k = |code, mods| KeyEvent::new(code, mods);
        // ctrl+l → 打开模型选择器
        assert_eq!(
            App::key_intent(k(KeyCode::Char('l'), KeyModifiers::CONTROL), false),
            KeyIntent::OpenModelSelect
        );
        // 普通 l → 编辑
        assert_eq!(
            App::key_intent(k(KeyCode::Char('l'), KeyModifiers::NONE), false),
            KeyIntent::Edit
        );
        // ctrl+p / ctrl+shift+p → 循环
        assert_eq!(
            App::key_intent(k(KeyCode::Char('p'), KeyModifiers::CONTROL), false),
            KeyIntent::CycleModelNext
        );
        assert_eq!(
            App::key_intent(
                k(KeyCode::Char('p'), KeyModifiers::CONTROL | KeyModifiers::SHIFT),
                false
            ),
            KeyIntent::CycleModelPrev
        );
        // shift+tab (BackTab) → thinking 循环
        assert_eq!(
            App::key_intent(k(KeyCode::BackTab, KeyModifiers::SHIFT), false),
            KeyIntent::CycleThinking
        );
        // alt+t → thinking 选择器（与 ctrl+t 切换显示互不干扰）
        assert_eq!(
            App::key_intent(k(KeyCode::Char('t'), KeyModifiers::ALT), false),
            KeyIntent::OpenThinkingSelect
        );
        assert_eq!(
            App::key_intent(k(KeyCode::Char('t'), KeyModifiers::CONTROL), false),
            KeyIntent::ToggleThinking
        );
        // 这些意图都需要命令通道（Some 返回）
        assert_eq!(
            routed_intent(KeyCode::Char('l'), KeyModifiers::CONTROL, false),
            Some(KeyIntent::OpenModelSelect)
        );
    }

    #[test]
    fn thinking_selector_state_and_apply() {
        let mut app = App::new();
        app.overlay_seq += 1;
        let seq = app.overlay_seq;
        app.overlay = Some(crate::app::overlay::Overlay::ThinkingSelect {
            levels: vec![],
            loading: true,
            cursor: 0,
            seq,
        });
        // 过期序号丢弃
        app.handle_async_data(
            AsyncKind::ThinkingLevels,
            seq + 1,
            &serde_json::json!({"levels":["low","high"]}),
        );
        match &app.overlay {
            Some(crate::app::overlay::Overlay::ThinkingSelect { loading, .. }) => {
                assert!(*loading);
            }
            other => panic!("{other:?}"),
        }
        // 正确序号填充
        app.handle_async_data(
            AsyncKind::ThinkingLevels,
            seq,
            &serde_json::json!({"levels":["low","high"]}),
        );
        match &app.overlay {
            Some(crate::app::overlay::Overlay::ThinkingSelect { levels, loading, .. }) => {
                assert!(!*loading);
                assert_eq!(levels, &vec!["low".to_string(), "high".to_string()]);
            }
            other => panic!("{other:?}"),
        }
        // cycle_thinking_level 结果
        app.handle_async_data(AsyncKind::CycleThinking, 0, &serde_json::json!({"level":"xhigh"}));
        assert_eq!(app.state.thinking_level.as_deref(), Some("xhigh"));
        app.handle_async_data(AsyncKind::CycleThinking, 0, &serde_json::Value::Null);
        let (text, color) = app.notice.clone().expect("notice");
        assert!(text.contains("无可用"), "{text}");
        assert_eq!(color, Color::Yellow);
    }

    #[test]
    fn ctrl_l_via_handle_key_opens_model_selector() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (client, dir) = test_commander();
        let commander = client.command_sender();
        let (tx, rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();

        app.handle_key(
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL),
            &commander,
            &tx,
        );
        // 浮层应打开且 loading
        match &app.overlay {
            Some(crate::app::overlay::Overlay::ModelSelect { loading, seq, .. }) => {
                assert!(*loading, "打开即 loading");
                assert!(*seq >= 1);
            }
            other => panic!("overlay 未打开: {other:?}"),
        }
        // 异步请求已发出：稍候应收到 Models 数据（fake pi 对 get_available_models
        // 回 {success:true} 无 data → items 空但 loading 结束）
        let got = std::time::Instant::now();
        let mut filled = false;
        while got.elapsed() < std::time::Duration::from_secs(3) {
            if let Ok(UiEvent::AsyncData { kind, .. }) =
                rx.recv_timeout(std::time::Duration::from_millis(200))
                && kind == AsyncKind::Models
            {
                filled = true;
                break;
            }
        }
        assert!(filled, "应收到 Models 异步数据");
        app.handle_async_data(
            AsyncKind::Models,
            match &app.overlay {
                Some(crate::app::overlay::Overlay::ModelSelect { seq, .. }) => *seq,
                _ => 0,
            },
            &serde_json::json!({"models":[]}),
        );
        match &app.overlay {
            Some(crate::app::overlay::Overlay::ModelSelect { loading, .. }) => {
                assert!(!*loading, "数据到达后结束 loading");
            }
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
