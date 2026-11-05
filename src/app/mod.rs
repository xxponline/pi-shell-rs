//! Minimal ratatui TUI application (roadmap stage 2).
//!
//! Owns the terminal lifecycle, bridges keyboard and RPC events onto one
//! channel, reduces RPC events into a [`SessionState`], and renders a three-
//! region layout: header / scrollable chat / input editor.

mod autocomplete;
mod chat;
mod commands;
mod editor;
mod event;
mod markdown;
mod overlay;
mod sessions;
mod settings_io;
mod tree;
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
    /// 当前会话文件路径（get_state 刷新；选择器标记/删除校验用）。
    session_file: Option<String>,
    /// compaction 进行中本地排队的提交文本（压缩结束按序补发）。
    compact_queue: Vec<String>,
    /// 斜杠补全菜单状态（spec：斜杠命令候选触发与过滤）。
    completion: autocomplete::CompletionState,
    /// /model 参数补全的可用模型缓存（`provider/id`；惰性拉取一次）。
    models_cache: Vec<String>,
    /// /thinking 参数补全的可用级别缓存（惰性拉取一次）。
    levels_cache: Vec<String>,
    /// 模型/级别缓存是否已请求（防重复拉取）。
    models_requested: bool,
    levels_requested: bool,
    /// 设置文件路径覆盖（测试注入；None = 默认 ~/.pi/agent/settings.json）。
    settings_path: Option<std::path::PathBuf>,
    /// 上一次"空闲且空输入"的 Esc 时刻（双击窗口判定，500ms）。
    last_escape: Option<std::time::Instant>,
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
            session_file: None,
            compact_queue: Vec::new(),
            completion: autocomplete::CompletionState::default(),
            models_cache: Vec::new(),
            levels_cache: Vec::new(),
            models_requested: false,
            levels_requested: false,
            settings_path: None,
            last_escape: None,
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
            // 探活成功：应用 model/sessionName/thinkingLevel 并记住会话路径。
            Ok(data) => {
                self.state.apply_get_state(&data);
                self.session_file = data
                    .get("sessionFile")
                    .and_then(|f| f.as_str())
                    .map(String::from);
            }
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
                    // compaction 结束边沿：补发压缩期间排队的提交
                    if raw.get("type").and_then(|t| t.as_str()) == Some("compaction_end") {
                        self.flush_compact_queue(commander, report_tx);
                    }
                    self.draw(terminal)?;
                }
                Ok(UiEvent::AsyncData { kind, seq, data }) => {
                    self.log_event("async", &format!("{kind:?} seq={seq}"));
                    self.handle_async_data(kind, seq, &data);
                    // 缓存类数据到达后重算补全（如 /model 参数候选）
                    self.refresh_completion();
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
                Ok(UiEvent::OpenModelSelect) => {
                    self.log_event("info", "open model select (fallback)");
                    self.open_model_select(commander, report_tx);
                    self.draw(terminal)?;
                }
                Ok(UiEvent::Info(text)) => {
                    self.log_event("info", &text);
                    self.notice = Some((text, Color::Green));
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

    /// 由 get_session_stats 响应构建信息面板行（纯函数）。
    ///
    /// 输入: data — get_session_stats 的 data；state — 会话状态（取会话名）
    /// 输出: 信息面板文本行
    fn session_info_lines(data: &serde_json::Value, state: &SessionState) -> Vec<String> {
        let name = state
            .session_name
            .clone()
            .unwrap_or_else(|| "(未命名)".to_string());
        let num = |k: &str| -> String {
            data.get(k)
                .and_then(|v| v.as_u64())
                .map(|n| n.to_string())
                .unwrap_or_else(|| "-".to_string())
        };
        let token_total = data
            .pointer("/tokens/total")
            .and_then(|v| v.as_u64())
            .map(|n| n.to_string())
            .unwrap_or_else(|| "-".to_string());
        let cost = data
            .get("cost")
            .and_then(|v| v.as_f64())
            .map(|c| format!("${c:.4}"))
            .unwrap_or_else(|| "-".to_string());
        vec![
            format!("名称: {name}"),
            format!("会话 id: {}", data.get("sessionId").and_then(|v| v.as_str()).unwrap_or("-")),
            format!(
                "消息: {} 用户 / {} 助手 / {} 工具",
                num("userMessages"),
                num("assistantMessages"),
                num("toolCalls")
            ),
            format!("总消息数: {}", num("totalMessages")),
            format!("tokens: {token_total}"),
            format!("费用: {cost}"),
        ]
    }

    /// 打开会话树浮层：置 loading 并异步拉 get_tree（序号防过期）。
    ///
    /// 输入: report_tx — 回报通道
    /// 输出: 无返回值；副作用为浮层与异步树请求
    fn open_tree(&mut self, commander: &CommandSender, report_tx: &Sender<UiEvent>) {
        self.overlay_seq += 1;
        let seq = self.overlay_seq;
        self.overlay = Some(Overlay::Tree {
            tree: Vec::new(),
            leaf_id: None,
            rows: Vec::new(),
            cursor: 0,
            folded: std::collections::HashSet::new(),
            filter: crate::app::tree::FilterMode::Default,
            loading: true,
            seq,
        });
        self.log_event("overlay", "open tree");
        Self::spawn_async(
            commander,
            report_tx,
            serde_json::json!({"type": "get_tree"}),
            AsyncKind::Tree,
            seq,
            None,
        );
    }

    /// 打开会话信息面板：置 loading 并异步拉 get_session_stats。
    ///
    /// 输入: report_tx — 回报通道
    /// 输出: 无返回值；副作用为浮层与异步统计请求
    fn open_session_info(&mut self, commander: &CommandSender, report_tx: &Sender<UiEvent>) {
        self.overlay_seq += 1;
        let seq = self.overlay_seq;
        self.overlay = Some(Overlay::Info {
            title: " Session Info ".to_string(),
            lines: Vec::new(),
            loading: true,
            seq,
        });
        Self::spawn_async(
            commander,
            report_tx,
            serde_json::json!({"type": "get_session_stats"}),
            AsyncKind::Stats,
            seq,
            None,
        );
    }

    /// 打开会话选择器：置 loading 浮层并异步拉取会话列表（序号防过期）。
    ///
    /// 输入: commander/report_tx — 命令与回报通道
    /// 输出: 无返回值；副作用为浮层与异步列表请求
    fn open_session_selector(&mut self, report_tx: &Sender<UiEvent>) {
        self.overlay_seq += 1;
        let seq = self.overlay_seq;
        self.overlay = Some(Overlay::SessionSelect {
            items: Vec::new(),
            cursor: 0,
            loading: true,
            seq,
            confirming: false,
        });
        self.log_event("overlay", "open session selector");
        let report = report_tx.clone();
        thread::spawn(move || {
            let data = crate::app::sessions::list_sessions_json();
            let _ = report.send(UiEvent::AsyncData {
                kind: AsyncKind::SessionList,
                seq,
                data,
            });
        });
    }

    /// 会话切换链：首跳命令 → (cancelled → 通知并终止) → get_state 刷新
    /// → get_messages 重建 transcript。
    ///
    /// 输入: commander — 命令句柄；report_tx — 回报通道；first — 首跳命令
    ///       （switch_session/new_session/fork/clone）；command_name — 用于
    ///       取消通知的命令名；success_notice — 成功后的绿色提示（可空）
    /// 输出: 无返回值；副作用为异步执行整条链
    pub(crate) fn spawn_session_chain(
        commander: &CommandSender,
        report_tx: &Sender<UiEvent>,
        first: serde_json::Value,
        command_name: &'static str,
        success_notice: Option<&'static str>,
    ) {
        let report = report_tx.clone();
        let cmd = commander.clone();
        thread::spawn(move || {
            let first_resp = match cmd.send_command(
                first,
                Duration::from_secs(30),
                ResponsePolicy::RequireSuccess,
            ) {
                Ok(r) => r,
                Err(e) => {
                    let _ = report.send(UiEvent::SendFailed(e.to_string()));
                    return;
                }
            };
            let cancelled = first_resp
                .get("cancelled")
                .and_then(|c| c.as_bool())
                .unwrap_or(false);
            if cancelled {
                let _ = report.send(UiEvent::AsyncData {
                    kind: AsyncKind::SessionSwitched,
                    seq: 0,
                    data: serde_json::json!({ "command": command_name }),
                });
                return;
            }
            // 刷新 get_state（apply 模型/会话名 + session_file）
            if let Ok(state) = cmd.send_command(
                serde_json::json!({"type": "get_state"}),
                Duration::from_secs(30),
                ResponsePolicy::RequireSuccess,
            ) {
                let _ = report.send(UiEvent::AsyncData {
                    kind: AsyncKind::StateApplied,
                    seq: 0,
                    data: state,
                });
            }
            // 重建 transcript
            match cmd.send_command(
                serde_json::json!({"type": "get_messages"}),
                Duration::from_secs(30),
                ResponsePolicy::RequireSuccess,
            ) {
                Ok(messages) => {
                    let _ = report.send(UiEvent::AsyncData {
                        kind: AsyncKind::Messages,
                        seq: 0,
                        data: messages,
                    });
                    if let Some(notice) = success_notice {
                        let _ = report.send(UiEvent::Info(notice.to_string()));
                    }
                }
                Err(e) => {
                    let _ = report.send(UiEvent::SendFailed(e.to_string()));
                }
            }
        });
    }

    /// 删除会话：校验 + trash/unlink 后刷新选择器列表。
    ///
    /// 输入: overlay — 打开中的会话选择器（取 path/seq）；idx — 选中项；
    ///       report_tx — 回报通道
    /// 输出: 无返回值；副作用为异步删除并刷新列表
    fn request_delete_session(
        &self,
        overlay: &Overlay,
        idx: usize,
        report_tx: &Sender<UiEvent>,
    ) {
        let Overlay::SessionSelect { items, seq, .. } = overlay else {
            return;
        };
        let Some(item) = items.get(idx) else {
            return;
        };
        let path = item.path.clone();
        let seq = *seq;
        let current = self.session_file.clone();
        let report = report_tx.clone();
        thread::spawn(move || {
            // 删除校验以**项目子目录**为父（文件实际所在层）
            let (dir, _) = crate::app::sessions::project_sessions_dir();
            match crate::app::sessions::delete_session_file(&path, &dir, current.as_deref()) {
                Ok(()) => {
                    let data = crate::app::sessions::list_sessions_json();
                    let _ = report.send(UiEvent::AsyncData {
                        kind: AsyncKind::SessionList,
                        seq,
                        data,
                    });
                    let _ = report.send(UiEvent::Info("✓ 会话已删除".to_string()));
                }
                Err(e) => {
                    let _ = report.send(UiEvent::SendFailed(format!("删除失败: {e}")));
                }
            }
        });
    }

    /// /compact [自定义指令]：发送 compact 命令（异步；成败经 SendFailed/
    /// compaction_end 通知呈现）。
    ///
    /// 输入: args — 指令参数（可空）；commander/report_tx — 发送与回报
    /// 输出: 无返回值；输入清空，压缩流程由事件驱动
    fn cmd_compact(
        &mut self,
        args: &str,
        commander: &CommandSender,
        report_tx: &Sender<UiEvent>,
    ) {
        self.editor.replace("");
        let mut cmd = serde_json::json!({"type": "compact"});
        if !args.is_empty() {
            cmd["customInstructions"] = serde_json::Value::String(args.to_string());
        }
        self.log_event("slash", &format!("compact: {args}"));
        Self::spawn_async(commander, report_tx, cmd, AsyncKind::CommandDone, 0, None);
    }

    /// compaction 结束边沿：把压缩期间排队的文本按序补发（每条带 steer，
    /// idle 时 pi 忽略该字段、streaming 时合法排队）。
    ///
    /// 输入: commander/report_tx — 发送与回报通道
    /// 输出: 无返回值；副作用为清空 compact_queue 并异步发送
    fn flush_compact_queue(&mut self, commander: &CommandSender, report_tx: &Sender<UiEvent>) {
        if self.compact_queue.is_empty() {
            return;
        }
        let queued = std::mem::take(&mut self.compact_queue);
        self.log_event(
            "compact",
            &format!("flush {} queued message(s) after compaction", queued.len()),
        );
        for message in queued {
            let payload =
                Self::build_command_payload("prompt", &message, /*streaming*/ true);
            Self::spawn_async(
                commander,
                report_tx,
                payload,
                AsyncKind::CommandDone,
                0,
                None,
            );
        }
    }

    /// /copy：取最后一条 assistant 文本（get_last_assistant_text），
    /// 经 OSC52 转义序列写入终端剪贴板；无内容红条（spec：复制与信息展示命令）。
    ///
    /// 输入: commander/report_tx — 发送与回报通道
    /// 输出: 无返回值；副作用为清空输入（已由分发层）并异步取文本
    fn cmd_copy(&mut self, commander: &CommandSender, report_tx: &Sender<UiEvent>) {
        self.log_event("slash", "copy");
        Self::spawn_async(
            commander,
            report_tx,
            serde_json::json!({"type": "get_last_assistant_text"}),
            AsyncKind::CopyText,
            0,
            None,
        );
    }

    /// 标准 base64 编码（含 padding；OSC52 载荷用）。
    ///
    /// 输入: data — 原始字节
    /// 输出: base64 字符串
    fn base64_encode(data: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
        for chunk in data.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = *chunk.get(1).unwrap_or(&0) as u32;
            let b2 = *chunk.get(2).unwrap_or(&0) as u32;
            let n = (b0 << 16) | (b1 << 8) | b2;
            out.push(ALPHABET[(n >> 18) as usize & 63] as char);
            out.push(ALPHABET[(n >> 12) as usize & 63] as char);
            out.push(if chunk.len() > 1 {
                ALPHABET[(n >> 6) as usize & 63] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                ALPHABET[n as usize & 63] as char
            } else {
                '='
            });
        }
        out
    }

    /// OSC52 剪贴板转义序列（`ESC ] 52 ; c ; <base64> BEL`）。
    ///
    /// 输入: text — 要复制的文本
    /// 输出: 完整转义序列字符串
    fn osc52_sequence(text: &str) -> String {
        format!("\x1b]52;c;{}\x07", Self::base64_encode(text.as_bytes()))
    }

    /// 定位 pi 的 CHANGELOG.md：env 覆盖（测试）→ PI_BIN 所在目录上溯
    /// （npm/bun 安装布局）→ 仓库内 pi 源码树。
    ///
    /// 输入: 无（读环境变量与文件系统）
    /// 输出: Some(路径) / None（未找到）
    fn resolve_changelog_path() -> Option<std::path::PathBuf> {
        if let Ok(p) = std::env::var("PI_RUST_TUI_CHANGELOG") {
            let path = std::path::PathBuf::from(p);
            if path.is_file() {
                return Some(path);
            }
        }
        let bin = pi_rust_tui::rpc::resolve_binary();
        if let Ok(real) = std::fs::canonicalize(&bin) {
            let mut dir = real.parent().map(std::path::Path::to_path_buf);
            for _ in 0..8 {
                let d = dir?;
                let candidate = d.join("CHANGELOG.md");
                if candidate.is_file() {
                    return Some(candidate);
                }
                dir = d.parent().map(std::path::Path::to_path_buf);
            }
        }
        let repo = std::path::Path::new("pi/packages/coding-agent/CHANGELOG.md");
        if repo.is_file() {
            return Some(repo.to_path_buf());
        }
        None
    }

    /// 从 CHANGELOG 文本取最新条目：首个 `## ` 小节（无小节则整篇），
    /// 超过 60 行截断并标注。
    ///
    /// 输入: content — CHANGELOG.md 全文
    /// 输出: 最新条目的行列表（可为空）
    fn newest_changelog_lines(content: &str) -> Vec<String> {
        let mut lines = content.lines();
        let mut started = false;
        let mut out: Vec<String> = Vec::new();
        for line in lines.by_ref() {
            if !started {
                if line.starts_with("## ") {
                    started = true;
                    out.push(line.trim_start_matches('#').trim().to_string());
                }
                continue; // `# ` 标题行与前言跳过
            }
            if line.starts_with("## ") {
                break; // 下一个版本小节 → 最新条目结束
            }
            out.push(line.to_string());
        }
        if out.is_empty() {
            // 无 ## 结构：退化为整篇（统一走下方截断）
            out = content.lines().map(String::from).collect();
        }
        if out.len() > 60 {
            out.truncate(60);
            out.push("…（已截断，完整内容见 CHANGELOG.md）".into());
        }
        out
    }

    /// /changelog：读 pi 包内 CHANGELOG.md 最新条目渲染进消息区（逐行
    /// Notice）；文件缺失黄条提示（spec：复制与信息展示命令）。
    ///
    /// 输入: self — 应用状态
    /// 输出: 无返回值；副作用为追加本地通知条目或设置缺失提示
    fn cmd_changelog(&mut self) {
        self.log_event("slash", "changelog");
        let Some(path) = Self::resolve_changelog_path() else {
            self.notice = Some(("未找到 CHANGELOG.md（pi 包不完整？）".into(), Color::Yellow));
            return;
        };
        let Ok(content) = std::fs::read_to_string(&path) else {
            self.notice = Some(("CHANGELOG.md 读取失败".into(), Color::Yellow));
            return;
        };
        let body = Self::newest_changelog_lines(&content);
        if body.is_empty() {
            self.notice = Some(("No changelog entries found.".into(), Color::Yellow));
            return;
        }
        use pi_rust_tui::session::Entry;
        self.state.transcript.push(Entry::Notice {
            kind: "changelog".into(),
            text: "── What's New ──".into(),
        });
        for line in body {
            self.state.transcript.push(Entry::Notice {
                kind: "changelog".into(),
                text: line,
            });
        }
        self.notice = Some(("✓ changelog 已显示".into(), Color::Green));
    }

    /// /reload：空闲时重读本地配置（会话目录设置 + 补全缓存失效）并绿条
    /// 提示、触发重绘；流式/压缩中黄条拒绝（spec：配置重读）。
    ///
    /// 输入: self — 应用状态
    /// 输出: 无返回值；副作用为提示与缓存复位
    fn cmd_reload(&mut self) {
        self.log_event("slash", "reload");
        if self.state.phase == Phase::Streaming {
            self.notice = Some(("等待当前响应完成后再 reload".into(), Color::Yellow));
            return;
        }
        if self.state.compacting {
            self.notice = Some(("等待压缩完成后再 reload".into(), Color::Yellow));
            return;
        }
        // 重读会话目录设置（resolve_sessions_dir 每次现算，此处使其立即
        // 生效的可见动作是：参数补全缓存失效，下次 /model / /thinking 重拉）
        self.models_cache.clear();
        self.levels_cache.clear();
        self.models_requested = false;
        self.levels_requested = false;
        let dir = crate::app::sessions::resolve_sessions_dir();
        self.log_event("slash", &format!("reload sessions_dir={}", dir.display()));
        self.notice = Some((format!("✓ 配置已重新加载（sessions: {}）", dir.display()), Color::Green));
    }

    /// trust.json 路径（对齐 pi `~/.pi/agent/trust.json`）。
    ///
    /// 输入: 无（读 HOME）
    /// 输出: 文件路径
    fn trust_file_path() -> std::path::PathBuf {
        // 测试可用专用 env 覆盖（避免并行测试改 HOME 造成路径竞态）
        if let Ok(p) = std::env::var("PI_RUST_TUI_TRUST_FILE") {
            return std::path::PathBuf::from(p);
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        std::path::Path::new(&home).join(".pi/agent/trust.json")
    }

    /// 读取 trust.json 为 (key → decision) 映射；缺失返回空、损坏返回
    /// 空并由调用方提示（pi 侧损坏会抛错，我们降级不崩溃）。
    ///
    /// 输入: path — trust.json 路径
    /// 输出: 规范化的 JSON 对象（非对象视为无效 → 空）
    fn read_trust_file(path: &std::path::Path) -> serde_json::Map<String, serde_json::Value> {
        let Ok(text) = std::fs::read_to_string(path) else {
            return serde_json::Map::new();
        };
        match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(serde_json::Value::Object(map)) => map,
            _ => serde_json::Map::new(),
        }
    }

    /// 写回 trust.json（原子写：tmp + rename，父目录自动创建）。
    ///
    /// 输入: path — 文件路径；map — 完整映射
    /// 输出: Ok(()) / Err(写入失败)
    fn write_trust_file(
        path: &std::path::Path,
        map: &serde_json::Map<String, serde_json::Value>,
    ) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(map).unwrap_or_default())?;
        std::fs::rename(&tmp, path)
    }

    /// 当前 cwd 的规范化键（对齐 pi normalizeCwd：canonicalize 解析软链）。
    ///
    /// 输入: 无
    /// 输出: 规范化路径字符串
    fn current_cwd_key() -> String {
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        std::fs::canonicalize(&cwd)
            .unwrap_or(cwd)
            .to_string_lossy()
            .to_string()
    }

    /// /export [路径]：无参/`.jsonl` 复制当前会话文件（默认
    /// `<cwd>/session-<id>.jsonl`）；非 `.jsonl` 走 export_html RPC。
    /// 无会话文件红条（spec：会话导出）。
    ///
    /// 输入: args — 输出路径（可空）；commander/report_tx — 发送与回报
    /// 输出: 无返回值；副作用为复制文件或异步导出 HTML
    fn cmd_export(&mut self, args: &str, commander: &CommandSender, report_tx: &Sender<UiEvent>) {
        self.log_event("slash", &format!("export: {args}"));
        let Some(src) = self.session_file.clone() else {
            self.notice = Some(("无当前会话文件可导出".into(), Color::Red));
            return;
        };
        let src_path = std::path::Path::new(&src);
        if !src_path.is_file() {
            self.notice = Some((format!("会话文件不存在: {src}"), Color::Red));
            return;
        }
        let target = if args.is_empty() {
            Self::default_export_path(&src, &std::env::current_dir().unwrap_or_default())
        } else {
            std::path::PathBuf::from(args)
        };
        let target_str = target.to_string_lossy().to_string();
        if args.is_empty() || target_str.ends_with(".jsonl") {
            match std::fs::copy(src_path, &target) {
                Ok(n) => {
                    self.notice = Some((
                        format!("✓ 已导出 {} 字节 → {}", n, target.display()),
                        Color::Green,
                    ));
                    self.log_event("slash", &format!("export jsonl → {}", target.display()));
                }
                Err(e) => {
                    self.notice = Some((format!("Failed to export session: {e}"), Color::Red));
                }
            }
        } else {
            // HTML（或其它扩展名，对齐 pi：非 jsonl 即 HTML）
            Self::spawn_async(
                commander,
                report_tx,
                serde_json::json!({"type": "export_html", "outputPath": target_str}),
                AsyncKind::ExportDone,
                0,
                None,
            );
        }
    }

    /// 默认导出路径：`<cwd>/session-<sessionId>.jsonl`（id 取会话文件名
    /// `ts_sessionId.jsonl` 的第二段，缺失则 `export`）。
    ///
    /// 输入: session_file — 当前会话文件路径；cwd — 输出目录
    /// 输出: 默认导出文件路径
    fn default_export_path(session_file: &str, cwd: &std::path::Path) -> std::path::PathBuf {
        let stem = std::path::Path::new(session_file)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let sid = stem.split('_').nth(1).unwrap_or("export");
        cwd.join(format!("session-{sid}.jsonl"))
    }

    /// /import <路径>：无参显示用法；带参校验（存在、.jsonl、首行可解析）
    /// 后弹二次确认（复制与切换在确认后）；校验失败红条、不开浮层。
    ///
    /// 输入: args — 源 jsonl 路径；self — 置确认浮层
    /// 输出: 无返回值
    fn cmd_import(&mut self, args: &str) {
        self.log_event("slash", &format!("import: {args}"));
        if args.is_empty() {
            self.notice = Some(("Usage: /import <path.jsonl>".into(), Color::Yellow));
            return;
        }
        let src = std::path::Path::new(args);
        if !args.ends_with(".jsonl") || !src.is_file() {
            self.notice = Some((format!("源文件不存在或非 .jsonl: {args}"), Color::Red));
            return;
        }
        if crate::app::sessions::parse_session_file(src).is_none() {
            self.notice = Some((format!("源文件非法（首行非 JSON）: {args}"), Color::Red));
            return;
        }
        self.overlay = Some(Overlay::ImportConfirm {
            path: args.to_string(),
            cursor: 0,
        });
    }

    /// 执行导入（确认后）：校验源文件（存在、.jsonl、首行可解析）→ 复制进
    /// 当前项目会话目录 → 切换链重建；任一步失败仅提示、不改状态。
    ///
    /// 输入: source — 源 jsonl 路径；commander/report_tx — 发送与回报
    /// 输出: 无返回值
    fn apply_import(&mut self, source: &str, commander: &CommandSender, report_tx: &Sender<UiEvent>) {
        let src = std::path::Path::new(source);
        if !source.ends_with(".jsonl") || !src.is_file() {
            self.notice = Some((format!("导入失败：源文件不存在或非 .jsonl: {source}"), Color::Red));
            return;
        }
        if crate::app::sessions::parse_session_file(src).is_none() {
            self.notice = Some((format!("导入失败：文件非法（首行非 JSON）: {source}"), Color::Red));
            return;
        }
        let (dir, _) = crate::app::sessions::project_sessions_dir();
        let file_name = src.file_name().expect("文件名已校验");
        let dest = dir.join(file_name);
        // 源已在项目会话目录：跳过自拷贝（fs::copy 同文件会截断），直接切换
        let src_real = std::fs::canonicalize(src).unwrap_or_else(|_| src.to_path_buf());
        let dest_real = std::fs::canonicalize(&dest).unwrap_or(dest.clone());
        if src_real == dest_real {
            self.log_event("slash", "import: source already in project dir, skip copy");
        } else if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::copy(src, &dest)) {
            self.notice = Some((format!("导入失败: {e}"), Color::Red));
            return;
        } else {
            self.log_event("slash", &format!("import copy → {}", dest.display()));
        }
        // 切换到导入的会话（复用统一切换链：get_state + get_messages 重建）
        Self::spawn_session_chain(
            commander,
            report_tx,
            serde_json::json!({"type": "switch_session", "sessionPath": dest.to_string_lossy()}),
            "import",
            Some("✓ 已导入并切换"),
        );
    }

    /// /scoped-models：打开只读模型范围浮层（loading → get_available_models）。
    ///
    /// 输入: commander/report_tx — 发送与回报通道
    /// 输出: 无返回值；副作用为置 ScopedModels 浮层并异步拉列表
    fn open_scoped_models(&mut self, commander: &CommandSender, report_tx: &Sender<UiEvent>) {
        self.log_event("slash", "scoped-models");
        self.overlay_seq += 1;
        let seq = self.overlay_seq;
        self.overlay = Some(Overlay::ScopedModels {
            models: vec![],
            loading: true,
            seq,
            cursor: 0,
        });
        Self::spawn_async(
            commander,
            report_tx,
            serde_json::json!({"type": "get_available_models"}),
            AsyncKind::ScopedModels,
            seq,
            None,
        );
    }

    /// 双击 Esc 判定（对齐 pi `onEscape`）：动作非 none 且输入为空时，
    /// 500ms 内第二次空闲 Esc 触发（tree/fork → 打开会话树，none → 不动作）；
    /// 首次只记录时间戳，触发后复位。非空输入的 Esc 不触发也不刷新窗口。
    ///
    /// 输入: commander/report_tx — 打开会话树需要的通道
    /// 输出: 无返回值；副作用为开树或记录时间戳（含日志）
    fn handle_double_escape(&mut self, commander: &CommandSender, report_tx: &Sender<UiEvent>) {
        use crate::app::settings_io::DoubleEscapeAction;
        let action =
            crate::app::settings_io::read_double_escape_action(&self.settings_path());
        if action == DoubleEscapeAction::None {
            // pi：action=none 时连窗口都不追踪
            self.log_event("key", "esc (double-escape disabled)");
            return;
        }
        // 仅空（含全空白）输入参与窗口——对齐 pi 的 empty editor 条件
        if self.can_submit() {
            return;
        }
        let now = std::time::Instant::now();
        let triggered = match self.last_escape {
            Some(prev) if now.duration_since(prev).as_millis() < 500 => {
                self.last_escape = None; // 触发后复位（pi: lastEscapeTime = 0）
                true
            }
            _ => {
                self.last_escape = Some(now);
                false
            }
        };
        if !triggered {
            self.log_event("key", "esc (first of double)");
            return;
        }
        self.log_event("key", &format!("double-esc → {action:?}"));
        // fork = 树（沿用 /fork 的既有简化，design D4）
        self.open_tree(commander, report_tx);
    }

    /// 本 App 使用的设置文件路径（测试可注入覆盖）。
    ///
    /// 输入: self — 应用状态
    /// 输出: settings.json 路径
    fn settings_path(&self) -> std::path::PathBuf {
        self.settings_path
            .clone()
            .unwrap_or_else(crate::app::settings_io::settings_file_path)
    }

    /// /settings：打开简化设置浮层（loading → get_state 回填开关与模型）。
    ///
    /// 输入: commander/report_tx — 发送与回报通道
    /// 输出: 无返回值；副作用为置 Settings 浮层并异步拉 get_state
    fn open_settings(&mut self, commander: &CommandSender, report_tx: &Sender<UiEvent>) {
        self.log_event("slash", "settings");
        self.overlay_seq += 1;
        let seq = self.overlay_seq;
        let double_esc = Some(
            crate::app::settings_io::read_double_escape_action(&self.settings_path())
                .as_str()
                .to_string(),
        );
        self.overlay = Some(Overlay::Settings {
            cursor: 0,
            loading: true,
            seq,
            auto_compaction: None,
            auto_retry: None,
            double_esc,
            model: None,
        });
        Self::spawn_async(
            commander,
            report_tx,
            serde_json::json!({"type": "get_state"}),
            AsyncKind::SettingsState,
            seq,
            None,
        );
    }

    /// 应用设置浮层选中项：0 = autoCompact、1 = autoRetry（翻转并发送）。
    ///
    /// 输入: overlay — 当前浮层（读开关状态）；idx — 行号；
    ///       commander/report_tx — 发送与回报
    /// 输出: 无返回值；副作用为发送 set_auto_* 并更新浮层状态
    fn apply_settings(
        &mut self,
        overlay: &mut Overlay,
        idx: usize,
        commander: &CommandSender,
        report_tx: &Sender<UiEvent>,
    ) {
        let Overlay::Settings {
            auto_compaction,
            auto_retry,
            double_esc,
            ..
        } = overlay
        else {
            return;
        };
        match idx {
            0 => {
                let next = !auto_compaction.unwrap_or(false);
                *auto_compaction = Some(next);
                Self::spawn_async(
                    commander,
                    report_tx,
                    serde_json::json!({"type": "set_auto_compaction", "enabled": next}),
                    AsyncKind::CommandDone,
                    0,
                    None,
                );
                self.log_event("slash", &format!("settings auto_compaction={next}"));
            }
            1 => {
                let next = !auto_retry.unwrap_or(false);
                *auto_retry = Some(next);
                Self::spawn_async(
                    commander,
                    report_tx,
                    serde_json::json!({"type": "set_auto_retry", "enabled": next}),
                    AsyncKind::CommandDone,
                    0,
                    None,
                );
                self.log_event("slash", &format!("settings auto_retry={next}"));
            }
            2 => {
                // doubleEsc：tree→fork→none 循环并写回 settings.json（立即生效）
                use crate::app::settings_io::{self, DoubleEscapeAction};
                let path = self.settings_path();
                let current = DoubleEscapeAction::parse(double_esc.as_deref().unwrap_or(""))
                    .unwrap_or(DoubleEscapeAction::Tree);
                let next = current.next();
                match settings_io::write_double_escape_action(&path, next) {
                    Ok(()) => {
                        *double_esc = Some(next.as_str().to_string());
                        self.log_event("slash", &format!("settings doubleEsc={next:?}"));
                    }
                    Err(e) => {
                        // 写失败：红条 + 行值回读文件（不显示未生效值）
                        self.notice = Some((format!("doubleEsc 写入失败: {e}"), Color::Red));
                        *double_esc =
                            Some(settings_io::read_double_escape_action(&path).as_str().to_string());
                    }
                }
            }
            _ => {}
        }
    }

    /// /trust：读当前项目的信任记录并打开决策浮层（Enter 应用）。
    ///
    /// 输入: self — 应用状态
    /// 输出: 无返回值；副作用为置 Trust 浮层
    fn open_trust(&mut self) {
        self.log_event("slash", "trust");
        let path = Self::trust_file_path();
        let map = Self::read_trust_file(&path);
        let key = Self::current_cwd_key();
        let current = map.get(&key).and_then(|v| v.as_bool());
        self.overlay = Some(Overlay::Trust { cursor: 0, current });
    }

    /// 应用信任决策：idx 0 = 信任（true）、1 = 清除（删除键）。
    ///
    /// 输入: idx — 浮层选中项
    /// 输出: 无返回值；副作用为写回 trust.json 与提示
    fn apply_trust(&mut self, idx: usize) {
        let path = Self::trust_file_path();
        let mut map = Self::read_trust_file(&path);
        let key = Self::current_cwd_key();
        match idx {
            0 => {
                map.insert(key.clone(), serde_json::Value::Bool(true));
                match Self::write_trust_file(&path, &map) {
                    Ok(()) => {
                        self.notice = Some((format!("✓ 已信任 {key}"), Color::Green));
                        self.log_event("slash", &format!("trust set {key}"));
                    }
                    Err(e) => {
                        self.notice = Some((format!("信任写入失败: {e}"), Color::Red));
                    }
                }
            }
            1 => {
                map.remove(&key);
                match Self::write_trust_file(&path, &map) {
                    Ok(()) => {
                        self.notice = Some(("✓ 已清除信任记录".into(), Color::Green));
                        self.log_event("slash", &format!("trust clear {key}"));
                    }
                    Err(e) => {
                        self.notice = Some((format!("信任写入失败: {e}"), Color::Red));
                    }
                }
            }
            _ => {}
        }
    }

    /// /hotkeys：打开键位表浮层（数据见 overlay::hotkeys_rows）。
    ///
    /// 输入: self — 应用状态
    /// 输出: 无返回值；副作用为置 Hotkeys 浮层
    fn open_hotkeys(&mut self) {
        self.log_event("slash", "hotkeys");
        self.overlay = Some(Overlay::Hotkeys { cursor: 0 });
    }

    /// 桩命令：本地拦截后给出黄条原因提示（/share /bug 网络上报类、
    /// /login /logout 交互登录类），不发送任何命令。
    ///
    /// 输入: name — 命令名（不含 `/`）
    /// 输出: 无返回值；副作用为设置黄色通知（spec：未支持命令提示桩）
    fn cmd_stub(&mut self, name: &str) {
        let reason = match name {
            "share" => "暂不支持：/share 需要网络上报（Radius artifact / GitHub gist）",
            "bug" => "暂不支持：/bug 需要网络上报到 Pi 开发者",
            "login" => "未实现交互式登录（依赖 ~/.pi/agent/auth.json 预登录）",
            "logout" => "未实现交互式登录（移除凭据请直接编辑 auth.json）",
            _ => "暂不支持",
        };
        self.log_event("slash", &format!("stub {name}: {reason}"));
        self.notice = Some((format!("🚫 {reason}"), Color::Yellow));
    }

    /// /quit：置退出标志并清空输入。
    ///
    /// 输入: 无
    /// 输出: 无返回值；副作用为 should_quit=true、输入清空
    fn cmd_quit(&mut self) {
        self.editor.replace("");
        self.should_quit = true;
    }

    /// /name <名称>：发送 set_session_name（无参时提示用法，不发送）。
    ///
    /// 输入: args — 命令参数原文（已 trim）；commander/report_tx — 发送与回报
    /// 输出: 无返回值；有参异步发命令并清空输入，无参显示用法提示
    fn cmd_set_name(
        &mut self,
        args: &str,
        commander: &CommandSender,
        report_tx: &Sender<UiEvent>,
    ) {
        if args.is_empty() {
            self.notice = Some(("用法: /name <名称>".to_string(), Color::Yellow));
            self.editor.replace("");
            return;
        }
        let name = args.to_string();
        self.editor.replace("");
        self.log_event("slash", &format!("set_session_name: {name}"));
        Self::spawn_async(
            commander,
            report_tx,
            serde_json::json!({"type": "set_session_name", "name": name}),
            AsyncKind::CommandDone,
            0,
            None,
        );
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
            self.completion.close(); // 输入被清空，菜单随之失效
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
                    self.apply_overlay(&mut overlay, idx, commander, report_tx);
                    self.close_overlay();
                }
                OverlayOutcome::Toggle(idx) => {
                    self.log_event("overlay", &format!("toggle #{idx}"));
                    // 设置类切换：应用后保持浮层打开（改动写回同一个浮层值）
                    let mut o = overlay;
                    self.apply_overlay(&mut o, idx, commander, report_tx);
                    self.overlay = Some(o);
                }
                OverlayOutcome::Delete(idx) => {
                    self.log_event("overlay", &format!("delete #{idx}"));
                    self.request_delete_session(&overlay, idx, report_tx);
                    // 浮层保持打开等待异步刷新，确认态复位
                    let mut o = overlay;
                    if let Overlay::SessionSelect { confirming, .. } = &mut o {
                        *confirming = false;
                    }
                    self.overlay = Some(o);
                }
            }
            self.refresh_completion();
            return;
        }

        // 补全菜单打开时优先消费导航/应用键（spec：菜单键位与应用）。
        // 各消费键直接 return——天然跳过函数末尾的补全重算（Tab 应用后
        // 保持菜单关闭，等下次输入再开）。
        if self.completion.open
            && (key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT)
        {
            match key.code {
                KeyCode::Up => {
                    self.completion_move(-1);
                    return;
                }
                KeyCode::Down => {
                    self.completion_move(1);
                    return;
                }
                KeyCode::PageUp => {
                    self.completion_move(-5);
                    return;
                }
                KeyCode::PageDown => {
                    self.completion_move(5);
                    return;
                }
                KeyCode::Esc => {
                    // spec：Esc 只关菜单，不清输入不提交
                    self.completion.close();
                    return;
                }
                KeyCode::Tab => {
                    // spec：Tab 应用补全后关菜单（不触发提交）；return 即跳过重算
                    self.apply_completion_selection();
                    return;
                }
                KeyCode::Enter if key.modifiers == KeyModifiers::NONE => {
                    // spec：回车先应用补全再立即提交该行（对齐 pi fall-through）
                    self.apply_completion_selection();
                }
                _ => {}
            }
        }

        let intent = Self::key_intent(key, self.state.phase == Phase::Streaming);

        // 空闲 Esc：双击判定（spec：双击 Esc 动作）。浮层/补全的 Esc 已在前序
        // 分支消费（优先且不计入窗口），流式 Esc 走 Abort 不落此分支。
        if key.code == KeyCode::Esc && key.modifiers == KeyModifiers::NONE && intent == KeyIntent::Ignore
        {
            self.handle_double_escape(commander, report_tx);
            return;
        }
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
        // 每次按键后按光标前文本重算补全（菜单内消费键已在上方 return）
        self.update_completion(commander, report_tx);
    }

    /// 按当前光标前文本重算补全菜单（无 RPC）。
    ///
    /// 输入: self — 应用状态（读编辑器与缓存）
    /// 输出: 无返回值；副作用为更新 completion（浮层打开时强制关闭）
    fn refresh_completion(&mut self) {
        if self.overlay.is_some() {
            self.completion.close();
            return;
        }
        let row = self.editor.row();
        let line = self.editor.lines()[row].clone();
        let col = self.editor.col();
        let byte = line
            .char_indices()
            .nth(col)
            .map(|(b, _)| b)
            .unwrap_or(line.len());
        let text_before = &line[..byte];
        match autocomplete::suggestions(text_before, &self.models_cache, &self.levels_cache) {
            Some((kind, prefix, items)) => {
                let query = match &kind {
                    autocomplete::CompletionKind::Command => prefix.trim_start_matches('/').to_string(),
                    autocomplete::CompletionKind::Argument(_) => prefix.clone(),
                };
                let selected = autocomplete::best_match_index(&items, &query);
                self.completion = autocomplete::CompletionState {
                    open: true,
                    prefix,
                    items,
                    selected,
                    kind: Some(kind),
                };
            }
            None => self.completion.close(),
        }
    }

    /// 重算补全，参数候选缓存为空时惰性拉取（各一次）。
    ///
    /// 输入: commander/report_tx — 发送与回报通道
    /// 输出: 无返回值；副作用为可能异步发起 get_available_* 请求
    fn update_completion(&mut self, commander: &CommandSender, report_tx: &Sender<UiEvent>) {
        self.refresh_completion();
        // 从光标文本判定数据需求（缓存为空时菜单尚未打开，kind 不可用）
        let row = self.editor.row();
        let line = self.editor.lines()[row].clone();
        let col = self.editor.col();
        let byte = line
            .char_indices()
            .nth(col)
            .map(|(b, _)| b)
            .unwrap_or(line.len());
        let need = autocomplete::arg_need(&line[..byte]);
        let need_models = need == Some("model");
        if need_models && self.models_cache.is_empty() && !self.models_requested {
            self.models_requested = true;
            Self::spawn_async(
                commander,
                report_tx,
                serde_json::json!({"type": "get_available_models"}),
                AsyncKind::ArgCacheModels,
                0,
                None,
            );
        }
        let need_levels = need == Some("thinking");
        if need_levels && self.levels_cache.is_empty() && !self.levels_requested {
            self.levels_requested = true;
            Self::spawn_async(
                commander,
                report_tx,
                serde_json::json!({"type": "get_available_thinking_levels"}),
                AsyncKind::ArgCacheLevels,
                0,
                None,
            );
        }
    }

    /// 移动菜单选中项（夹取边界）。
    ///
    /// 输入: delta — +1 下移 / -1 上移 / ±N 翻页
    /// 输出: 无返回值；副作用为更新 completion.selected
    fn completion_move(&mut self, delta: i32) {
        let len = self.completion.items.len() as i32;
        if len == 0 {
            return;
        }
        let next = (self.completion.selected as i32 + delta).clamp(0, len - 1);
        self.completion.selected = next as usize;
        self.log_event("key", &format!("completion select {next}"));
    }

    /// 应用当前选中补全到编辑器（替换前缀），随后关闭菜单。
    ///
    /// 输入: self — 应用状态（编辑器与菜单）
    /// 输出: 无返回值；副作用为改写当前行与光标、关闭菜单
    fn apply_completion_selection(&mut self) {
        let Some(kind) = self.completion.kind.clone() else {
            self.completion.close();
            return;
        };
        let Some(item) = self.completion.items.get(self.completion.selected).cloned() else {
            self.completion.close();
            return;
        };
        let row = self.editor.row();
        let line = self.editor.lines()[row].clone();
        let (new_line, new_cursor) = autocomplete::apply_completion(
            &line,
            self.editor.col(),
            &self.completion.prefix,
            &item.value,
            &kind,
        );
        self.editor.set_line(row, new_line, new_cursor);
        self.log_event("key", &format!("completion apply {}", item.value));
        self.completion.close();
    }

    /// /model [搜索]：无参开选择器；带参做精确匹配（`provider/id` 或 `id`，
    /// 大小写不敏感）命中直接 set_model 并绿条提示，无命中回退选择器。
    ///
    /// 输入: args — 搜索词（可空）；commander/report_tx — 发送与回报
    /// 输出: 无返回值；副作用为开浮层或异步匹配切换
    fn cmd_model(&mut self, args: &str, commander: &CommandSender, report_tx: &Sender<UiEvent>) {
        self.log_event("slash", &format!("model: {args}"));
        if args.is_empty() {
            self.open_model_select(commander, report_tx);
            return;
        }
        let search = args.to_string();
        let report = report_tx.clone();
        let cmd = commander.clone();
        thread::spawn(move || {
            let list = cmd.send_command(
                serde_json::json!({"type": "get_available_models"}),
                Duration::from_secs(30),
                ResponsePolicy::RequireSuccess,
            );
            let data = match list {
                Ok(d) => d,
                Err(e) => {
                    let _ = report.send(UiEvent::SendFailed(e.to_string()));
                    return;
                }
            };
            let q = search.to_lowercase();
            let models: Vec<(String, String)> = data
                .get("models")
                .and_then(|m| m.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|m| {
                            let provider = m.get("provider").and_then(|x| x.as_str())?;
                            let id = m.get("id").and_then(|x| x.as_str())?;
                            Some((provider.to_string(), id.to_string()))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let hit = models.iter().find(|(provider, id)| {
                format!("{provider}/{id}").to_lowercase() == q || id.to_lowercase() == q
            });
            match hit {
                Some((provider, id)) => {
                    let applied = cmd.send_command(
                        serde_json::json!({"type": "set_model", "provider": provider, "modelId": id}),
                        Duration::from_secs(30),
                        ResponsePolicy::RequireSuccess,
                    );
                    match applied {
                        Ok(_) => {
                            // 回填状态行模型名 + 绿条提示
                            let _ = report.send(UiEvent::AsyncData {
                                kind: AsyncKind::ModelApplied,
                                seq: 0,
                                data: serde_json::json!({"name": format!("{provider}/{id}")}),
                            });
                            let _ = report.send(UiEvent::Info(format!("Model: {provider}/{id}")));
                        }
                        Err(e) => {
                            let _ = report.send(UiEvent::SendFailed(e.to_string()));
                        }
                    }
                }
                None => {
                    let _ = report.send(UiEvent::Info(
                        "未找到匹配模型，打开选择器".to_string(),
                    ));
                    let _ = report.send(UiEvent::OpenModelSelect);
                }
            }
        });
    }

    /// /thinking [级别]：无参开级别列表；带参精确匹配（大小写不敏感）直接
    /// 应用，未知级别红条列出全部可用级别（spec：thinking 入口与未知级别）。
    ///
    /// 输入: args — 级别名（可空）；commander/report_tx — 发送与回报
    /// 输出: 无返回值；副作用为开浮层或异步应用
    fn cmd_thinking(&mut self, args: &str, commander: &CommandSender, report_tx: &Sender<UiEvent>) {
        self.log_event("slash", &format!("thinking: {args}"));
        if args.is_empty() {
            self.open_thinking_select(commander, report_tx);
            return;
        }
        let want = args.to_string();
        let report = report_tx.clone();
        let cmd = commander.clone();
        thread::spawn(move || {
            let list = cmd.send_command(
                serde_json::json!({"type": "get_available_thinking_levels"}),
                Duration::from_secs(30),
                ResponsePolicy::RequireSuccess,
            );
            let data = match list {
                Ok(d) => d,
                Err(e) => {
                    let _ = report.send(UiEvent::SendFailed(e.to_string()));
                    return;
                }
            };
            let levels: Vec<String> = data
                .get("levels")
                .and_then(|l| l.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let q = want.to_lowercase();
            let hit = levels.iter().find(|l| l.to_lowercase() == q);
            match hit {
                Some(level) => {
                    let applied = cmd.send_command(
                        serde_json::json!({"type": "set_thinking_level", "level": level}),
                        Duration::from_secs(30),
                        ResponsePolicy::RequireSuccess,
                    );
                    match applied {
                        Ok(_) => {
                            let _ = report.send(UiEvent::AsyncData {
                                kind: AsyncKind::ThinkingApplied,
                                seq: 0,
                                data: serde_json::json!(level.clone()),
                            });
                            let _ = report.send(UiEvent::Info(format!("thinking: {level}")));
                        }
                        Err(e) => {
                            let _ = report.send(UiEvent::SendFailed(e.to_string()));
                        }
                    }
                }
                None => {
                    let _ = report.send(UiEvent::SendFailed(format!(
                        "Unknown thinking level \"{want}\". Available levels: {}",
                        levels.join(", ")
                    )));
                }
            }
        });
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
        // compaction 进行中：本地排队（pi 的 prompt 在压缩中直接拒绝，必须客户端排队）
        if self.state.compacting {
            if message.is_empty() {
                self.log_event("submit", "skipped: empty input (compact queue)");
                return;
            }
            self.compact_queue.push(message.clone());
            self.editor.replace("");
            self.notice = Some((
                format!("压缩进行中，已排队 {} 条", self.compact_queue.len()),
                Color::Yellow,
            ));
            self.log_event("submit", &format!("queued during compact: {message}"));
            return;
        }
        // 本地命令与正常发送都先清空输入（spec：命令执行后输入清空）
        self.editor.replace("");
        // 斜杠分发：仅普通 prompt 路径（Enter 与空闲 alt+enter 共用）；
        // 流式 alt+enter 的 follow_up 路径不拦截（对齐 pi）。
        if command == "prompt"
            && crate::app::commands::dispatch(self, commander, report_tx, &message)
        {
            self.log_event("submit", &format!("local command: {message}"));
            return;
        }
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
        overlay: &mut Overlay,
        idx: usize,
        commander: &CommandSender,
        report_tx: &Sender<UiEvent>,
    ) {
        match overlay {
            Overlay::ImportConfirm { path, .. } => {
                if idx == 0 {
                    self.apply_import(path, commander, report_tx);
                } else {
                    self.notice = Some(("Import cancelled".into(), Color::Yellow));
                }
            }
            Overlay::Settings { .. } => self.apply_settings(overlay, idx, commander, report_tx),
            Overlay::Trust { .. } => self.apply_trust(idx),
            Overlay::Tree { rows, .. } => {
                if let Some(row) = rows.get(idx) {
                    let entry_id = row.id.clone();
                    Self::spawn_session_chain(
                        commander,
                        report_tx,
                        serde_json::json!({"type": "fork", "entryId": entry_id}),
                        "fork",
                        Some("✓ 已分支到选中节点"),
                    );
                }
            }
            Overlay::SessionSelect { items, .. } => {
                if let Some(item) = items.get(idx) {
                    Self::spawn_session_chain(
                        commander,
                        report_tx,
                        serde_json::json!({
                            "type": "switch_session",
                            "sessionPath": item.path.to_string_lossy()
                        }),
                        "switch_session",
                        None,
                    );
                }
            }
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
            AsyncKind::CommandDone => {
                // 无需回填状态；错误已由 SendFailed 通道处理。
            }
            AsyncKind::StateApplied => {
                self.state.apply_get_state(data);
                self.session_file = data
                    .get("sessionFile")
                    .and_then(|f| f.as_str())
                    .map(String::from);
            }
            AsyncKind::Messages => {
                self.state.load_messages(data);
            }
            AsyncKind::SessionSwitched => {
                let cmd = data
                    .get("command")
                    .and_then(|c| c.as_str())
                    .unwrap_or("会话操作");
                self.notice = Some((format!("{cmd} 已取消"), Color::Yellow));
            }
            AsyncKind::Tree => {
                if let Some(Overlay::Tree {
                    tree,
                    leaf_id,
                    rows,
                    folded,
                    filter,
                    loading,
                    seq: s,
                    ..
                }) = self.overlay.as_mut()
                    && *s == seq
                {
                        *tree = data
                            .get("tree")
                            .and_then(|t| t.as_array())
                            .cloned()
                            .unwrap_or_default();
                        *leaf_id = data
                            .get("leafId")
                            .and_then(|l| l.as_str())
                            .map(String::from);
                        *rows = crate::app::tree::flatten_tree(
                            tree,
                            leaf_id.as_deref(),
                            folded,
                            *filter,
                        );
                        *loading = false;
                }
            }
            AsyncKind::ArgCacheModels => {
                // /model 参数补全缓存：provider/id 列表（与选择器同源数据）
                self.models_cache = data
                    .get("models")
                    .and_then(|m| m.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|m| {
                                let provider = m.get("provider").and_then(|x| x.as_str())?;
                                let id = m.get("id").and_then(|x| x.as_str())?;
                                Some(format!("{provider}/{id}"))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                self.log_event("async", &format!("models cache {}", self.models_cache.len()));
            }
            AsyncKind::ArgCacheLevels => {
                self.levels_cache = data
                    .get("levels")
                    .and_then(|l| l.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                self.log_event("async", &format!("levels cache {}", self.levels_cache.len()));
            }
            AsyncKind::CopyText => {
                let text = data.get("text").and_then(|t| t.as_str()).unwrap_or("");
                if text.is_empty() {
                    self.notice = Some(("暂无可复制的 assistant 回复".into(), Color::Red));
                } else {
                    use std::io::Write as _;
                    let seq = Self::osc52_sequence(text);
                    let mut stdout = std::io::stdout();
                    let _ = stdout.write_all(seq.as_bytes());
                    let _ = stdout.flush();
                    self.notice = Some((
                        format!("✓ 已复制 {} 字符到剪贴板", text.chars().count()),
                        Color::Green,
                    ));
                    self.log_event("slash", &format!("copy {} chars", text.chars().count()));
                }
            }
            AsyncKind::SettingsState => {
                let mut filled = None;
                if let Some(Overlay::Settings {
                    loading,
                    seq: s,
                    auto_compaction,
                    model,
                    ..
                }) = self.overlay.as_mut()
                    && *s == seq
                {
                    *auto_compaction = data.get("autoCompactionEnabled").and_then(|v| v.as_bool());
                    *model = match data.get("model") {
                        Some(serde_json::Value::String(m)) => Some(m.clone()),
                        Some(obj) => obj
                            .get("name")
                            .or_else(|| obj.get("id"))
                            .and_then(|v| v.as_str())
                            .map(String::from),
                        None => None,
                    };
                    *loading = false;
                    filled = Some(*auto_compaction);
                }
                if let Some(v) = filled {
                    self.log_event("async", &format!("settings state compacting={v:?}"));
                }
            }
            AsyncKind::ScopedModels => {
                let mut count = None;
                if let Some(Overlay::ScopedModels {
                    models,
                    loading,
                    seq: s,
                    ..
                }) = self.overlay.as_mut()
                    && *s == seq
                {
                    *models = data
                        .get("models")
                        .and_then(|m| m.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|m| {
                                    let provider = m.get("provider").and_then(|x| x.as_str())?;
                                    let id = m.get("id").and_then(|x| x.as_str())?;
                                    Some(format!("{provider}/{id}"))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    *loading = false;
                    count = Some(models.len());
                }
                if let Some(n) = count {
                    self.log_event("async", &format!("scoped models {n}"));
                }
            }
            AsyncKind::ExportDone => {
                let path = data
                    .get("path")
                    .and_then(|p| p.as_str())
                    .unwrap_or("(未知路径)");
                self.notice = Some((format!("✓ 已导出 HTML → {path}"), Color::Green));
                self.log_event("slash", &format!("export_html → {path}"));
            }
            AsyncKind::Stats => {
                if let Some(Overlay::Info {
                    lines,
                    loading,
                    seq: s,
                    ..
                }) = self.overlay.as_mut()
                    && *s == seq
                {
                    *lines = Self::session_info_lines(data, &self.state);
                    *loading = false;
                }
            }
            AsyncKind::SessionList => {
                if let Some(Overlay::SessionSelect {
                    items,
                    loading,
                    seq: s,
                    ..
                }) = self.overlay.as_mut()
                    && *s == seq
                {
                    *items = crate::app::sessions::parse_sessions_json(data);
                    *loading = false;
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
            current_session: self.session_file.as_deref(),
            local_queue: &self.compact_queue,
            completion: if self.completion.open {
                Some(&self.completion)
            } else {
                None
            },
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

    /// 假 pi：switch_session 返回 cancelled=false，随后 get_state / get_messages
    /// 返回可断言的固定数据（4.3 切换链）。
    fn chain_fake_pi(tag: &str, cancelled: bool) -> (pi_rust_tui::rpc::RpcClient, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "chain-pi-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let script = format!(
            r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
  cmd=$(printf '%s' "$line" | sed -n 's/.*"type":"\([^"]*\)".*/\1/p')
  case "$cmd" in
    switch_session|new_session|fork|clone)
      printf '{{"type":"response","id":"%s","success":true,"data":{{"cancelled":{cancelled}}}}}
' "$id" ;;
    get_state)
      printf '{{"type":"response","id":"%s","success":true,"data":{{"sessionFile":"/s/target.jsonl","model":{{"id":"m","name":"TargetModel"}},"thinkingLevel":"low","sessionId":"t"}}}}
' "$id" ;;
    get_messages)
      printf '{{"type":"response","id":"%s","success":true,"data":{{"messages":[{{"role":"system","content":""}},{{"role":"user","content":[{{"type":"text","text":"switched question"}}]}},{{"role":"assistant","content":[{{"type":"text","text":"switched answer"}}]}}]}}}}
' "$id" ;;
    *) printf '{{"type":"response","id":"%s","success":true}}
' "$id" ;;
  esac
done
"#,
            cancelled = cancelled
        );
        let path = dir.join("pi.sh");
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        // 保留一份供外部检查（测试结束后目录被删，这里另存）
        let _ = std::fs::write("/tmp/last-chain-pi.sh", std::fs::read(&path).unwrap_or_default());
        let client = pi_rust_tui::rpc::RpcClient::spawn(path.to_str().unwrap(), &[]).unwrap();
        (client, dir)
    }

    /// 从回报通道收集 AsyncData（按 kind 过滤，限时）。
    fn collect_async(
        rx: &std::sync::mpsc::Receiver<UiEvent>,
        kinds: &[AsyncKind],
        ms: u64,
    ) -> Vec<(AsyncKind, serde_json::Value)> {
        let mut got = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        while std::time::Instant::now() < deadline {
            if let Ok(UiEvent::AsyncData { kind, data, .. }) =
                rx.recv_timeout(std::time::Duration::from_millis(200))
                && kinds.contains(&kind)
            {
                let done = kind == AsyncKind::Messages;
                got.push((kind, data));
                if done {
                    break;
                }
            }
        }
        got
    }

    #[test]
    fn switch_chain_rebuilds_state_and_session_file() {
        let (client, dir) = chain_fake_pi("ok", false);
        let commander = client.command_sender();
        let (tx, rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();

        App::spawn_session_chain(
            &commander,
            &tx,
            serde_json::json!({"type": "switch_session", "sessionPath": "/s/old.jsonl"}),
            "switch_session",
            None,
        );
        let got = collect_async(
            &rx,
            &[AsyncKind::StateApplied, AsyncKind::Messages],
            4000,
        );
        assert!(got.iter().any(|(k, _)| *k == AsyncKind::StateApplied), "应有 StateApplied");
        let messages = got
            .iter()
            .find(|(k, _)| *k == AsyncKind::Messages)
            .map(|(_, d)| d.clone())
            .expect("应有 Messages");

        // 应用到 App
        for (kind, data) in &got {
            app.handle_async_data(*kind, 0, data);
        }
        assert_eq!(app.session_file.as_deref(), Some("/s/target.jsonl"), "session_file 刷新");
        assert_eq!(app.state.model.as_deref(), Some("TargetModel"), "模型刷新");
        // transcript 重建为切换后的会话
        let users: Vec<&str> = app
            .state
            .entries()
            .iter()
            .filter_map(|e| match e {
                pi_rust_tui::session::Entry::User { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(users, vec!["switched question"], "{:?}", app.state.entries());
        assert!(app.state.entries().iter().any(|e| matches!(
            e,
            pi_rust_tui::session::Entry::Assistant { .. }
        )));
        let _ = messages;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn switch_chain_cancelled_notifies_without_touching_state() {
        let (client, dir) = chain_fake_pi("cancel", true);
        let commander = client.command_sender();
        let (tx, rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();
        // 先有旧状态
        app.state.load_messages(&serde_json::json!({"messages": [
            {"role":"user","content":[{"type":"text","text":"before"}]}
        ]}));

        App::spawn_session_chain(
            &commander,
            &tx,
            serde_json::json!({"type": "switch_session", "sessionPath": "/s/x.jsonl"}),
            "switch_session",
            None,
        );
        // 等 SessionSwitched（不应有 Messages）
        let mut switched = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            if let Ok(ev) = rx.recv_timeout(std::time::Duration::from_millis(200)) {
                match ev {
                    UiEvent::AsyncData { kind: AsyncKind::SessionSwitched, data, .. } => {
                        assert_eq!(
                            data.get("command").and_then(|c| c.as_str()),
                            Some("switch_session")
                        );
                        switched = true;
                        break;
                    }
                    UiEvent::AsyncData { kind: AsyncKind::Messages, .. } => {
                        panic!("cancelled 后不应重建 Messages");
                    }
                    _ => {}
                }
            }
        }
        assert!(switched, "应收到 SessionSwitched");
        // 应用通知 → 黄色取消提示
        app.handle_async_data(
            AsyncKind::SessionSwitched,
            0,
            &serde_json::json!({"command":"switch_session"}),
        );
        let (text, color) = app.notice.clone().expect("cancel notice");
        assert!(text.contains("取消"), "{text}");
        assert_eq!(color, Color::Yellow);
        // 旧状态未动
        assert!(app.state.entries().iter().any(|e| matches!(
            e,
            pi_rust_tui::session::Entry::User { text } if text == "before"
        )));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resume_opens_selector_and_fills_via_async() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (client, dir) = test_commander();
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();

        // /resume 命中本地表 → 打开浮层（loading）
        assert!(crate::app::commands::dispatch(
            &mut app,
            &commander,
            &tx,
            "/resume"
        ));
        match &app.overlay {
            Some(Overlay::SessionSelect { loading, seq, confirming, .. }) => {
                assert!(*loading);
                assert!(*seq >= 1);
                assert!(!*confirming);
            }
            other => panic!("未打开会话选择器: {other:?}"),
        }

        // 键盘路径同样可用（Esc 关闭）
        let seq = match &app.overlay {
            Some(Overlay::SessionSelect { seq, .. }) => *seq,
            _ => 0,
        };
        app.overlay = Some(Overlay::SessionSelect {
            items: Vec::new(),
            cursor: 0,
            loading: true,
            seq,
            confirming: false,
        });
        app.handle_key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &commander,
            &tx,
        );
        // loading 中 Esc 可关闭（退出等待）
        assert!(app.overlay.is_none(), "loading 中 Esc 关闭浮层");

        // 重新打开以验证填充
        crate::app::commands::dispatch(&mut app, &commander, &tx, "/resume");
        let seq = match &app.overlay {
            Some(Overlay::SessionSelect { seq, .. }) => *seq,
            other => panic!("重开失败: {other:?}"),
        };

        // SessionList 填充（正确 seq）
        let data = serde_json::json!({"sessions":[
            {"path":"/s/a.jsonl","name":"alpha","firstMessage":"","modifiedMs":1700000000},
            {"path":"/s/b.jsonl","name":null,"firstMessage":"first ask","modifiedMs":1600000000}
        ]});
        app.handle_async_data(AsyncKind::SessionList, seq, &data);
        match &app.overlay {
            Some(Overlay::SessionSelect { items, loading, .. }) => {
                assert!(!*loading, "数据到达结束 loading");
                assert_eq!(items.len(), 2);
                assert_eq!(items[0].name.as_deref(), Some("alpha"));
                assert_eq!(items[1].first_message, "first ask");
            }
            other => panic!("{other:?}"),
        }
        // 过期 seq 丢弃
        app.handle_async_data(AsyncKind::SessionList, seq + 99, &serde_json::json!({"sessions":[]}));
        match &app.overlay {
            Some(Overlay::SessionSelect { items, .. }) => assert_eq!(items.len(), 2),
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_and_clone_dispatch_send_their_commands() {
        // 录制假 pi：断言 /new 与 /clone 发出正确命令类型（5.1）
        let (client, dir, log) = recording_commander("newclone", r#"{"steering":[],"followUp":[]}"#);
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();

        app.editor.replace("/new");
        assert!(crate::app::commands::dispatch(&mut app, &commander, &tx, "/new"));
        assert!(app.editor.is_empty(), "命令后输入清空");
        let content = read_log_until(&log, "new_session", 2000).expect("应发出 new_session");
        assert!(content.contains(r#""type":"new_session""#), "{content}");

        app.editor.replace("/clone");
        assert!(crate::app::commands::dispatch(&mut app, &commander, &tx, "/clone"));
        let content = read_log_until(&log, "clone", 2000).expect("应发出 clone");
        assert!(content.contains(r#""type":"clone""#), "{content}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tree_and_session_info_open_and_fill() {
        let (client, dir) = test_commander();
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();

        // /tree → loading 浮层
        assert!(crate::app::commands::dispatch(&mut app, &commander, &tx, "/tree"));
        let seq = match &app.overlay {
            Some(Overlay::Tree { loading, seq, .. }) => {
                assert!(*loading);
                *seq
            }
            other => panic!("未打开树: {other:?}"),
        };
        // get_tree 数据到达 → 行构建
        app.handle_async_data(
            AsyncKind::Tree,
            seq,
            &serde_json::json!({
                "leafId": "n2",
                "tree": [{
                    "entry": {"type":"message","id":"n1","message":{"role":"user","content":[{"type":"text","text":"root q"}]}},
                    "children": [{
                        "entry": {"type":"message","id":"n2","message":{"role":"assistant","content":[{"type":"text","text":"answer"}]}},
                        "children": []
                    }]
                }]
            }),
        );
        match &app.overlay {
            Some(Overlay::Tree { rows, loading, leaf_id, .. }) => {
                assert!(!*loading);
                assert_eq!(leaf_id.as_deref(), Some("n2"));
                assert_eq!(rows.len(), 2);
                assert!(rows[1].is_leaf);
            }
            other => panic!("{other:?}"),
        }
        // /session → Info 浮层 + stats 填充
        app.editor.replace("/session");
        assert!(crate::app::commands::dispatch(&mut app, &commander, &tx, "/session"));
        let seq = match &app.overlay {
            Some(Overlay::Info { loading, seq, .. }) => {
                assert!(*loading);
                *seq
            }
            other => panic!("未打开 info: {other:?}"),
        };
        app.handle_async_data(
            AsyncKind::Stats,
            seq,
            &serde_json::json!({
                "sessionId": "abc123",
                "userMessages": 3, "assistantMessages": 4, "toolCalls": 5,
                "totalMessages": 12,
                "tokens": {"total": 9876},
                "cost": 0.042
            }),
        );
        match &app.overlay {
            Some(Overlay::Info { lines, loading, .. }) => {
                assert!(!*loading);
                let joined = lines.join("\n");
                assert!(joined.contains("abc123"), "{joined}");
                assert!(joined.contains("9876"), "{joined}");
                assert!(joined.contains("$0.042"), "{joined}");
                assert!(joined.contains("3 用户"), "{joined}");
            }
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tree_and_info_overlays_close_on_esc() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (client, dir) = test_commander();
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();
        crate::app::commands::dispatch(&mut app, &commander, &tx, "/tree");
        assert!(app.overlay.is_some());
        app.handle_key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &commander,
            &tx,
        );
        // loading 中 Esc 也应关闭（overlay_key Info/Tree 的 Esc 臂）
        assert!(app.overlay.is_none(), "Esc 关闭树浮层");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_delete_two_stage_resets_confirming() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (client, dir) = test_commander();
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();

        // 打开选择器并填充一项
        crate::app::commands::dispatch(&mut app, &commander, &tx, "/resume");
        let seq = match &app.overlay {
            Some(Overlay::SessionSelect { seq, .. }) => *seq,
            other => panic!("{other:?}"),
        };
        app.handle_async_data(
            AsyncKind::SessionList,
            seq,
            &serde_json::json!({"sessions":[
                {"path":"/nonexistent/deletable.jsonl","name":"victim","firstMessage":"","modifiedMs":1}
            ]}),
        );
        let ctrl_d = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);

        // 第一次 ctrl+d → 确认态（不关窗、不删除）
        app.handle_key(ctrl_d, &commander, &tx);
        match &app.overlay {
            Some(Overlay::SessionSelect { confirming, items, .. }) => {
                assert!(*confirming, "进入确认态");
                assert_eq!(items.len(), 1);
            }
            other => panic!("{other:?}"),
        }

        // 第二次 ctrl+d → Delete 臂执行：确认态复位、浮层保持（等异步刷新）
        app.handle_key(ctrl_d, &commander, &tx);
        match &app.overlay {
            Some(Overlay::SessionSelect { confirming, .. }) => {
                assert!(!*confirming, "Delete 后确认态复位");
            }
            other => panic!("浮层应保持: {other:?}"),
        }

        // 删除线程对不存在路径失败 → 稍后出现红色通知（路径校验失败）
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut saw_fail = false;
        while std::time::Instant::now() < deadline && !saw_fail {
            if let Ok(UiEvent::SendFailed(msg)) =
                _rx.recv_timeout(std::time::Duration::from_millis(200))
            {
                saw_fail = msg.contains("删除失败");
            }
        }
        assert!(saw_fail, "不存在的路径删除应失败并通知");

        // Esc 退出浮层
        app.handle_key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &commander,
            &tx,
        );
        assert!(app.overlay.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn compact_command_sends_and_clears_input() {
        let (client, dir, log) = recording_commander("compact", r#"{"steering":[],"followUp":[]}"#);
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();
        app.editor.replace("/compact make it tiny");
        assert!(crate::app::commands::dispatch(&mut app, &commander, &tx, "/compact make it tiny"));
        assert!(app.editor.is_empty());
        let content = read_log_until(&log, "compact", 2000).expect("compact 应发出");
        assert!(content.contains(r#""type":"compact""#), "{content}");
        assert!(content.contains("make it tiny"), "customInstructions 透传: {content}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn submit_during_compaction_queues_locally() {
        let (client, dir, log) = recording_commander("cqueue", r#"{"steering":[],"followUp":[]}"#);
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();
        app.state.compacting = true;

        app.editor.replace("wait for compact");
        app.submit_as(&commander, &tx, "prompt");

        // 本地排队：输入清空、队列可见、通知黄字
        assert!(app.editor.is_empty());
        assert_eq!(app.compact_queue, vec!["wait for compact".to_string()]);
        let (text, color) = app.notice.clone().expect("排队通知");
        assert!(text.contains("压缩进行中"), "{text}");
        assert_eq!(color, Color::Yellow);

        // 压缩结束边沿补发：每条 prompt 带 steeringBehavior=steer
        app.state.compacting = false;
        app.flush_compact_queue(&commander, &tx);
        assert!(app.compact_queue.is_empty(), "补发后队列清空");
        let content = read_log_until(&log, "steer", 2000).expect("补发应带 steer");
        assert!(content.contains(r#""streamingBehavior":"steer""#), "{content}");
        assert!(content.contains("wait for compact"), "{content}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn compact_end_edge_without_queue_is_noop() {
        let (client, dir, _log) = recording_commander("cempty", r#"{"steering":[],"followUp":[]}"#);
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();
        app.flush_compact_queue(&commander, &tx); // 空队列不 panic、无副作用
        assert!(app.compact_queue.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stub_commands_intercept_locally_without_rpc() {
        // spec：未支持命令提示桩——拦截、黄条、不发任何 RPC
        let (client, dir, log) = recording_commander("stub", r#"{"steering":[],"followUp":[]}"#);
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();

        for (cmd, keyword) in [
            ("/login", "预登录"),
            ("/logout", "auth.json"),
            ("/share", "网络上报"),
            ("/bug", "网络上报"),
        ] {
            app.editor.replace(cmd);
            assert!(
                crate::app::commands::dispatch(&mut app, &commander, &tx, cmd),
                "{cmd} 应命中本地命令"
            );
            assert!(app.editor.is_empty(), "{cmd} 执行后输入清空");
            let (text, color) = app.notice.clone().unwrap_or_default();
            assert!(text.contains(keyword), "{cmd} 提示应含 {keyword}: {text}");
            assert_eq!(color, Color::Yellow, "{cmd} 应为黄条");
            assert!(!app.should_quit);
        }

        // 未发送任何 RPC：日志文件应始终为空
        std::thread::sleep(std::time::Duration::from_millis(600));
        let sent = std::fs::read_to_string(&log).unwrap_or_default();
        assert!(sent.trim().is_empty(), "桩命令不应发送 RPC，实际: {sent}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── 斜杠补全菜单（spec：菜单键位与应用） ──

    /// 逐字键入文本（每键走完整 handle_key 路径）。
    fn type_text(app: &mut App, cmd: &CommandSender, tx: &Sender<UiEvent>, text: &str) {
        for ch in text.chars() {
            app.handle_key(
                KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
                cmd,
                tx,
            );
        }
    }

    #[test]
    fn completion_menu_opens_on_slash_and_navigates() {
        let (client, dir, _log) = recording_commander("cmenu", r#"{"steering":[],"followUp":[]}"#);
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();

        type_text(&mut app, &commander, &tx, "/");
        assert!(app.completion.open, "行首 / 应开菜单");
        assert_eq!(app.completion.items.len(), 24, "应列出全部 24 个命令");
        assert_eq!(app.completion.selected, 0);

        // ↓/↑/翻页
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &commander, &tx);
        assert_eq!(app.completion.selected, 1);
        app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE), &commander, &tx);
        assert_eq!(app.completion.selected, 6);
        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &commander, &tx);
        assert_eq!(app.completion.selected, 5);
        // 边界夹取
        for _ in 0..30 {
            app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &commander, &tx);
        }
        assert_eq!(app.completion.selected, 0, "上边界夹取");

        // Esc 只关菜单、不清输入（spec）
        app.editor.replace("/th");
        app.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE), &commander, &tx);
        assert!(app.completion.open);
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &commander, &tx);
        assert!(!app.completion.open, "Esc 关菜单");
        assert_eq!(app.editor.text(), "/thi", "Esc 不清输入");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn completion_best_match_and_typing_refresh() {
        let (client, dir, _log) = recording_commander("cbest", r#"{"steering":[],"followUp":[]}"#);
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();

        type_text(&mut app, &commander, &tx, "/thi");
        assert!(app.completion.open);
        assert_eq!(app.completion.items[0].value, "thinking", "thi 首项应为 thinking");
        assert_eq!(app.completion.selected, 0, "预选最佳匹配");
        assert!(app.completion.items[0]
            .description
            .as_deref()
            .unwrap()
            .contains("<level>"), "说明列含参数提示");

        // 继续输入不存在的字符 → 无匹配收起
        for ch in "zz".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE), &commander, &tx);
        }
        assert!(!app.completion.open, "无匹配收起");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn completion_tab_applies_without_submit() {
        let (client, dir, log) = recording_commander("ctab", r#"{"steering":[],"followUp":[]}"#);
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();

        type_text(&mut app, &commander, &tx, "/resu");
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &commander, &tx);
        assert_eq!(app.editor.text(), "/resume ", "Tab 应用并补空格");
        assert_eq!(app.editor.col(), 8, "光标在空格后");
        assert!(!app.completion.open, "应用后关菜单");
        // Tab 不提交：无 prompt 发出
        std::thread::sleep(std::time::Duration::from_millis(400));
        let sent = std::fs::read_to_string(&log).unwrap_or_default();
        assert!(!sent.contains("prompt"), "Tab 不应发送: {sent}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn completion_enter_applies_then_submits() {
        let (client, dir, _log) = recording_commander("center", r#"{"steering":[],"followUp":[]}"#);
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();

        type_text(&mut app, &commander, &tx, "/resu");
        assert!(app.completion.open);
        // 回车 = 应用补全 + 立即提交（/resume → 打开会话选择器）
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &commander, &tx);
        assert!(
            matches!(app.overlay, Some(Overlay::SessionSelect { .. })),
            "提交 /resume 应打开会话选择器: {:?}",
            app.overlay
        );
        assert!(app.editor.is_empty(), "提交后输入清空");
        assert!(!app.completion.open);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn completion_closes_when_overlay_opens_and_non_slash() {
        let (client, dir, _log) = recording_commander("cclose", r#"{"steering":[],"followUp":[]}"#);
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();

        type_text(&mut app, &commander, &tx, "/");
        assert!(app.completion.open);
        // ctrl+l 打开模型选择器 → 菜单关闭
        app.handle_key(
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL),
            &commander,
            &tx,
        );
        assert!(app.overlay.is_some(), "ctrl+l 开浮层");
        assert!(!app.completion.open, "浮层打开时补全关闭");

        // 关浮层后输入普通文本 → 不触发
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &commander, &tx);
        type_text(&mut app, &commander, &tx, "hello");
        assert!(!app.completion.open, "普通文本不触发");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn completion_arg_lazy_fetch_filters_and_applies() {
        // 假 pi：按命令回不同数据（models / levels）
        let dir = std::env::temp_dir().join(format!(
            "argcache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("recv.log");
        let script = r#"#!/bin/sh
log="__LOG__"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$log"
  id=$(printf '%s' "$line" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
  case "$line" in
    *get_available_models*)
      printf '{"type":"response","id":"%s","success":true,"data":{"models":[{"provider":"openai","id":"gpt-5.4","name":"GPT-5.4"},{"provider":"anthropic","id":"claude-opus","name":"Opus"}]}}\n' "$id" ;;
    *get_available_thinking_levels*)
      printf '{"type":"response","id":"%s","success":true,"data":{"levels":["off","low","high"]}}\n' "$id" ;;
    *) printf '{"type":"response","id":"%s","success":true}\n' "$id" ;;
  esac
done
"#
        .replace("__LOG__", &log.display().to_string());
        let path = dir.join("pi.sh");
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let client = pi_rust_tui::rpc::RpcClient::spawn(path.to_str().unwrap(), &[]).unwrap();
        let commander = client.command_sender();
        let (tx, rx) = std::sync::mpsc::channel::<UiEvent>();

        let mut app = App::new();
        // 键入 "/model "（空格后触发参数补全；缓存空 → 先拉取、菜单暂不显示）
        for ch in "/model ".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE), &commander, &tx);
        }
        assert!(app.models_requested, "应触发一次模型拉取");
        assert!(!app.completion.open, "缓存到达前菜单未开");

        // 模拟事件循环：收到 ArgCache 数据 → 填缓存 → 重算
        let ev = rx.recv_timeout(std::time::Duration::from_secs(5)).expect("应收到 ArgCacheModels");
        match ev {
            UiEvent::AsyncData { kind, seq, data } => {
                assert_eq!(kind, AsyncKind::ArgCacheModels);
                app.handle_async_data(kind, seq, &data);
                app.refresh_completion();
            }
            other => panic!("意外事件: {other:?}"),
        }
        assert!(app.completion.open, "缓存到达后菜单应开");
        assert_eq!(app.completion.items.len(), 2, "全部模型候选");

        // 继续输入过滤
        for ch in "gpt".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE), &commander, &tx);
        }
        assert_eq!(app.completion.items.len(), 1);
        assert_eq!(app.completion.items[0].value, "openai/gpt-5.4");

        // Tab 应用参数（不加空格）
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &commander, &tx);
        assert_eq!(app.editor.text(), "/model openai/gpt-5.4");
        assert!(!app.completion.open);

        // /thinking 同理走级别缓存
        app.editor.replace("");
        for ch in "/thinking ".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE), &commander, &tx);
        }
        assert!(app.levels_requested, "应触发级别拉取");
        let ev = rx.recv_timeout(std::time::Duration::from_secs(5)).expect("应收到 ArgCacheLevels");
        if let UiEvent::AsyncData { kind, seq, data } = ev {
            assert_eq!(kind, AsyncKind::ArgCacheLevels);
            app.handle_async_data(kind, seq, &data);
            app.refresh_completion();
        }
        assert_eq!(app.completion.items.len(), 3, "级别候选 off/low/high");

        // 无参数补全的命令空格后不弹、不拉取
        app.editor.replace("");
        for ch in "/copy x".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE), &commander, &tx);
        }
        assert!(!app.completion.open, "/copy 空格后不弹菜单");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── /copy 剪贴板（spec：复制与信息展示命令） ──

    #[test]
    fn base64_and_osc52_sequence() {
        // 已知向量（含 padding 与多字节）
        assert_eq!(App::base64_encode(b""), "");
        assert_eq!(App::base64_encode(b"f"), "Zg==");
        assert_eq!(App::base64_encode(b"fo"), "Zm8=");
        assert_eq!(App::base64_encode(b"foo"), "Zm9v");
        assert_eq!(App::base64_encode("中".as_bytes()), "5Lit");
        let seq = App::osc52_sequence("hi");
        assert!(seq.starts_with("\x1b]52;c;"), "OSC52 头: {seq:?}");
        assert!(seq.ends_with("\x07"), "BEL 结尾: {seq:?}");
        assert!(seq.contains("aGk="), "hi 的 base64: {seq:?}");
    }

    #[test]
    fn copy_command_sends_and_empty_reply_is_red() {
        let (client, dir, log) = recording_commander("copy", r#"{"steering":[],"followUp":[]}"#);
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();

        // 分发 → 异步取最后回复
        app.editor.replace("/copy");
        assert!(crate::app::commands::dispatch(&mut app, &commander, &tx, "/copy"));
        assert!(app.editor.is_empty());
        let content = read_log_until(&log, "get_last_assistant_text", 2000).expect("应发出取文本命令");

        // 空回复 → 红条
        app.handle_async_data(
            AsyncKind::CopyText,
            0,
            &serde_json::json!({"text": ""}),
        );
        let (text, color) = app.notice.clone().expect("空回复应有通知");
        assert!(text.contains("暂无可复制"), "{text}");
        assert_eq!(color, Color::Red);
        let _ = content;
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── /hotkeys 键位表浮层（spec：复制与信息展示命令） ──

    #[test]
    fn hotkeys_command_opens_overlay_and_esc_closes() {
        let (client, dir, _log) = recording_commander("hotkeys", r#"{"steering":[],"followUp":[]}"#);
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();

        app.editor.replace("/hotkeys");
        assert!(crate::app::commands::dispatch(&mut app, &commander, &tx, "/hotkeys"));
        assert!(matches!(app.overlay, Some(Overlay::Hotkeys { .. })), "应打开键位表");
        assert!(app.editor.is_empty());

        // ↑↓ 滚动
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &commander, &tx);
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &commander, &tx);
        match &app.overlay {
            Some(Overlay::Hotkeys { cursor }) => assert_eq!(*cursor, 2),
            other => panic!("{other:?}"),
        }
        // Esc / Enter 关闭
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &commander, &tx);
        assert!(app.overlay.is_none(), "Esc 关闭");

        // 回车也关闭
        crate::app::commands::dispatch(&mut app, &commander, &tx, "/hotkeys");
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &commander, &tx);
        assert!(app.overlay.is_none(), "回车关闭");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hotkeys_rows_cover_real_bindings() {
        // 数据自检：关键绑定都在表里（防文档与实现漂移）
        let rows = crate::app::overlay::hotkeys_rows();
        let text: String = rows.iter().map(|(_, k, d)| format!("{k} {d}\n")).collect();
        for needle in ["enter", "alt+enter", "ctrl+c", "ctrl+d", "shift+enter", "ctrl+l", "alt+t", "shift+tab", "tab", "/ + 输入"] {
            assert!(text.contains(needle), "键位表缺 {needle}");
        }
        assert!(text.contains("Docs/slash-commands.md"), "应指引完整命令文档");
    }

    // ── /changelog（spec：复制与信息展示命令） ──

    #[test]
    fn newest_changelog_extracts_first_section() {
        let content = "# Changelog\n\n## 1.2.0 - newest\n\n- fix a\n- feat b\n\n## 1.1.0 - older\n\n- old item\n";
        let lines = App::newest_changelog_lines(content);
        assert_eq!(lines[0], "1.2.0 - newest", "取最新小节标题: {lines:?}");
        assert!(lines.iter().any(|l| l.contains("feat b")), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains("old item")), "不含旧版本: {lines:?}");

        // 无 ## 结构 → 退化前 60 行
        let plain = "line1\nline2";
        let out = App::newest_changelog_lines(plain);
        assert_eq!(out, vec!["line1".to_string(), "line2".to_string()]);

        // 超长截断
        let long: String = "x\n".repeat(80);
        let out = App::newest_changelog_lines(&long);
        assert!(out.len() <= 61, "截断后行数: {}", out.len());
        assert!(out.last().unwrap().contains("截断"));
    }

    #[test]
    fn changelog_command_renders_into_transcript_or_warns() {
        let (client, dir, _log) = recording_commander("changelog", r#"{"steering":[],"followUp":[]}"#);
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();

        // 用 env 覆盖指向临时文件（set_var 在 Rust 2024 为 unsafe）
        let tmp = std::env::temp_dir().join(format!("changelog-test-{}.md", std::process::id()));
        std::fs::write(&tmp, "# Changelog\n\n## 9.9.9 - test\n\n- hello change\n").unwrap();
        unsafe { std::env::set_var("PI_RUST_TUI_CHANGELOG", &tmp) };

        let before = app.state.transcript.len();
        app.cmd_changelog();
        assert!(app.state.transcript.len() > before, "应追加 changelog 条目");
        let text: String = format!("{:?}", &app.state.transcript[before..]);
        assert!(text.contains("9.9.9"), "应含最新版本: {text}");
        assert!(text.contains("hello change"), "应含条目内容: {text}");
        let (notice, color) = app.notice.clone().unwrap();
        assert!(notice.contains("changelog"), "{notice}");
        assert_eq!(color, Color::Green);

        // 命令分发路径
        app.editor.replace("/changelog");
        assert!(crate::app::commands::dispatch(&mut app, &commander, &tx, "/changelog"));
        assert!(app.editor.is_empty());

        unsafe { std::env::remove_var("PI_RUST_TUI_CHANGELOG") };
        let _ = std::fs::remove_file(&tmp);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── /trust 信任记录（对照 pi trust.json 格式） ──

    #[test]
    fn trust_roundtrip_and_overlay_flow() {
        let home = std::env::temp_dir().join(format!("trust-home-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let trust_path = home.join(".pi/agent/trust.json");
        unsafe { std::env::set_var("PI_RUST_TUI_TRUST_FILE", &trust_path) };

        let path = App::trust_file_path();
        assert_eq!(path, trust_path, "env 覆盖生效");

        // 缺失 → 空
        let map = App::read_trust_file(&path);
        assert!(map.is_empty());

        // 写入 → 读回（格式：{ "<cwd>": true } 纯对象）
        let mut map = serde_json::Map::new();
        map.insert("/some/project".into(), serde_json::Value::Bool(true));
        App::write_trust_file(&path, &map).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["/some/project"], serde_json::Value::Bool(true), "格式对齐 pi: {text}");
        let back = App::read_trust_file(&path);
        assert_eq!(back.get("/some/project"), Some(&serde_json::Value::Bool(true)));

        // 清除
        let mut map = back;
        map.remove("/some/project");
        App::write_trust_file(&path, &map).unwrap();
        assert!(App::read_trust_file(&path).is_empty(), "清除后为空");

        // 浮层流程：打开（未记录）→ Enter 信任 → 文件记录 → 重开显示已信任 → 清除
        let (client, dir, _log) = recording_commander("trust", r#"{"steering":[],"followUp":[]}"#);
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();

        app.editor.replace("/trust");
        assert!(crate::app::commands::dispatch(&mut app, &commander, &tx, "/trust"));
        assert!(app.editor.is_empty());
        match &app.overlay {
            Some(Overlay::Trust { current: None, .. }) => {}
            other => panic!("应显示未记录: {other:?}"),
        }
        // Enter → 应用 idx0（信任）
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &commander, &tx);
        assert!(app.overlay.is_none(), "应用后关闭");
        let cwd = App::current_cwd_key();
        let map = App::read_trust_file(&path);
        assert_eq!(map.get(&cwd), Some(&serde_json::Value::Bool(true)), "cwd 应被信任");
        let (notice, color) = app.notice.clone().unwrap();
        assert!(notice.contains("已信任"), "{notice}");
        assert_eq!(color, Color::Green);

        // 重开 → Some(true)；下移到清除 → Enter
        crate::app::commands::dispatch(&mut app, &commander, &tx, "/trust");
        match &app.overlay {
            Some(Overlay::Trust { current: Some(true), .. }) => {}
            other => panic!("应显示已信任: {other:?}"),
        }
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &commander, &tx);
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &commander, &tx);
        let map = App::read_trust_file(&path);
        assert!(map.get(&cwd).is_none(), "清除后无记录: {map:?}");

        unsafe { std::env::remove_var("PI_RUST_TUI_TRUST_FILE") };
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── /reload（spec：配置重读） ──

    #[test]
    fn reload_refuses_when_streaming_or_compacting() {
        let (client, dir, _log) = recording_commander("reload1", r#"{"steering":[],"followUp":[]}"#);
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();

        // 流式 → 黄条拒绝
        app.state.phase = Phase::Streaming;
        assert!(crate::app::commands::dispatch(&mut app, &commander, &tx, "/reload"));
        let (text, color) = app.notice.clone().unwrap();
        assert!(text.contains("当前响应"), "{text}");
        assert_eq!(color, Color::Yellow);
        assert_eq!(app.state.phase, Phase::Streaming, "流式不受影响");

        // 压缩中 → 黄条拒绝
        app.state.phase = Phase::Idle;
        app.state.compacting = true;
        crate::app::commands::dispatch(&mut app, &commander, &tx, "/reload");
        let (text, color) = app.notice.clone().unwrap();
        assert!(text.contains("压缩"), "{text}");
        assert_eq!(color, Color::Yellow);

        // 空闲 → 绿条 + 缓存失效
        app.state.compacting = false;
        app.models_requested = true;
        app.levels_requested = true;
        app.models_cache.push("x/y".into());
        crate::app::commands::dispatch(&mut app, &commander, &tx, "/reload");
        let (text, color) = app.notice.clone().unwrap();
        assert!(text.contains("已重新加载"), "{text}");
        assert_eq!(color, Color::Green);
        assert!(app.models_cache.is_empty() && app.levels_cache.is_empty(), "缓存失效");
        assert!(!app.models_requested && !app.levels_requested, "允许重拉");
        assert!(app.editor.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── /model /thinking 入口与精确匹配（spec） ──

    /// 造一个支持 model/thinking 全分支的假 pi。
    fn selector_fake_pi(tag: &str) -> (pi_rust_tui::rpc::RpcClient, std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "sel-{}-{}",
            tag,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("recv.log");
        let script = r#"#!/bin/sh
log="__LOG__"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$log"
  id=$(printf '%s' "$line" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
  case "$line" in
    *get_available_models*)
      printf '{"type":"response","id":"%s","success":true,"data":{"models":[{"provider":"openai","id":"gpt-5.4","name":"GPT"},{"provider":"anthropic","id":"claude-opus","name":"Opus"}]}}\n' "$id" ;;
    *get_available_thinking_levels*)
      printf '{"type":"response","id":"%s","success":true,"data":{"levels":["off","low","high"]}}\n' "$id" ;;
    *) printf '{"type":"response","id":"%s","success":true}\n' "$id" ;;
  esac
done
"#
        .replace("__LOG__", &log.display().to_string());
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

    #[test]
    fn model_command_opens_selector_without_args() {
        let (client, dir, _log) = selector_fake_pi("noargs");
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();
        app.editor.replace("/model");
        assert!(crate::app::commands::dispatch(&mut app, &commander, &tx, "/model"));
        assert!(
            matches!(app.overlay, Some(Overlay::ModelSelect { loading: true, .. })),
            "无参应打开选择器: {:?}",
            app.overlay
        );
        assert!(app.editor.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn model_command_exact_match_sets_model() {
        let (client, dir, log) = selector_fake_pi("exact");
        let commander = client.command_sender();
        let (tx, rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();

        // id 精确匹配（大小写不敏感）
        app.editor.replace("/model GPT-5.4");
        assert!(crate::app::commands::dispatch(&mut app, &commander, &tx, "/model GPT-5.4"));

        // 等待 ModelApplied + Info
        let mut saw_model = false;
        let mut saw_info = false;
        for _ in 0..10 {
            match rx.recv_timeout(std::time::Duration::from_millis(500)) {
                Ok(UiEvent::AsyncData { kind: AsyncKind::ModelApplied, seq, data }) => {
                    app.handle_async_data(AsyncKind::ModelApplied, seq, &data);
                    saw_model = true;
                }
                Ok(UiEvent::Info(text)) => {
                    assert!(text.contains("Model: openai/gpt-5.4"), "{text}");
                    saw_info = true;
                }
                Ok(_) => {}
                Err(_) => {}
            }
            if saw_model && saw_info {
                break;
            }
        }
        assert!(saw_model && saw_info, "应收到模型回填与提示");
        assert_eq!(app.state.model.as_deref(), Some("openai/gpt-5.4"), "状态行更新");
        let content = read_log_until(&log, "set_model", 1000).expect("应发出 set_model");
        assert!(content.contains(r#""provider":"openai""#), "{content}");

        // provider/id 形式也精确命中
        app.editor.replace("/model anthropic/claude-opus");
        crate::app::commands::dispatch(&mut app, &commander, &tx, "/model anthropic/claude-opus");
        let content = read_log_until(&log, "anthropic", 2000).expect("第二次 set_model");
        assert!(content.contains("claude-opus"), "{content}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn model_command_no_match_falls_back_to_selector() {
        let (client, dir, _log) = selector_fake_pi("nomatch");
        let commander = client.command_sender();
        let (tx, rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();

        crate::app::commands::dispatch(&mut app, &commander, &tx, "/model no-such-model");
        let mut saw_open = false;
        for _ in 0..10 {
            match rx.recv_timeout(std::time::Duration::from_millis(500)) {
                Ok(UiEvent::Info(text)) => {
                    assert!(text.contains("未找到"), "{text}");
                }
                Ok(UiEvent::OpenModelSelect) => {
                    // 模拟事件循环：打开选择器
                    app.open_model_select(&commander, &tx);
                    saw_open = true;
                    break;
                }
                Ok(_) => {}
                Err(_) => {}
            }
        }
        assert!(saw_open, "无匹配应回退打开选择器");
        assert!(matches!(app.overlay, Some(Overlay::ModelSelect { .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn thinking_command_applies_or_reports_unknown_level() {
        let (client, dir, log) = selector_fake_pi("think");
        let commander = client.command_sender();
        let (tx, rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();

        // 精确匹配应用
        crate::app::commands::dispatch(&mut app, &commander, &tx, "/thinking HIGH");
        let mut applied = false;
        let mut info = false;
        for _ in 0..10 {
            match rx.recv_timeout(std::time::Duration::from_millis(500)) {
                Ok(UiEvent::AsyncData { kind: AsyncKind::ThinkingApplied, seq, data }) => {
                    app.handle_async_data(AsyncKind::ThinkingApplied, seq, &data);
                    applied = true;
                }
                Ok(UiEvent::Info(text)) => {
                    assert!(text.contains("thinking: high"), "{text}");
                    info = true;
                }
                Ok(_) => {}
                Err(_) => {}
            }
            if applied && info {
                break;
            }
        }
        assert!(applied && info, "应应用级别");
        assert_eq!(app.state.thinking_level.as_deref(), Some("high"), "状态行更新");
        let content = read_log_until(&log, "set_thinking_level", 1000).expect("应发出 set_thinking_level");
        assert!(content.contains(r#""level":"high""#), "{content}");

        // 未知级别 → 红条列出可用项
        crate::app::commands::dispatch(&mut app, &commander, &tx, "/thinking bogus");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut err = None;
        while std::time::Instant::now() < deadline && err.is_none() {
            if let Ok(UiEvent::SendFailed(msg)) = rx.recv_timeout(std::time::Duration::from_millis(300)) {
                err = Some(msg);
            }
        }
        let err = err.expect("未知级别应红条");
        assert!(err.contains("Unknown thinking level"), "{err}");
        assert!(err.contains("off") && err.contains("high"), "应列出可用级别: {err}");

        // 无参 → 打开级别列表
        crate::app::commands::dispatch(&mut app, &commander, &tx, "/thinking");
        assert!(
            matches!(app.overlay, Some(Overlay::ThinkingSelect { loading: true, .. })),
            "{:?}",
            app.overlay
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── /settings 简化浮层（spec：简化设置与模型范围展示） ──

    #[test]
    fn settings_opens_toggles_and_closes() {
        let (client, dir, log) = recording_commander("settings", r#"{"steering":[],"followUp":[]}"#);
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();

        // 打开 → loading + get_state 拉取
        app.editor.replace("/settings");
        assert!(crate::app::commands::dispatch(&mut app, &commander, &tx, "/settings"));
        assert!(app.editor.is_empty());
        match &app.overlay {
            Some(Overlay::Settings { loading: true, .. }) => {}
            other => panic!("{other:?}"),
        }
        read_log_until(&log, "get_state", 2000).expect("应拉 get_state");

        // 数据回填（模拟事件循环处理）
        let seq = match &app.overlay {
            Some(Overlay::Settings { seq, .. }) => *seq,
            other => panic!("{other:?}"),
        };
        app.handle_async_data(
            AsyncKind::SettingsState,
            seq,
            &serde_json::json!({"autoCompactionEnabled": true, "model": "openai/gpt-5.4"}),
        );
        match &app.overlay {
            Some(Overlay::Settings {
                loading: false,
                auto_compaction: Some(true),
                model,
                ..
            }) => {
                assert_eq!(model.as_deref(), Some("openai/gpt-5.4"));
            }
            other => panic!("{other:?}"),
        }

        // Enter 第 0 行 → set_auto_compaction(false 翻转)
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &commander, &tx);
        let content = read_log_until(&log, "set_auto_compaction", 2000).expect("应发送开关");
        assert!(content.contains(r#""enabled":false"#), "{content}");

        // 下移到 autoRetry + Enter → set_auto_retry
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &commander, &tx);
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &commander, &tx);
        let content = read_log_until(&log, "set_auto_retry", 2000).expect("应发送 retry 开关");
        assert!(content.contains(r#""enabled":true"#), "{content}");

        // Esc 关闭
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &commander, &tx);
        assert!(app.overlay.is_none(), "Esc 关闭");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scoped_models_shows_readonly_list() {
        let (client, dir, _log) = selector_fake_pi("scoped");
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();

        app.editor.replace("/scoped-models");
        assert!(crate::app::commands::dispatch(&mut app, &commander, &tx, "/scoped-models"));
        // 回填可用模型
        let seq = match &app.overlay {
            Some(Overlay::ScopedModels { seq, .. }) => *seq,
            other => panic!("{other:?}"),
        };
        app.handle_async_data(
            AsyncKind::ScopedModels,
            seq,
            &serde_json::json!({"models":[{"provider":"openai","id":"gpt-5.4"},{"provider":"anthropic","id":"claude-opus"}]}),
        );
        match &app.overlay {
            Some(Overlay::ScopedModels { models, loading, .. }) => {
                assert!(!*loading);
                assert_eq!(models.len(), 2);
            }
            other => panic!("{other:?}"),
        }
        // 只读：↑↓/Esc 不发送任何命令（这里仅断言浮层仍在、Esc 关闭）
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &commander, &tx);
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &commander, &tx);
        assert!(app.overlay.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── /export /import（spec：会话导出 / 会话导入） ──

    #[test]
    fn export_jsonl_copies_and_html_uses_rpc() {
        let (client, dir, log) = recording_commander("export", r#"{"steering":[],"path":"/tmp/x.html"}"#);
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();

        // 无会话文件 → 红条
        app.editor.replace("/export");
        assert!(crate::app::commands::dispatch(&mut app, &commander, &tx, "/export"));
        let (text, color) = app.notice.clone().unwrap();
        assert!(text.contains("无当前会话文件"), "{text}");
        assert_eq!(color, Color::Red);

        // 默认路径纯函数
        let cwd = std::path::Path::new("/tmp");
        assert_eq!(
            App::default_export_path("/x/1712345678_abcDEF123.jsonl", cwd),
            std::path::Path::new("/tmp/session-abcDEF123.jsonl")
        );
        assert_eq!(
            App::default_export_path("/x/no-underscore.jsonl", cwd),
            std::path::Path::new("/tmp/session-export.jsonl")
        );

        // 指定 .jsonl → 本地复制
        let src = dir.join("src.jsonl");
        std::fs::write(&src, "{\"type\":\"message\"}\n").unwrap();
        let dst = dir.join("out.jsonl");
        app.session_file = Some(src.to_string_lossy().to_string());
        app.editor.replace(&format!("/export {}", dst.display()));
        assert!(crate::app::commands::dispatch(
            &mut app,
            &commander,
            &tx,
            &format!("/export {}", dst.display())
        ));
        let copied = std::fs::read_to_string(&dst).expect("应产出文件");
        assert!(copied.contains("message"), "{copied}");
        let (text, color) = app.notice.clone().unwrap();
        assert!(text.contains("已导出"), "{text}");
        assert_eq!(color, Color::Green);

        // 非 .jsonl → export_html RPC
        app.editor.replace("/export /tmp/out.html");
        assert!(crate::app::commands::dispatch(&mut app, &commander, &tx, "/export /tmp/out.html"));
        let content = read_log_until(&log, "export_html", 2000).expect("应发 export_html");
        assert!(content.contains("/tmp/out.html"), "{content}");

        // ExportDone → 绿条路径提示
        app.handle_async_data(
            AsyncKind::ExportDone,
            0,
            &serde_json::json!({"path": "/tmp/out.html"}),
        );
        let (text, color) = app.notice.clone().unwrap();
        assert!(text.contains("/tmp/out.html"), "{text}");
        assert_eq!(color, Color::Green);

        // 不可写目标 → 红条
        app.editor.replace("/export /nonexistent-dir/x.jsonl");
        crate::app::commands::dispatch(&mut app, &commander, &tx, "/export /nonexistent-dir/x.jsonl");
        let (text, color) = app.notice.clone().unwrap();
        assert!(text.contains("Failed to export"), "{text}");
        assert_eq!(color, Color::Red);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_validates_then_confirms_then_switches() {
        let (client, dir, log) = recording_commander("import", r#"{"cancelled":false}"#);
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();

        // 无参 → 用法提示
        app.editor.replace("/import");
        assert!(crate::app::commands::dispatch(&mut app, &commander, &tx, "/import"));
        let (text, color) = app.notice.clone().unwrap();
        assert!(text.contains("Usage"), "{text}");
        assert_eq!(color, Color::Yellow);
        assert!(app.overlay.is_none());

        // 源不存在 → 红条不开浮层
        crate::app::commands::dispatch(&mut app, &commander, &tx, "/import /no/such.jsonl");
        let (text, color) = app.notice.clone().unwrap();
        assert!(text.contains("不存在"), "{text}");
        assert_eq!(color, Color::Red);
        assert!(app.overlay.is_none(), "失败不开确认");

        // 合法源 → 确认浮层；取消不复制不切换
        let src = dir.join("import-src.jsonl");
        std::fs::write(
            &src,
            "{\"type\":\"session\",\"cwd\":\"/tmp\"}\n{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"hi\"}]}}\n",
        )
        .unwrap();
        crate::app::commands::dispatch(
            &mut app,
            &commander,
            &tx,
            &format!("/import {}", src.display()),
        );
        assert!(
            matches!(app.overlay, Some(Overlay::ImportConfirm { cursor: 0, .. })),
            "{:?}",
            app.overlay
        );
        // 下移到"取消"回车 → 取消提示、无 RPC
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &commander, &tx);
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &commander, &tx);
        assert!(app.overlay.is_none(), "取消后关闭");
        let (text, color) = app.notice.clone().unwrap();
        assert!(text.contains("cancelled"), "{text}");
        assert_eq!(color, Color::Yellow);
        std::thread::sleep(std::time::Duration::from_millis(300));
        let sent = std::fs::read_to_string(&log).unwrap_or_default();
        assert!(!sent.contains("switch_session"), "取消不切换: {sent}");

        // 重新确认 → 复制进项目会话目录 + switch_session
        crate::app::commands::dispatch(
            &mut app,
            &commander,
            &tx,
            &format!("/import {}", src.display()),
        );
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &commander, &tx);
        assert!(app.overlay.is_none(), "确认后关闭");
        let content = read_log_until(&log, "switch_session", 3000).expect("应发 switch_session");
        assert!(content.contains("import-src.jsonl"), "目标为复制件: {content}");
        let (dest_dir, _) = crate::app::sessions::project_sessions_dir();
        assert!(dest_dir.join("import-src.jsonl").is_file(), "文件已复制进项目目录");
        let _ = std::fs::remove_file(dest_dir.join("import-src.jsonl"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── doubleEsc 设置（design D3/D5：文件真源 + 浮层第三行） ──

    #[test]
    fn settings_overlay_reads_double_esc_initial_value() {
        let dir = std::env::temp_dir().join(format!("de-init-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        let (client, cdir, _log) = recording_commander("de-init", r#"{"steering":[],"followUp":[]}"#);
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();

        // 文件缺失 → tree（默认）；路径经字段注入（无全局 env 竞态）
        app.settings_path = Some(path.clone());
        app.open_settings(&commander, &tx);
        match &app.overlay {
            Some(Overlay::Settings { double_esc: Some(v), .. }) => assert_eq!(v, "tree"),
            other => panic!("{other:?}"),
        }
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &commander, &tx);

        // 文件为 none → 初值 none（临时路径注入）
        std::fs::write(&path, r#"{"doubleEscapeAction":"none"}"#).unwrap();
        app.open_settings(&commander, &tx);
        match &app.overlay {
            Some(Overlay::Settings { double_esc: Some(v), .. }) => assert_eq!(v, "none"),
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&cdir);
    }

    #[test]
    fn settings_double_esc_row_renders_and_cursor_clamps() {
        let dir = std::env::temp_dir().join(format!("de-render-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, r#"{"doubleEscapeAction":"fork"}"#).unwrap();

        let (client, cdir, _log) = recording_commander("de-render", r#"{"steering":[],"followUp":[]}"#);
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();
        // 直接构造已回填的浮层（渲染不依赖异步）
        app.settings_path = Some(path.clone());
        app.overlay = Some(Overlay::Settings {
            cursor: 0,
            loading: false,
            seq: 1,
            auto_compaction: Some(true),
            auto_retry: None,
            double_esc: Some("fork".into()),
            model: Some("openai/gpt-5.4".into()),
        });

        // 渲染断言：浮层含 doubleEsc 行与当前值
        let state = pi_rust_tui::session::SessionState::new();
        let backend = ratatui::backend::TestBackend::new(90, 30);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let ui = crate::app::view::ViewUi {
                    overlay: app.overlay.as_ref(),
                    ..Default::default()
                };
                crate::app::view::render(frame, &state, &ui);
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                text.push_str(buf[(x, y)].symbol());
            }
            text.push('\n');
        }
        assert!(text.contains("doubleEsc"), "浮层应含 doubleEsc 行:\n{text}");
        assert!(text.contains("fork"), "应显示当前值: {text}");

        // 光标下移夹取到 2（模型/提示行只读）
        for _ in 0..5 {
            app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &commander, &tx);
        }
        match &app.overlay {
            Some(Overlay::Settings { cursor, .. }) => assert_eq!(*cursor, 2, "夹取到 doubleEsc 行"),
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&cdir);
    }

    #[test]
    fn settings_double_esc_cycles_and_writes_back() {
        let dir = std::env::temp_dir().join(format!("de-cycle-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        // 预置含其它键的设置文件（验证保留）
        std::fs::write(&path, r#"{"sessionDir":"/keep/me"}"#).unwrap();

        let (client, cdir, _log) = recording_commander("de-cycle", r#"{"steering":[],"followUp":[]}"#);
        let commander = client.command_sender();
        let (tx, rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();
        app.settings_path = Some(path.clone());
        app.open_settings(&commander, &tx);
        // 泵掉 get_state 响应 → 浮层退出 loading
        let ev = rx.recv_timeout(std::time::Duration::from_secs(5)).expect("SettingsState");
        if let UiEvent::AsyncData { kind, seq, data } = ev {
            assert_eq!(kind, AsyncKind::SettingsState);
            app.handle_async_data(kind, seq, &data);
        }

        // 移到 doubleEsc 行（初始 tree）→ 三轮 Enter：fork → none → tree
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &commander, &tx);
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &commander, &tx);
        let expect = [("fork", "fork"), ("none", "none"), ("tree", "tree")];
        for (file_val, row_val) in expect {
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &commander, &tx);
            let text = std::fs::read_to_string(&path).unwrap();
            let v: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(v["doubleEscapeAction"], file_val, "文件值: {text}");
            assert_eq!(v["sessionDir"], "/keep/me", "其它键保留: {text}");
            match &app.overlay {
                Some(Overlay::Settings { double_esc: Some(row), .. }) => {
                    assert_eq!(row, row_val, "行值即时更新");
                }
                other => panic!("{other:?}"),
            }
        }
        assert!(app.overlay.is_some(), "切换后浮层保持打开（Toggle 语义）");

        // 损坏文件 → 红条 + 行值回读（不假更新）
        std::fs::write(&path, "{broken").unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &commander, &tx);
        let (notice, color) = app.notice.clone().expect("应有通知");
        assert!(notice.contains("写入失败"), "{notice}");
        assert_eq!(color, Color::Red);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{broken",
            "损坏文件不被覆盖"
        );

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&cdir);
    }

    // ── 双击 Esc（spec：双击 Esc 动作） ──

    /// 造一个带注入设置路径的 App 与假 pi 通道。
    fn double_escape_ctx(
        action: &str,
    ) -> (App, CommandSender, Sender<UiEvent>, std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "dbl-esc-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let settings = dir.join("settings.json");
        std::fs::write(
            &settings,
            format!(r#"{{"doubleEscapeAction":"{action}"}}"#),
        )
        .unwrap();
        let (client, cdir, _log) = recording_commander("dbl", r#"{"steering":[],"followUp":[]}"#);
        let commander = client.command_sender();
        let (tx, _rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut app = App::new();
        app.settings_path = Some(settings);
        let _ = cdir;
        (app, commander, tx, dir, std::path::PathBuf::new())
    }

    fn press_esc(app: &mut App, cmd: &CommandSender, tx: &Sender<UiEvent>) {
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), cmd, tx);
    }

    #[test]
    fn double_escape_opens_tree_within_window() {
        let (mut app, cmd, tx, dir, _) = double_escape_ctx("tree");
        // 首次：无动作、记录时间戳
        press_esc(&mut app, &cmd, &tx);
        assert!(app.overlay.is_none(), "单次 Esc 无动作");
        assert!(app.last_escape.is_some(), "记录时间戳");
        // 50ms 内第二次 → 开树
        std::thread::sleep(std::time::Duration::from_millis(50));
        press_esc(&mut app, &cmd, &tx);
        assert!(
            matches!(app.overlay, Some(Overlay::Tree { .. })),
            "双击开树: {:?}",
            app.overlay
        );
        assert!(app.last_escape.is_none(), "触发后时间戳复位");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn double_escape_timeout_does_not_trigger() {
        let (mut app, cmd, tx, dir, _) = double_escape_ctx("tree");
        press_esc(&mut app, &cmd, &tx);
        // 超过 500ms（取 650ms，4 倍余量内仍远超窗口）
        std::thread::sleep(std::time::Duration::from_millis(650));
        press_esc(&mut app, &cmd, &tx);
        assert!(app.overlay.is_none(), "超时不触发");
        assert!(app.last_escape.is_some(), "第二次作为新的首击");
        // 紧接着第三次（在新窗口内）→ 触发
        std::thread::sleep(std::time::Duration::from_millis(50));
        press_esc(&mut app, &cmd, &tx);
        assert!(matches!(app.overlay, Some(Overlay::Tree { .. })), "新窗口内触发");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn double_escape_fork_maps_to_tree_and_none_disables() {
        // fork → 树（design D4 简化）
        let (mut app, cmd, tx, dir, _) = double_escape_ctx("fork");
        press_esc(&mut app, &cmd, &tx);
        std::thread::sleep(std::time::Duration::from_millis(50));
        press_esc(&mut app, &cmd, &tx);
        assert!(
            matches!(app.overlay, Some(Overlay::Tree { .. })),
            "fork 动作也开树: {:?}",
            app.overlay
        );
        let _ = std::fs::remove_dir_all(&dir);

        // none → 完全不动作（也不追踪窗口）
        let (mut app, cmd, tx, dir, _) = double_escape_ctx("none");
        press_esc(&mut app, &cmd, &tx);
        assert!(app.overlay.is_none());
        assert!(app.last_escape.is_none(), "none 不追踪窗口");
        std::thread::sleep(std::time::Duration::from_millis(50));
        press_esc(&mut app, &cmd, &tx);
        assert!(app.overlay.is_none(), "none 双击也不动作");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn double_escape_requires_empty_input_and_skips_nonempty_refresh() {
        let (mut app, cmd, tx, dir, _) = double_escape_ctx("tree");
        // 非空输入：双击不触发
        app.editor.replace("draft text");
        press_esc(&mut app, &cmd, &tx);
        std::thread::sleep(std::time::Duration::from_millis(50));
        press_esc(&mut app, &cmd, &tx);
        assert!(app.overlay.is_none(), "非空输入不触发");
        assert!(app.last_escape.is_none(), "非空 Esc 不记录窗口");

        // 空输入首击 → 带文本 Esc（不刷新）→ 清空后窗口内再击 → 触发
        app.editor.replace("");
        press_esc(&mut app, &cmd, &tx);
        assert!(app.last_escape.is_some());
        app.editor.replace("typed in between");
        press_esc(&mut app, &cmd, &tx); // 非空：不刷新时间戳
        app.editor.replace("");
        std::thread::sleep(std::time::Duration::from_millis(50));
        press_esc(&mut app, &cmd, &tx);
        assert!(
            matches!(app.overlay, Some(Overlay::Tree { .. })),
            "中间非空 Esc 不应刷新窗口: {:?}",
            app.overlay
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn escape_priority_streaming_overlay_completion_not_counted() {
        let (mut app, cmd, tx, dir, _) = double_escape_ctx("tree");

        // 1) 流式 Esc → Abort，不记录窗口
        app.state.phase = Phase::Streaming;
        app.editor.replace("");
        press_esc(&mut app, &cmd, &tx);
        assert!(app.last_escape.is_none(), "流式 Esc 不进双击窗口");
        app.state.phase = Phase::Idle;

        // 2) 浮层 Esc → 关浮层，不记录窗口
        app.open_session_selector(&tx);
        assert!(app.overlay.is_some());
        press_esc(&mut app, &cmd, &tx);
        assert!(app.overlay.is_none(), "浮层 Esc 关闭");
        assert!(app.last_escape.is_none(), "浮层 Esc 不进双击窗口");

        // 3) 补全菜单 Esc → 关菜单，不记录窗口
        for ch in "/re".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE), &cmd, &tx);
        }
        assert!(app.completion.open, "菜单已开");
        press_esc(&mut app, &cmd, &tx);
        assert!(!app.completion.open, "菜单 Esc 关闭");
        assert!(app.last_escape.is_none(), "菜单 Esc 不进双击窗口");

        // 4) 回归：清空输入（菜单 Esc 保留输入）后，一次空闲 Esc 仍只是首击
        app.editor.replace("");
        assert!(app.last_escape.is_none(), "step3 后应无窗口");
        press_esc(&mut app, &cmd, &tx);
        assert!(app.last_escape.is_some(), "首击应记录");
        assert!(app.overlay.is_none(), "首击不开树: {:?}", app.overlay);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
