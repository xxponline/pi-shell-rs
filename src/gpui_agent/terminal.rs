//! GPUIAgentTerminal 组件（spec: 等宽字符网格渲染模型 / 三分区布局与状态行）。
//!
//! 订阅 [`crate::gpui_agent::actor`] 的事件扇出，自带 `session` 层归约，
//! 以等宽字符网格原生绘制三分区布局（消息区 / 状态行 / 输入区）。
//!
//! 已接：会话句柄、事件归约（4.1）、输入翻译与编辑/滚动/补全通路
//! （4.2/4.3）、三分区布局与网格度量（3.2）；消息区/状态行/输入区的
//! 绘制面仍为占位（5.x 接入）。

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
#[cfg(test)]
use std::sync::mpsc::Receiver;
use std::time::Instant;

use gpui_kit::base::TestTextExt;
use gpui_kit::component::input::{InputEvent, Position, Textarea, TextareaState};
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::text::TextView;
use gpui_kit::*;
use ratatui::style::{Color, Modifier, Style};

use crate::app::theme::TuiTheme;
use crate::app::{App as AppState, GridLine, RenderFlags, UiEvent};
use crate::gpui_agent::actor::RpcSession;
use crate::gpui_agent::input::keystroke_to_keyevents;
use crate::rpc::{ClientEvent, CommandSender};
use crate::session::{Block, Entry, SessionState, ToolExecution, ToolStatus};
/// 浅色主题调色板（spec: 独立窗口浅色模式固定；design D1）。
///
/// 收拢原散落的硬编码色：浅色面板/文字一组，深色工具卡上的浅字一组；
/// 后续主题 change 将整体替换为 theme token。
mod palette {
    use gpui_kit::Rgba;

    /// hex(0xRRGGBB) → Rgba（const，供常量色使用）。
    pub(super) const fn hex(v: u32) -> Rgba {
        Rgba {
            r: ((v >> 16) & 0xff) as f32 / 255.0,
            g: ((v >> 8) & 0xff) as f32 / 255.0,
            b: (v & 0xff) as f32 / 255.0,
            a: 1.0,
        }
    }

    // —— 浅色面板（Light 主题底色）——
    /// 根/消息区/补全菜单底（白）。
    pub(super) const CHAT_BG: Rgba = hex(0xffffff);
    /// 状态行底。
    pub(super) const STATUS_BG: Rgba = hex(0xf0f0f2);
    /// 输入区底（微灰，与消息区区分）。
    pub(super) const INPUT_BG: Rgba = hex(0xf7f8fa);
    /// 补全选中行底。
    pub(super) const COMP_SELECTED_BG: Rgba = hex(0xd8e6fb);

    // —— 浅色面板上的文字 ——
    /// 补全选中行文字。
    pub(super) const COMP_TEXT_SELECTED: Rgba = hex(0x1f1f1f);
    /// 补全普通行文字。
    pub(super) const COMP_TEXT: Rgba = hex(0x4a4a4a);
    /// 补全计数行文字。
    pub(super) const COMP_COUNT_TEXT: Rgba = hex(0x707070);
    /// "You:" 标签。
    pub(super) const USER_LABEL: Rgba = hex(0x2f7fe0);
    /// thinking 展开正文。
    pub(super) const THINKING_TEXT: Rgba = hex(0x666666);
    /// thinking 折叠占位（弱化斜体）。
    pub(super) const THINKING_PLACEHOLDER: Rgba = hex(0x6f6f6f);
    /// bash 命令行。
    pub(super) const BASH_HEAD: Rgba = hex(0x2f7f77);
    /// bash 输出摘要。
    pub(super) const BASH_BODY: Rgba = hex(0x606060);
    /// 通知：错误。
    pub(super) const NOTICE_ERROR: Rgba = hex(0xc62828);
    /// 通知：重试/警告。
    pub(super) const NOTICE_RETRY: Rgba = hex(0x946200);
    /// 通知：普通（弱化）。
    pub(super) const NOTICE_DEFAULT: Rgba = hex(0x666666);

    // —— 深色工具卡（底色来自 TuiTheme = pi dark）上的浅字 ——
    /// 工具卡结果预览（卡底为深色，保持浅字）。
    pub(super) const TOOL_PREVIEW_TEXT: Rgba = hex(0x9a9a9a);
}


/// 网格字号（px）——与宿主 AgentTerminal 同值（13px），保证度量口径一致。
const GRID_FONT_SIZE_PX: f32 = 13.;

/// 网格等宽字体候选表（按平台优先级取宿主实际安装的第一个；
/// 全部缺失退回首候选，度量退化由后续任务的等宽校验暴露）。
const GRID_FONT_FALLBACKS: &[&str] = &[
    "Menlo",            // macOS 自带等宽
    "Cascadia Mono",    // Windows 10+ 自带
    "Consolas",         // Windows 长期自带
    "Ubuntu Sans Mono", // Linux 常见
    "DejaVu Sans Mono", // Linux 通用兜底
];

/// GPUIAgentTerminal：独立窗口/宿主内嵌的 Agent 终端组件。
///
/// 持有会话句柄（关闭窗口或 app 退出时回收 pi 子进程，双保险：
/// 实体 Drop 与 `on_app_quit` 钩子均调用幂等 shutdown）。
pub struct GPUIAgentTerminal {
    /// 会话句柄（RPC 路由 + 主事件订阅；主订阅已被归约泵接管）。
    session: RpcSession,
    /// 组件侧归约状态（事件泵写入，渲染面读取）。
    state: TerminalState,
    /// 输入焦点句柄（= 内建 Textarea 的焦点句柄克隆；开窗后聚焦、拦截层判定）。
    focus: FocusHandle,
    /// 内建多行编辑器（task 5.4：auto_grow 随行数增长、内容行上限 6）。
    textarea: Entity<TextareaState>,
    /// 命令发送端（`handle_key` 提交/中止与排队补发通道）。
    commander: CommandSender,
    /// 回报通道发送端（`handle_key`/异步补发的 UiEvent 出口）。
    ui_tx: Sender<UiEvent>,
    /// 回报泵任务（仅生产：std→tokio→gpui 自动应用；gpui 测试调度器禁止
    /// 后台线程唤醒前台任务，测试下不建泵，改由 `pump_ui_events` 拉取，task 6.2）。
    #[cfg(not(test))]
    _ui_task: Task<()>,
    /// 回报通道接收端（仅测试：等待点显式拉取积压回报）。
    #[cfg(test)]
    ui_rx: Receiver<UiEvent>,
    /// 内建虚拟滚动状态（task 5.2：行数对账与贴底跟随在渲染/归约泵同步）。
    scroller: Entity<MessageScrollerState>,
    /// Root 键位拦截订阅（RAII：实体销毁即反注册，防悬垂）。
    _interceptor: Subscription,
    /// 编辑器变更订阅（镜像进 App + 重算补全；RAII 反注册，task 4.3）。
    _change_sub: Subscription,
    /// app 退出钩子订阅（RAII：实体销毁即反注册，防悬垂）。
    _quit: Subscription,
    /// 归约泵任务（持有至实体销毁；实体释放后泵自动退出）。
    /// 归约泵任务（仅生产态；测试态不装——见归约泵处注释）。
    #[cfg(not(test))]
    _reduce_task: Task<()>,
}

/// 组件侧归约状态：`App`（内含唯一 `SessionState`）+ 连接标志（与渲染解耦，可离线回放测试）。
///
/// 会话状态只存在 `App` 内部那一份——事件泵归约与按键语义（`handle_key`
/// 的 phase/bash_running 判定）读写同一状态，防双状态漂移。
/// 语义对齐 ratatui App 的事件处理：`ParseError` → 红色通知，
/// `Closed` → 通知 + 关闭标志（spec: 事件扇出/关闭感知）。
pub struct TerminalState {
    /// 应用状态（session 归约写入 `app.state`；notice/编辑器/滚动/补全同体）。
    app: AppState,
    /// pi stdout 已关闭（关闭后渲染面显示断开态）。
    closed: bool,
}

impl TerminalState {
    /// 创建初始归约状态（空 transcript、未关闭）。
    ///
    /// 功能: 组件状态初始化；
    /// 输入: 无；
    /// 输出: 空态 `TerminalState`。
    fn new() -> Self {
        Self {
            app: AppState::new(),
            closed: false,
        }
    }

    /// 归约一条核心事件（事件泵与离线回放共用的唯一入口）。
    ///
    /// 功能: 无 id 会话事件交给 `App::apply_core_event`（session 归约 +
    ///       compaction_end 补发边沿，对齐 ratatui 事件循环 Rpc 分支）；
    ///       解析错误/关闭置红色通知（文案与颜色同 ratatui）；
    /// 输入: event — 订阅扇出的 `ClientEvent`；commander/report_tx —
    ///       排队补发与回报通道；
    /// 输出: 无返回值；副作用为状态推进/通知置位（渲染面经 `notify` 重绘）。
    fn reduce(
        &mut self,
        event: &ClientEvent,
        commander: &CommandSender,
        report_tx: &Sender<UiEvent>,
    ) {
        match event {
            ClientEvent::Event(v) => self.app.apply_core_event(v, commander, report_tx),
            ClientEvent::ParseError { raw } => {
                self.app.notice = Some((format!("[parse_error] {raw}"), Color::Red));
            }
            ClientEvent::Closed => {
                self.closed = true;
                self.app.notice = Some(("pi 连接已关闭".to_string(), Color::Red));
            }
        }
    }

    /// 会话状态只读访问（渲染面读 transcript/运行态/用量）。
    ///
    /// 功能: 暴露 `App` 内部唯一 `SessionState`；
    /// 输入: 无；
    /// 输出: 归约后的会话状态引用。
    pub fn session(&self) -> &SessionState {
        &self.app.state
    }
}

impl GPUIAgentTerminal {
    /// 创建组件、接管主事件订阅并启动归约泵，注册 app 退出钩子。
    ///
    /// 功能: 接管 `RpcSession` 主订阅 → 阻塞接收线程转投 tokio 通道 →
    ///       gpui 任务逐条 `reduce` 进组件状态并 `notify` 重绘；
    ///       退出时显式回收 pi（与实体 Drop 幂等叠加）；
    /// 输入: session — 已探活的会话句柄（组件接管其生命周期与事件流）；
    ///       cx — gpui 实体上下文（注册钩子/拉起归约泵）；
    /// 输出: 组件实例（供 `cx.new` 挂到窗口）。
    pub fn new(
        #[cfg_attr(test, allow(unused_mut))] mut session: RpcSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let quit = cx.on_app_quit(|this, _cx| {
            this.session.shutdown();
            async {}
        });

        // 探活数据应用（与 ratatui 启动同源：model/thinkingLevel/
        // sessionName——状态行 model 显示依赖，task 5.3）。
        let mut state = TerminalState::new();
        state.app.state.apply_get_state(&session.probe);
        // 斜杠命令 P0 查询子集（task 6.3）：其余已知命令给未支持提示桩。
        state.app.slash_subset_only = true;

        // 内建虚拟滚动（task 5.2）：初始行数 = 打开时条目数（fresh 会话为 0，
        // 随后由渲染点对账 append/reset 追平）。
        let scroller = cx.new(|cx| {
            MessageScrollerState::new(state.session().entries().len(), cx)
        });

        // 内建多行编辑器（task 5.4）：auto_grow(1,6) = 随行数增长、内容行
        // 上限 6（与 ratatui editor_height 的内容行 clamp(1,6) 同源）。
        let textarea = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 6));
        let focus = textarea.focus_handle(cx);
        // 文本变更 → 镜像进 App.editor + 重算补全（task 4.3：补全随输入刷新；
        // push 回写用 set_value，不发 Change，不会形成回环）。
        let change_sub = cx.subscribe(&textarea, |this, _ta, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.sync_editor_from_textarea(cx);
                this.state
                    .app
                    .update_completion(&this.commander, &this.ui_tx);
                cx.notify();
            }
        });

        let commander = session.command_sender();
        let (ui_tx, ui_rx) = std::sync::mpsc::channel::<UiEvent>();

        // 回报泵（task 6.2）：异步命令应答/排队回填不依赖下一次按键。
        // 生产（真实调度器）：std → tokio → gpui 任务逐条应用并重绘；
        // 测试（gpui 测试调度器禁止后台线程唤醒前台任务，非确定性检测）：
        // 接收端留在组件里，由测试在等待点 `pump_ui_events` 显式拉取。
        #[cfg(not(test))]
        let ui_task = {
            let (uiq_tx, mut uiq_rx) = tokio::sync::mpsc::unbounded_channel::<UiEvent>();
            std::thread::spawn(move || {
                while let Ok(ev) = ui_rx.recv() {
                    if uiq_tx.send(ev).is_err() {
                        break; // 消费方（实体）已释放
                    }
                }
            });
            cx.spawn(async move |this, cx: &mut AsyncApp| {
                while let Some(first) = uiq_rx.recv().await {
                    // 攒批（task 5.5）：一批回报一次应用、一次重绘。
                    let mut batch = vec![first];
                    drain_pending(&mut uiq_rx, &mut batch);
                    let alive = this
                        .update(cx, |term, cx| {
                            for ev in &batch {
                                term.apply_ui_event(ev, cx);
                            }
                            cx.notify();
                        })
                        .is_ok();
                    if !alive {
                        break; // 实体已释放，泵退出
                    }
                }
            })
        };

        // 键位拦截（task 4.3 + 6.2）：三区路由——浮层打开全量接管、
        // 补全打开接管导航键、空闲仅语义键（enter/esc/ctrl+c），其余编辑键
        // 放行给内建 Textarea；Root 键位绑定先于元素监听，拦截层仍是
        // 唯一稳定入口。
        let intercept_focus = focus.clone();
        let intercept_entity = cx.weak_entity();
        let interceptor = cx.intercept_keystrokes(move |ev, window, cx| {
            if !intercept_focus.is_focused(window) {
                return;
            }
            let keystroke = ev.keystroke.clone();
            if let Some(entity) = intercept_entity.upgrade() {
                entity.update(cx, |term, cx| {
                    let route = route_keystroke(
                        term.state.app.overlay.is_some(),
                        term.state.app.completion.open,
                        &keystroke,
                    );
                    if route == Route::ToEditor {
                        return; // 编辑键交内建编辑器
                    }
                    // 先同步镜像：Change 订阅投递可能延迟到 flush，语义键
                    //（提交/清空/回填）不能依赖它（task 4.3 桥接时序）。
                    term.sync_editor_from_textarea(cx);
                    term.handle_keystroke(&keystroke);
                    // 语义动作后回写（命令清空/队列回填/补全应用 → 编辑器）。
                    term.push_editor_to_textarea(window, cx);
                    // 双击 ctrl+c / 双击 Esc 等语义置退出标志 → 关窗
                    // （关窗即退由 QuitMode::LastWindowClosed 接管）。
                    if term.state.app.should_quit {
                        window.remove_window();
                    }
                    cx.stop_propagation();
                    cx.notify();
                });
            }
        });

        // 归约泵：std mpsc（阻塞 recv，不占 gpui 线程）→ tokio unbounded
        // （future 与执行器无关）→ gpui 任务逐条提交实体。
        //
        // cfg(test) 不装泵：测试对事件的接收走直接注入（`state.reduce` 直调）
        // 与 ui_rx 手动泵；后台线程 wake gpui 前台任务会被 test scheduler 判为
        // 非确定性并 panic（ext 对话 send_raw 回执走事件流时实测触发，
        // add-gpui-overlays task 5.5）。不 take_events：会话内接收端继续排队，
        // 测试结束随实体 Drop 回收。
        #[cfg(not(test))]
        let reduce_task = {
            let events = session.take_events();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            std::thread::spawn(move || {
                while let Ok(event) = events.recv() {
                    if tx.send(event).is_err() {
                        break; // 消费方（实体）已释放
                    }
                }
            });
            cx.spawn(async move |this, cx: &mut AsyncApp| {
                while let Some(first) = rx.recv().await {
                    // 攒批（task 5.5）：把已到达的事件一次应用——流式事件风暴
                    // 只触发一帧，帧数不随事件数线性增长。
                    let mut batch = vec![first];
                    drain_pending(&mut rx, &mut batch);
                    let alive = this
                        .update(cx, |term, cx| {
                            for event in &batch {
                                // 字段级不相交借用：归约同时要 commander/ui_tx。
                                let (state, commander, ui_tx) =
                                    (&mut term.state, &term.commander, &term.ui_tx);
                                state.reduce(event, commander, ui_tx);
                            }
                            // 行数 + 行高对账（流式内容变高变矮要重测）。
                            term.sync_scroller(cx, true);
                            cx.notify();
                        })
                        .is_ok();
                    if !alive {
                        break; // 实体已释放，泵退出
                    }
                }
            })
        };

        Self {
            session,
            state,
            focus,
            textarea,
            scroller,
            commander,
            ui_tx,
            #[cfg(not(test))]
            _ui_task: ui_task,
            #[cfg(test)]
            ui_rx,
            _quit: quit,
            _interceptor: interceptor,
            _change_sub: change_sub,
            #[cfg(not(test))]
            _reduce_task: reduce_task,
        }
    }

    /// 会话句柄只读访问（后续任务接命令发送用）。
    ///
    /// 功能: 暴露内部 `RpcSession`；
    /// 输入: 无；
    /// 输出: 会话句柄引用（生命周期与组件一致）。
    pub fn session(&self) -> &RpcSession {
        &self.session
    }

    /// 组件侧归约状态只读访问（渲染面/宿主查询用）。
    ///
    /// 功能: 暴露归约后的会话状态；
    /// 输入: 无；
    /// 输出: 归约状态引用（transcript/运行态/用量/关闭标志）。
    pub fn state(&self) -> &TerminalState {
        &self.state
    }

    /// 输入窗焦点句柄克隆（开窗聚焦/宿主查询用；`FocusHandle` 克隆廉价，
    /// 且返回所有权避免与 `cx` 可变借用冲突）。
    ///
    /// 功能: 暴露输入窗焦点句柄；
    /// 输入: 无；
    /// 输出: 句柄克隆（调用方经 `.focus(window, cx)` 聚焦）。
    pub fn input_focus(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// 内建编辑器 → App 镜像（task 4.3 文本桥接：全文 + 光标同步）。
    ///
    /// 功能: 把 Textarea 的值与光标位置灌进 `App.editor`（供补全前缀判定、
    ///       提交/斜杠分发读取）；
    /// 输入: cx — 实体上下文（读编辑器实体）；
    /// 输出: 无返回值；副作用为 App.editor 全文与 row/col 更新。
    fn sync_editor_from_textarea(&mut self, cx: &mut App) {
        let (text, pos) = {
            let ta = self.textarea.read(cx);
            (ta.value().to_string(), ta.cursor_position())
        };
        let app = &mut self.state.app;
        app.editor.replace(&text);
        app.editor.set_cursor(pos.line as usize, pos.character as usize);
    }

    /// App → 内建编辑器回写（task 4.3：语义动作后——命令清空、队列回填、
    /// 补全应用都发生在 App.editor 上，需落回 Textarea 才能被用户看到）。
    ///
    /// 功能: 文本不同才 set_value + set_cursor_position（set_value 不发
    ///       Change，不与镜像形成回环）；
    /// 输入: window/cx — gpui 上下文（set_value 需要）；
    /// 输出: 无返回值；副作用为 Textarea 文本与光标更新（文本相同则不动）。
    fn push_editor_to_textarea(&mut self, window: &mut Window, cx: &mut App) {
        let text = self.state.app.editor.text();
        if self.textarea.read(cx).value().as_str() == text.as_str() {
            return;
        }
        let (row, col) = (self.state.app.editor.row(), self.state.app.editor.col());
        let position = Position {
            line: row as u32,
            character: col as u32,
        };
        self.textarea.update(cx, |ta, cx| {
            ta.set_value(text, window, cx);
            ta.set_cursor_position(position, window, cx);
        });
    }

    /// 经活动窗口回写内建编辑器（task 4.3：异步队列回填等路径没有
    /// `&mut Window`，借活动窗口转调 [`Self::push_editor_to_textarea`]）。
    ///
    /// 输入: cx — 应用上下文（取活动窗口）；
    /// 输出: 无返回值；副作用同 push（无窗口时跳过，由渲染点兜底）。
    fn push_editor_via_active_window(&mut self, cx: &mut App) {
        let handle = cx
            .active_window()
            .or_else(|| cx.windows().into_iter().next());
        let Some(handle) = handle else {
            return; // 无窗口：渲染/按键路径会再兜底
        };
        let _ = handle.update(cx, |_view, window, cx| {
            self.push_editor_to_textarea(window, cx);
        });
    }

    /// 处理一次键击（keystroke 拦截层的纯逻辑核心，可无 TTY 单测）。
    ///
    /// 功能: gpui 键击 → crossterm 键事件（`input` 翻译层）→ 逐条喂
    ///       `App::handle_key`（编辑/滚动/补全/提交语义与 ratatui 同源），
    ///       随后清一次回报通道；
    /// 输入: keystroke — gpui 键击（键名 + 修饰键 + 可选合成字符）；
    /// 输出: 无返回值；副作用为编辑器/滚动/补全状态推进与 notice 置位。
    fn handle_keystroke(&mut self, keystroke: &Keystroke) {
        for key in keystroke_to_keyevents(keystroke) {
            self.state
                .app
                .handle_key(key, &self.commander, &self.ui_tx);
        }
    }

    /// 虚拟滚动对账（task 5.2；渲染点与归约泵共用）。
    ///
    /// 功能: 条目数增长 → 尾部 `append`（保持回看锚点）；收缩/整体替换 →
    ///       `reset`（回到底部语义）；同数且 `remeasure` → 全行重测（流式
    ///       内容变高变矮——仅归约泵传 true，渲染点传 false 防 notify 自旋）；
    /// 输入: cx — 实体上下文；remeasure — 同数时是否重测行高；
    /// 输出: 无返回值；副作用为虚拟列表行数/行高与会话条目对齐。
    fn sync_scroller(&mut self, cx: &mut Context<Self>, remeasure: bool) {
        let n = self.scroller_rows();
        let scroller = self.scroller.clone();
        scroller.update(cx, |s, cx| {
            let prev = s.item_count();
            if n == prev {
                if remeasure && n > 0 {
                    s.remeasure(cx);
                }
            } else if n > prev {
                s.append(n - prev, cx);
            } else {
                s.reset(n, cx);
            }
        });
    }

    /// 滚动行数 = 已落条目 + 流式 in-flight 尾块（task 5.2：流式中的
    /// assistant/thinking/工具调用也要有行，flush 后并入条目、行数自然收敛）。
    ///
    /// 输入: 无（读会话状态）；
    /// 输出: 虚拟列表应有行数。
    fn scroller_rows(&self) -> usize {
        let session = self.state.session();
        session.entries().len() + session.in_flight().len()
    }

    /// 应用一条回报事件（回报泵的任务体入口，task 6.2）。
    ///
    /// 功能: Info/SendFailed → notice（绿/红，同 ratatui），AsyncData/
    ///       CommandsFetched → 缓存回填 + 补全重算，RestoreQueued → 回填
    ///       输入框，OpenModelSelect → 选择器不可用提示（P0 无浮层）；
    ///       其余（外部编辑器/主题等）P0 静默丢弃；
    /// 输入: ev — 回报事件（泵任务逐条投递）；
    /// 输出: 无返回值；副作用为 notice/缓存/输入框更新（随后 notify 重绘）。
    fn apply_ui_event(&mut self, ev: &UiEvent, cx: &mut Context<Self>) {
        match ev {
            UiEvent::Info(text) => {
                self.state.app.notice = Some((text.clone(), Color::Green));
            }
            UiEvent::SendFailed(err) => {
                self.state.app.notice = Some((err.clone(), Color::Red));
            }
            UiEvent::AsyncData { kind, seq, data } => {
                self.state.app.handle_async_data(*kind, *seq, data);
                self.state.app.refresh_completion();
            }
            UiEvent::CommandsFetched(data) => {
                self.state.app.apply_commands_fetched(data);
                self.state.app.refresh_completion();
            }
            UiEvent::RestoreQueued(data) => {
                self.state.app.restore_queued(data);
                // 回填改的是 App.editor：立即回写 Textarea，否则下一次语义键
                // 的“键前同步”会用空编辑器反清镜像（task 4.3 桥接时序）。
                self.push_editor_via_active_window(cx);
            }
            UiEvent::OpenModelSelect => {
                self.state.app.notice = Some((
                    "未找到匹配模型（GPUI P0 无选择器，请带精确参数）".to_string(),
                    Color::Yellow,
                ));
            }
            // P0：其余回报（浮层选择/外部编辑器/主题注入等）随后续任务接。
            _ => {}
        }
    }

    /// 拉取并应用积压的回报事件（仅测试；task 6.2）。
    ///
    /// 功能: `try_recv` 循环逐条走 [`Self::apply_ui_event`]——gpui 测试调度
    ///       器禁止后台线程唤醒前台任务，测试在等待点显式拉取；生产由回报
    ///       泵任务自动应用（语义同源，仅唤醒机制不同）；
    /// 输入: 无（读写 self）；
    /// 输出: 无返回值；副作用为 notice/缓存/输入框更新。
    #[cfg(test)]
    fn pump_ui_events(&mut self, cx: &mut Context<Self>) {
        while let Ok(ev) = self.ui_rx.try_recv() {
            self.apply_ui_event(&ev, cx);
        }
    }
}

/// 键击路由结果（task 6.1：三区模型）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    /// 交共享语义层（`App::handle_key`）并停止传播。
    ToApp,
    /// 放行给内建编辑器（Textarea 自理）。
    ToEditor,
}

/// 键击路由判定（task 6.1：三区模型纯函数，可无 TTY 单测）。
///
/// 功能: 决定一次键击交共享语义层还是放行编辑器；
/// 输入: overlay_open — 浮层打开（A 区：全量接管；ctrl+c/浮层/意图分发的
///       优先级由 `App::handle_key` 内部保证，与 TUI 分支序一致）；
///       completion_open — 补全菜单打开（B 区：导航/应用键）；
///       ks — gpui 键击；
/// 输出: [`Route::ToApp`] / [`Route::ToEditor`]。
pub(crate) fn route_keystroke(overlay_open: bool, completion_open: bool, ks: &Keystroke) -> Route {
    if overlay_open {
        return Route::ToApp;
    }
    let to_app = match ks.key.as_str() {
        "enter" => !ks.modifiers.shift, // shift+enter = 换行（编辑器自理）
        "escape" => true,               // 菜单关闭 / 流式中断 / 空闲无操作
        "c" if ks.modifiers.control => true, // ctrl+c 清空
        // 打开键（task 6.3）：镜像 `app::key_intent` —— ctrl+l → model、
        // alt+t → thinking（不门控菜单状态，与 TUI 意图分发同层）。
        "l" if ks.modifiers.control && !ks.modifiers.shift => true,
        "t" if ks.modifiers.alt => true,
        "up" | "down" | "pageup" | "pagedown" | "tab" if completion_open => true,
        _ => false,
    };
    if to_app { Route::ToApp } else { Route::ToEditor }
}

impl Render for GPUIAgentTerminal {
    /// 三分区占位渲染：消息区（弹性占满）/ 状态行（一行）/ 输入区（三行）。
    ///
    /// 输入: window — 网格度量来源（字体表/像素吸附）；cx — 实体上下文（未用）；
    /// 输出: 垂直 flex 元素树；三个分区各带 id 与观测锚点（无头测试 find 断言）。
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        //（桥接注意：渲染点不能盲目回写——Change 订阅投递可能落后于渲染，
        //  会用旧镜像清空正在输入的文本。App→Textarea 回写只发生在语义键
        //  拦截后与异步回填（RestoreQueued）两处。）
        // 渲染探针（task 5.5）：记录构帧耗时，收尾打印（关闭时零开销）。
        let probe_t0 = probe_enabled().then(Instant::now);
        // 虚拟滚动行数对账（task 5.2；渲染点只对行数，归约泵才重测行高，
        // 防 notify 自旋）。
        self.sync_scroller(cx, false);
        let (cell_w, line_h) = grid_metrics(window);
        let line_h_px = f32::from(line_h);
        let viewport = window.viewport_size();
        let cols = ((f32::from(viewport.width) / f32::from(cell_w)).floor() as u16).max(1);
        // 状态行内容（task 5.3）：与 ratatui render_header 同源装配（左段）；
        // 瞬时提示单独走 status_bar 右槽（红/黄/绿同 ratatui notice 色）。
        let status_line = self.state.app.status_grid_line(cols);
        let notice_right = self.state.app.notice.as_ref().map(|(text, color)| {
            div()
                .text_color(to_hsla(*color))
                .test_text(text.clone())
                .child(text.clone())
        });
        // 消息区（task 5.2）：内建虚拟滚动承载条目，行渲染按索引回调。
        let this = cx.weak_entity();
        let scroller_el = MessageScroller::new(
            "gpui-msgs",
            self.scroller.clone(),
            move |index, _window, cx| {
                let empty = div();
                let Some(entity) = this.upgrade() else {
                    return empty;
                };
                let term = entity.read(cx);
                let session = term.state.session();
                let flags = term.state.app.render_flags();
                let entries = session.entries();
                if let Some(entry) = entries.get(index) {
                    entry_element(entry, index, flags)
                } else if let Some(block) = session.in_flight().get(index - entries.len()) {
                    // 流式尾块（未 message_end）：与条目同样式逐块渲染。
                    assistant_block(block, index, index - entries.len(), flags)
                } else {
                    empty
                }
            },
        );
        // 补全菜单行（task 4.3 + 9.1：候选/选中态来自 App.completion，
        // 与 ratatui 同源的上限常量 + 滚动窗口 + 计数行）。
        let comp = &self.state.app.completion;
        let comp_open = comp.open && !comp.items.is_empty();
        let mut completion_pane: Vec<Div> = Vec::new();
        if comp_open {
            // 滚动窗口：选中项始终可见（镜像 view::render_completion；
            // spec: slash-autocomplete「超出上限滚动」）。
            let total = comp.items.len();
            let max = crate::app::view::COMPLETION_MAX_VISIBLE.min(total);
            let start = comp
                .selected
                .saturating_sub(max - 1)
                .min(total.saturating_sub(max));
            for (offset, item) in comp.items[start..].iter().take(max).enumerate() {
                let selected = start + offset == comp.selected;
                let mut row = div()
                    .h(line_h)
                    .px_2()
                    .flex()
                    .items_center();
                if selected {
                    row = row.bg(palette::COMP_SELECTED_BG);
                }
                completion_pane.push(
                    row.child(
                        div()
                            .text_sm()
                            .text_color(if selected {
                                palette::COMP_TEXT_SELECTED
                            } else {
                                palette::COMP_TEXT
                            })
                            .child(item.label.clone())
                            .test_text(item.label.clone()),
                    ),
                );
            }
            if comp.items.len() > crate::app::view::COMPLETION_MAX_VISIBLE {
                completion_pane.push(
                    div()
                        .h(line_h)
                        .px_2()
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .text_xs()
                                .text_color(palette::COMP_COUNT_TEXT)
                                .child(format!("({}/{})", comp.selected + 1, comp.items.len()))
                                .test_text(format!(
                                    "({}/{})",
                                    comp.selected + 1,
                                    comp.items.len()
                                )),
                        ),
                );
            }
        }
        let family = grid_font_family(window);
        let root = div()
            .size_full()
            .relative() // 浮层绝对定位的锚（task 3.2）
            .flex()
            .flex_col()
            .bg(palette::CHAT_BG)
            .child(
                // 消息区（task 5.2）：内建虚拟滚动（回看/贴底/跳底由
                // MessageScroller 自理），行内容按条目索引回调渲染。
                div()
                    .id("gpui-messages")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_hidden()
                    .test_support()
                    .child(div().size_full().child(scroller_el)),
            )
            .child(
                // 状态行（task 5.3）：内建 status_bar 分段——左 = header_line
                // 同源数据（phase/会话名/thinking/model/用量/费用/状态文本），
                // 右 = 瞬时提示。
                div()
                    .id("gpui-status")
                    .h(line_h)
                    .bg(palette::STATUS_BG)
                    .test_support()
                    .child({
                        let bar = StatusBar::new().left(
                            grid_row_with_id(
                                "gpui-status-left",
                                &status_line,
                                line_h_px,
                                &family,
                            )
                            .test_text(grid_row_text(&status_line)),
                        );
                        match notice_right {
                            Some(right) => bar.right(right),
                            None => bar,
                        }
                    }),
            )
            .child(
                // 输入区（task 5.4）：内建 Textarea（auto_grow 1..6 行、内建
                // 光标/撤销/换行）；全局键位经拦截层进共享语义层（task 4.3）。
                div()
                    .id("gpui-input")
                    .flex()
                    .flex_col()
                    .px_2()
                    .bg(palette::INPUT_BG)
                    .test_support()
                    .child(Textarea::new(&self.textarea)),
            )
            .child(
                // 补全菜单（输入区正下方，task 4.3）。
                div()
                    .id("gpui-completion")
                    .flex()
                    .flex_col()
                    .bg(palette::CHAT_BG)
                    .test_support()
                    .children(completion_pane),
            );
        // 浮层层（task 3.2）：overlay 打开时绝对置顶（遮罩+居中面板）；
        // gpui 树序后画者在上，故挂在根的最后。
        let root = match crate::gpui_agent::overlay::overlay_layer(&self.state.app, viewport, cx) {
            Some(layer) => root.child(layer),
            None => root,
        };
        if let Some(t0) = probe_t0 {
            static FRAME: AtomicU64 = AtomicU64::new(0);
            let n = FRAME.fetch_add(1, Ordering::Relaxed);
            eprintln!(
                "[gpui-render-probe] frame={n} build={:?} rows={}",
                t0.elapsed(),
                self.scroller_rows()
            );
        }
        root
    }
}

/// 攒批核心（task 5.5）：首条之后非阻塞取空通道——事件风暴合并为一批、
/// 一批只触发一次实体更新与重绘（帧数不随事件数线性增长）。
///
/// 输入: rx — tokio 无界通道接收端；batch — 批收集器（首条已入）；
/// 输出: 无返回值；副作用为把已到达事件全部推进 batch。
fn drain_pending<T>(rx: &mut tokio::sync::mpsc::UnboundedReceiver<T>, batch: &mut Vec<T>) {
    while let Ok(ev) = rx.try_recv() {
        batch.push(ev);
    }
}

/// 渲染探针开关（task 5.5）：`PI_GPUI_RENDER_PROBE=1` 开启帧构建耗时打印
/// （进程级一次性读取；分段定位卡顿用）。
///
/// 输入: 无；输出: true = 开启。
fn probe_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("PI_GPUI_RENDER_PROBE").is_ok_and(|v| v == "1"))
}

/// 网格字体解析：候选表按序取宿主实际安装的第一个；字体表不可用/全部缺失
/// 退回首候选（度量退化由等宽校验负责暴露——后续任务）。
///
/// 输入: window — 提供系统字体表；
/// 输出: 字体族名（`SharedString`）。
fn grid_font_family(window: &Window) -> SharedString {
    // 进程级缓存（task 5.5）：全量字体枚举 + 排序是帧级热点（实测构帧
    // ~130ms，两次调用），会话内字体集不变 → 首帧解析一次。
    static CACHE: OnceLock<SharedString> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let available = window.text_system().all_font_names();
            GRID_FONT_FALLBACKS
                .iter()
                .copied()
                .find(|candidate| available.iter().any(|name| name.as_str() == *candidate))
                .unwrap_or(GRID_FONT_FALLBACKS[0])
                .into()
        })
        .clone()
}

/// 网格度量：(单元格宽, 行高)——渲染与换算共享的唯一真相。
///
/// 单元格宽 = 网格字体 13px 的等宽 advance（10×"M" 均值）并吸附到设备像素
/// （与 gpui 盒吸附同式，防 cols×cell_w 漂移）；行高 = 默认 line_height
/// 经 pixel_snap，与 gpui 文本布局同式。
///
/// 输入: window — 文本系统与像素吸附来源；
/// 输出: `(cell_w, line_h)`（依赖排版缓存，同一帧内重复调用结果一致）。
pub fn grid_metrics(window: &Window) -> (Pixels, Pixels) {
    const SAMPLE: &str = "MMMMMMMMMM";
    let family = grid_font_family(window);
    let runs = [TextRun {
        len: SAMPLE.len(),
        font: font(family),
        ..Default::default()
    }];
    let layout = window
        .text_system()
        .layout_line(SAMPLE, px(GRID_FONT_SIZE_PX), &runs, None);
    let cell_w = window.pixel_snap(px(f32::from(layout.width) / SAMPLE.len() as f32));
    let style = TextStyle {
        font_size: AbsoluteLength::Pixels(px(GRID_FONT_SIZE_PX)),
        ..Default::default()
    };
    let line_h = window.pixel_snap(
        style
            .line_height
            .to_pixels(style.font_size, window.rem_size()),
    );
    (cell_w, line_h)
}

/// 网格尺寸换算：像素区域 ÷ 单元格 → (cols, rows)（向下取整，spec:
/// 等宽字符网格渲染模型的换算口径）。
///
/// 输入: area — 区域像素尺寸；cell — 单元格像素尺寸（非正时退化为 0 行/列）；
/// 输出: `(cols, rows)`，封顶 `u16::MAX`。
pub fn grid_size(area: Size<Pixels>, cell: Size<Pixels>) -> (u16, u16) {
    let cols = (area.width / cell.width).floor();
    let rows = (area.height / cell.height).floor();
    (
        cols.clamp(0., u16::MAX as f32) as u16,
        rows.clamp(0., u16::MAX as f32) as u16,
    )
}

/// 默认文本前景（ratatui `fg=None` = 终端默认色；深底观感对齐 pi dark）。
const DEFAULT_TEXT_FG: Color = Color::Rgb(0xd4, 0xd4, 0xd4);

/// 默认面板底（与组件根背景同值；REVERSED/无 bg 时的兜底）。
const DEFAULT_BG: Color = Color::Rgb(0x1e, 0x1e, 0x1e);

/// xterm256 索引 → RGB（0-15 VGA、16-231 色立方、232-255 灰阶，标准调色板）。
///
/// 输入: i — 0..=255 索引；输出: (r, g, b)。
fn indexed_rgb(i: u8) -> (u8, u8, u8) {
    const BASIC: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (128, 0, 0),
        (0, 128, 0),
        (128, 128, 0),
        (0, 0, 128),
        (128, 0, 128),
        (0, 128, 128),
        (192, 192, 192),
        (128, 128, 128),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (0, 0, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    match i {
        0..=15 => BASIC[i as usize],
        16..=231 => {
            let n = i - 16;
            let step = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            (step(n / 36), step((n / 6) % 6), step(n % 6))
        }
        _ => {
            let v = 8 + (i - 232) * 10;
            (v, v, v)
        }
    }
}

/// ratatui 颜色 → gpui 颜色（named 走 xterm 标准调色板、Rgb 直通、
/// Reset 取默认前景）。
///
/// 输入: c — ratatui 颜色；输出: gpui `Hsla`（a=1）。
fn to_hsla(c: Color) -> Hsla {
    let (r, g, b) = match c {
        Color::Reset => (0xd4, 0xd4, 0xd4),
        Color::Black => (0, 0, 0),
        Color::Red => (128, 0, 0),
        Color::Green => (0, 128, 0),
        Color::Yellow => (128, 128, 0),
        Color::Blue => (0, 0, 128),
        Color::Magenta => (128, 0, 128),
        Color::Cyan => (0, 128, 128),
        Color::Gray => (192, 192, 192),
        Color::DarkGray => (128, 128, 128),
        Color::LightRed => (255, 0, 0),
        Color::LightGreen => (0, 255, 0),
        Color::LightYellow => (255, 255, 0),
        Color::LightBlue => (0, 0, 255),
        Color::LightMagenta => (255, 0, 255),
        Color::LightCyan => (0, 255, 255),
        Color::White => (255, 255, 255),
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(i) => indexed_rgb(i),
    };
    rgb((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)).into()
}

/// 生效样式 → 运行字体（BOLD/ITALIC 映射到网格字体族）。
///
/// 输入: style — 生效 ratatui 样式；family — 网格字体族；
/// 输出: `Font`（weight/style 随修饰位，其余默认）。
fn run_font(style: &Style, family: &SharedString) -> Font {
    let mut f = font(family.clone());
    if style.add_modifier.contains(Modifier::BOLD) {
        f.weight = FontWeight::BOLD;
    }
    if style.add_modifier.contains(Modifier::ITALIC) {
        f.style = FontStyle::Italic;
    }
    f
}

/// 中性屏行 → 定高行元素（状态行等场景；元素 id 由调用方指定）。
///
/// 输入: id — 元素 id；line — 中性屏行；line_h — 行高 px；family — 字体族；
/// 输出: 定高行 div（StyledText runs 逐段着色）。
fn grid_row_with_id(
    id: impl Into<ElementId>,
    line: &GridLine,
    line_h: f32,
    family: &SharedString,
) -> Stateful<Div> {
    let mut text = String::new();
    let mut runs: Vec<TextRun> = Vec::new();
    for seg in &line.segs {
        if seg.text.is_empty() {
            continue;
        }
        let mut eff = line.style.patch(seg.style);
        // REVERSED：前景对调（缺省侧取默认前景/默认底）。
        if eff.add_modifier.contains(Modifier::REVERSED) {
            let fg = eff.fg.unwrap_or(DEFAULT_TEXT_FG);
            let bg = eff.bg.unwrap_or(DEFAULT_BG);
            eff.fg = Some(bg);
            eff.bg = Some(fg);
        }
        let start = text.len();
        text.push_str(&seg.text);
        let mut color = to_hsla(eff.fg.unwrap_or(DEFAULT_TEXT_FG));
        if eff.add_modifier.contains(Modifier::DIM) {
            color.a = 0.6;
        }
        runs.push(TextRun {
            len: text.len() - start,
            font: run_font(&eff, family),
            color,
            background_color: eff.bg.map(to_hsla),
            underline: eff
                .add_modifier
                .contains(Modifier::UNDERLINED)
                .then(|| UnderlineStyle {
                    thickness: px(1.),
                    color: None,
                    wavy: false,
                }),
            strikethrough: eff
                .add_modifier
                .contains(Modifier::CROSSED_OUT)
                .then(|| StrikethroughStyle {
                    thickness: px(1.),
                    color: None,
                }),
        });
    }
    let mut styled = StyledText::new(text);
    if !runs.is_empty() {
        styled = styled.with_runs(runs);
    }
    div().id(id).h(px(line_h)).child(styled)
}

/// 中性屏行的纯文本（测试快照记录与断言共用）。
///
/// 输入: line — 中性屏行；输出: 段文本按序拼接。
fn grid_row_text(line: &GridLine) -> String {
    line.segs.iter().map(|s| s.text.as_str()).collect()
}

/// 会话条目 → 内建元素（task 5.2：条目级渲染，虚拟滚动逐行回调）。
///
/// 输入: entry — 会话条目；index — 条目序号（元素 id 稳定性）；
///       flags — 渲染偏好（thinking 展开态）；
/// 输出: 条目行元素（User 标签+markdown / Assistant 分块 / Notice 色条 /
///       Bash 命令+输出摘要；未知内容不 panic）。
fn entry_element(entry: &Entry, index: usize, flags: RenderFlags) -> Div {
    let _ = index; // 元素 id 已去（Stateful 类型不齐）；序号仍保留在签名供后续稳定 id 恢复
    match entry {
        Entry::User { text } => div()
            .flex()
            .flex_col()
            .px_2()
            .py_1()
            .child(
                div()
                    .text_sm()
                    .text_color(palette::USER_LABEL)
                    .test_text("You:")
                    .child("You:"),
            )
            .child(
                div()
                    .w_full()
                    .test_text(text.clone())
                    .child(TextView::markdown(format!("md-u-{index}"), text.clone())),
            ),
        Entry::Assistant { blocks, .. } => {
            let mut col = div().flex().flex_col().px_2().py_1();
            for (bi, block) in blocks.iter().enumerate() {
                col = col.child(assistant_block(block, index, bi, flags));
            }
            col
        }
        Entry::Notice { kind, text } => notice_row(kind, text, index),
        Entry::Bash {
            command,
            output,
            exit_code,
            ..
        } => bash_row(command, output, *exit_code, index),
    }
}

/// assistant 块 → 元素（markdown / thinking 折叠 / 工具卡）。
///
/// 输入: block — 内容块；index/bi — 条目与块序号（元素 id）；flags — 渲染偏好；
/// 输出: 块元素（空文本块给零高占位）。
fn assistant_block(block: &Block, index: usize, bi: usize, flags: RenderFlags) -> Div {
    let _ = (index, bi);
    match block {
        Block::Text { text } if !text.is_empty() => div().w_full().child(
            div()
                .test_text(text.clone())
                .child(TextView::markdown(format!("md-{index}-{bi}"), text.clone())),
        ),
        Block::Text { .. } => div(),
        Block::Thinking { text } => {
            if flags.show_thinking {
                div()
                    .w_full()
                    .text_color(palette::THINKING_TEXT)
                    .child(div().test_text(text.clone()).child(text.clone()))
            } else {
                let placeholder = format!("<thinking {} chars>", text.chars().count());
                div()
                .text_color(palette::THINKING_PLACEHOLDER)
                    .italic()
                    .child(div().test_text(placeholder.clone()).child(placeholder))
            }
        }
        Block::ToolCall {
            name,
            args_json: _,
            tool,
            ..
        } => tool_card(name, tool, index, bi),
    }
}

/// 工具卡（read/bash/edit/通用同构：状态面板 + 结果首行折叠预览）。
///
/// 输入: name — 工具名；tool — 执行状态/结果；index/bi — 元素 id 序号；
/// 输出: 卡片元素（面板底色随状态：运行灰/失败暗红/成功墨绿）。
fn tool_card(name: &str, tool: &ToolExecution, index: usize, bi: usize) -> Div {
    let _ = (index, bi);
    let theme = TuiTheme::default();
    let (panel, status, fg) = match &tool.status {
        ToolStatus::Pending => (theme.tool_running_bg, "pending", Color::Yellow),
        ToolStatus::Running => (theme.tool_running_bg, "running", Color::Yellow),
        ToolStatus::Ended if tool.is_error => (theme.tool_error_bg, "error", Color::Red),
        ToolStatus::Ended => (theme.tool_success_bg, "done", Color::Green),
    };
    let head = format!("● {name} · {status}");
    let preview = tool_preview(tool);
    let mut card = div()
        .flex()
        .flex_col()
        .px_2()
        .py_1()
        .bg(to_hsla(panel))
        .child(
            div()
                .text_color(to_hsla(fg))
                .test_text(head.clone())
                .child(head),
        );
    if !preview.is_empty() {
        card = card.child(
            div()
                .text_xs()
                .text_color(palette::TOOL_PREVIEW_TEXT)
                .test_text(preview.clone())
                .child(preview),
        );
    }
    card
}

/// 工具结果首行预览（折叠态；与 TUI 同源抽取 `tools::result_text`）。
///
/// 输入: tool — 执行结果（result 缺回落 partial）；
/// 输出: 首行文本（多行时追加 `…`）；无内容返回空串。
fn tool_preview(tool: &ToolExecution) -> String {
    let text = crate::app::result_text(tool).unwrap_or_default();
    let mut lines = text.lines();
    let first = lines.next().unwrap_or("").to_string();
    if lines.next().is_some() {
        format!("{first}\n…")
    } else {
        first
    }
}

/// 通知行（error 红 / retry 黄 / 其余弱化——对齐 TUI notice_line 语义）。
///
/// 输入: kind — 通知类别；text — 通知正文；index — 元素 id 序号；
/// 输出: 色条行元素。
fn notice_row(kind: &str, text: &str, index: usize) -> Div {
    let _ = index;
    let fg = match kind {
        "error" => palette::NOTICE_ERROR,
        "retry" => palette::NOTICE_RETRY,
        _ => palette::NOTICE_DEFAULT,
    };
    div()
        .px_2()
        .py_1()
        .text_color(fg)
        .child(div().test_text(text.to_string()).child(text.to_string()))
}

/// bash 条目（命令行 + 输出摘要，前 5 行 + `…`）。
///
/// 输入: command/output/exit_code — bash 条目字段；index — 元素 id 序号；
/// 输出: 行元素。
fn bash_row(command: &str, output: &str, exit_code: Option<i64>, index: usize) -> Div {
    let _ = index;
    let mut head = format!("$ {command}");
    if let Some(code) = exit_code {
        head.push_str(&format!("  (exit {code})"));
    }
    let body = first_lines(output, 5);
    let mut row = div()
        .flex()
        .flex_col()
        .px_2()
        .py_1()
        .child(
            div()
                .text_sm()
                .text_color(palette::BASH_HEAD)
                .test_text(head.clone())
                .child(head),
        );
    if !body.is_empty() {
        row = row.child(
            div()
                .text_xs()
                .text_color(palette::BASH_BODY)
                .test_text(body.clone())
                .child(body),
        );
    }
    row
}

/// 取前 n 行（超出追加 `…`）。
///
/// 输入: text — 多行文本；n — 行数上限；
/// 输出: 摘要文本（空文本返回空串）。
fn first_lines(text: &str, n: usize) -> String {
    let mut ls = text.lines();
    let head: Vec<&str> = ls.by_ref().take(n).collect();
    let mut out = head.join("\n");
    if ls.next().is_some() {
        out.push_str("\n…");
    }
    out
}

#[cfg(test)]
mod tests {
    //! 显式导入、不用 glob —— 避免 `gpui_kit::*` 把 `#[test]` 宏遮蔽掉
    //! （宿主仓 terminal/mod.rs 同款教训：遮蔽后宏生成的 `#[test]` 递归展开）。
    use super::{
        Color, GPUIAgentTerminal, Keystroke, Modifiers, Route, ScrollDelta, TerminalState, UiEvent,
        drain_pending, grid_metrics, grid_size, point, route_keystroke,
    };
    use crate::gpui_agent::actor::RpcSession;
    use crate::rpc::ClientEvent;
    use crate::session::{Block, Entry, Phase, ToolExecution, replay_file};
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{
        AppContext, Bounds, ElementId, Point, Size, TestAppContext, WindowBounds, WindowOptions,
        px, size,
    };
    use std::fs;
    use std::io::{BufRead, BufReader, Write as _};

    /// 写一个应答 get_state 的 fake pi 脚本（模式同 `actor.rs` 测试）。
    ///
    /// 功能: 落盘可执行 shell 脚本；
    /// 输入: name — 目录区分名；
    /// 输出: 脚本绝对路径（unix 上 chmod 755）。
    fn write_stub(name: &str) -> String {
        let dir = std::env::temp_dir().join(format!("gpui-term-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fake-pi.sh");
        let body = r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
  printf '{"type":"response","id":"%s","success":true,"data":{}}\n' "$id"
done
"#;
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        drop(f);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        path.to_string_lossy().into_owned()
    }

    /// 浮层图层：Info 打开时面板在场、关闭时消失（task 3.2；spec: 悬浮层渲染）。
    ///
    /// 功能: 注入 `Overlay::Info` 渲染一帧断言标题/正文/关闭提示在场，
    ///       清空 overlay 再渲染一帧断言图层消失；
    /// 输入: 测试上下文（fake pi 提供会话）；
    /// 输出: 断言通过（实体 Drop 回收 fake pi）。
    #[gpui_kit::test]
    fn overlay_info_renders_title(cx: &mut TestAppContext) {
        let path = write_stub("overlay-info");
        let session = RpcSession::spawn(&path, &[], None).expect("spawn 应成功");

        cx.update(gpui_kit::init);
        let (handle, term) = cx.update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Point::default(),
                        size: size(px(800.), px(600.)),
                    })),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| GPUIAgentTerminal::new(session, window, cx)),
            )
            .expect("open test window")
        });

        // 注入 Info 浮层（共享状态直读，不走按键）：先 loading 相位。
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state.app.overlay = Some(crate::app::overlay::Overlay::Info {
                    title: "Session".into(),
                    lines: vec!["hello-info-line".into()],
                    loading: true,
                    seq: 1,
                });
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render loading");
        cx.run_until_parked();
        let loading_text = cx
            .update_window(handle, |_, window, _| {
                gpui_kit::base::test_support::texts(window)
            })
            .expect("texts")
            .iter()
            .map(|t| t.text().to_string())
            .collect::<Vec<_>>()
            .join("");
        assert!(
            loading_text.contains("loading…") && loading_text.contains("Session"),
            "loading 态不应空面板（标题+loading 行）: {loading_text:?}"
        );

        // 切到已加载相位。
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state.app.overlay = Some(crate::app::overlay::Overlay::Info {
                    title: "Session".into(),
                    lines: vec!["hello-info-line".into()],
                    loading: false,
                    seq: 1,
                });
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render frame");
        cx.run_until_parked();
        let with_overlay = cx
            .update_window(handle, |_, window, _| {
                gpui_kit::base::test_support::texts(window)
            })
            .expect("texts");
        let with_text = with_overlay
            .iter()
            .map(|t| t.text().to_string())
            .collect::<Vec<_>>()
            .join("");
        assert!(with_text.contains("Session"), "标题应可见: {with_text:?}");
        assert!(with_text.contains("hello-info-line"), "正文应可见");
        assert!(with_text.contains("Esc 关闭"), "关闭提示应可见");

        // 关闭浮层 → 图层消失。
        cx.update(|cx| term.update(cx, |term, _| term.state.app.overlay = None));
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render frame");
        cx.run_until_parked();
        let without = cx
            .update_window(handle, |_, window, _| {
                gpui_kit::base::test_support::texts(window)
            })
            .expect("texts");
        let without_text = without
            .iter()
            .map(|t| t.text().to_string())
            .collect::<Vec<_>>()
            .join("");
        assert!(
            !without_text.contains("hello-info-line"),
            "关闭后正文应消失"
        );
    }

    /// 键击路由三区矩阵（task 6.1；spec: 悬浮层按键与滚轮路由）。
    ///
    /// 功能: 断言浮层区全量接管、空闲区仅语义键、补全区含导航键、
    ///       浮层优先级压过补全；
    /// 输入: 构造的 Keystroke 矩阵；
    /// 输出: 断言通过（Route 逐步相等）。
    #[test]
    fn route_keystroke_three_zones() {
        let k = |key: &str, control: bool, shift: bool, alt: bool| Keystroke {
            key: key.to_string(),
            modifiers: Modifiers {
                control,
                shift,
                alt,
                ..Default::default()
            },
            key_char: None,
        };
        // A 区：浮层打开 → 任意键全量接管
        assert_eq!(
            route_keystroke(true, false, &k("x", false, false, false)),
            Route::ToApp
        );
        assert_eq!(
            route_keystroke(true, false, &k("enter", false, false, false)),
            Route::ToApp
        );
        // C 区 idle：语义键进 App，编辑键放行
        assert_eq!(
            route_keystroke(false, false, &k("enter", false, false, false)),
            Route::ToApp
        );
        assert_eq!(
            route_keystroke(false, false, &k("escape", false, false, false)),
            Route::ToApp
        );
        assert_eq!(
            route_keystroke(false, false, &k("c", true, false, false)),
            Route::ToApp
        );
        assert_eq!(
            route_keystroke(false, false, &k("enter", false, true, false)),
            Route::ToEditor
        );
        assert_eq!(
            route_keystroke(false, false, &k("a", false, false, false)),
            Route::ToEditor
        );
        // B 区：补全打开时导航键进 App，关闭时放行
        assert_eq!(
            route_keystroke(false, true, &k("down", false, false, false)),
            Route::ToApp
        );
        assert_eq!(
            route_keystroke(false, false, &k("down", false, false, false)),
            Route::ToEditor
        );
        // 浮层压过补全
        assert_eq!(
            route_keystroke(true, true, &k("z", true, false, false)),
            Route::ToApp
        );
        // 打开键（task 6.3）：ctrl+l / alt+t 无论菜单状态都进 App
        assert_eq!(
            route_keystroke(false, false, &k("l", true, false, false)),
            Route::ToApp
        );
        assert_eq!(
            route_keystroke(false, true, &k("t", false, false, true)),
            Route::ToApp
        );
        // 非打开键的 ctrl/alt 组合仍交编辑器
        assert_eq!(
            route_keystroke(false, false, &k("l", false, false, false)),
            Route::ToEditor
        );
    }

    /// 浮层下滚轮被遮罩阻断（task 6.2；spec: 浮层下滚轮忽略）。
    ///
    /// 功能: 对照组——无浮层时滚轮推进回看锚点；实验组——浮层打开后同
    ///       一滚轮不再移动锚点（遮罩 occlude 阻断后方命中），浮层保持打开；
    /// 输入: 构造的长会话 + `window.scroll` 命中链；
    /// 输出: 断言通过（锚点先动后不动）。
    #[gpui_kit::test]
    fn overlay_blocks_wheel_on_messages(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "wheel-overlay");
        cx.update(|cx| {
            term.update(cx, |term, _| {
                for _ in 0..10 {
                    feed_user_lines(term, 60);
                }
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("prime render");
        cx.run_until_parked();

        // 钉到首行（锚点0）：向下滚才会动，动了才证明事件到达。
        cx.update(|cx| {
            term.update(cx, |term, cx| {
                let _ = term.scroller.update(cx, |sc, cx| sc.scroll_to_item(0, cx));
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();

        let delta = ScrollDelta::Pixels(point(px(0.), px(-120.)));

        // 对照组：无浮层，滚轮应推进锚点。
        cx.update_window(handle, |_, window, cx| {
            window.scroll("gpui-messages", delta, cx)
        })
        .expect("scroll control");
        cx.run_until_parked();
        let control = cx.update(|cx| term.read(cx).scroller.read(cx).logical_scroll_top());
        assert!(
            control.item_ix > 0 || f32::from(control.offset_in_item) > 0.,
            "对照组：无浮层时滚轮应推进锚点（{control:?}）"
        );

        // 复位锚点 + 打开浮层。
        cx.update(|cx| {
            term.update(cx, |term, cx| {
                let _ = term.scroller.update(cx, |sc, cx| sc.scroll_to_item(0, cx));
                term.state.app.overlay = Some(crate::app::overlay::Overlay::Info {
                    title: "Wheel".into(),
                    lines: vec![],
                    loading: false,
                    seq: 1,
                });
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render overlay");
        cx.run_until_parked();

        // 实验组：同一滚轮被遮罩阻断。
        cx.update_window(handle, |_, window, cx| {
            window.scroll("gpui-messages", delta, cx)
        })
        .expect("scroll blocked");
        cx.run_until_parked();
        let after = cx.update(|cx| term.read(cx).scroller.read(cx).logical_scroll_top());
        assert!(
            after.item_ix == 0 && f32::from(after.offset_in_item) == 0.,
            "浮层下滚轮不应移动消息区锚点（{after:?}）"
        );
        let still_open = cx.update(|cx| term.read(cx).state.app.overlay.is_some());
        assert!(still_open, "滚动不应关闭浮层");
    }

    /// 浮层打开时按键不漏入编辑器（task 6.2；spec: 悬浮层按键与滚轮路由）。
    ///
    /// 功能: 空闲打字进编辑器；开浮层后打字不改编辑器、导航键移浮层光标、
    ///       esc 关浮层且编辑器文本不变；
    /// 输入: 真实按键路径（press 经拦截层）；
    /// 输出: 断言通过（文本/光标/浮层开关逐步相等）。
    #[gpui_kit::test]
    fn overlay_keys_do_not_leak_to_editor(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "overlay-keys");
        // 空闲打字：非全局键进编辑器。
        cx.update_window(handle, |_, window, cx| {
            window.press("h", cx);
            window.press("i", cx);
        })
        .expect("type idle");
        cx.run_until_parked();
        let value = cx.update(|cx| term.read(cx).textarea.read(cx).value().to_string());
        assert_eq!(value, "hi", "空闲打字应进编辑器");

        // 打开两选项浮层（ModelSelect）。
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state.app.overlay = Some(crate::app::overlay::Overlay::ModelSelect {
                    items: vec![
                        crate::app::overlay::ModelItem {
                            provider: "p".into(),
                            id: "m1".into(),
                            name: "Model One".into(),
                        },
                        crate::app::overlay::ModelItem {
                            provider: "p".into(),
                            id: "m2".into(),
                            name: "Model Two".into(),
                        },
                    ],
                    loading: false,
                    cursor: 0,
                    seq: 1,
                });
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render overlay");
        cx.run_until_parked();

        // 浮层下打字：不改编辑器，浮层光标不动。
        cx.update_window(handle, |_, window, cx| window.press("x", cx))
            .expect("type under overlay");
        cx.run_until_parked();
        let (value, cursor) = cx.update(|cx| {
            let term = term.read(cx);
            (
                term.textarea.read(cx).value().to_string(),
                match &term.state.app.overlay {
                    Some(crate::app::overlay::Overlay::ModelSelect { cursor, .. }) => *cursor,
                    other => panic!("浮层应保持打开: {other:?}"),
                },
            )
        });
        assert_eq!(value, "hi", "浮层下打字不应漏入编辑器");
        assert_eq!(cursor, 0, "无效键不应移动浮层光标");

        // down：浮层光标下移，编辑器仍不变。
        cx.update_window(handle, |_, window, cx| window.press("down", cx))
            .expect("nav down");
        cx.run_until_parked();
        let (value, cursor) = cx.update(|cx| {
            let term = term.read(cx);
            (
                term.textarea.read(cx).value().to_string(),
                match &term.state.app.overlay {
                    Some(crate::app::overlay::Overlay::ModelSelect { cursor, .. }) => *cursor,
                    other => panic!("浮层应保持打开: {other:?}"),
                },
            )
        });
        assert_eq!(cursor, 1, "down 应移动浮层光标");
        assert_eq!(value, "hi", "导航键不应改编辑器");

        // esc：浮层关闭，编辑器不变。
        cx.update_window(handle, |_, window, cx| window.press("escape", cx))
            .expect("esc close");
        cx.run_until_parked();
        let (value, open) = cx.update(|cx| {
            let term = term.read(cx);
            (
                term.textarea.read(cx).value().to_string(),
                term.state.app.overlay.is_some(),
            )
        });
        assert!(!open, "esc 应关闭浮层");
        assert_eq!(value, "hi", "esc 不应改编辑器");
    }

    /// 等浮层异步回填完成（loading=false；上限2秒防挂）。
    ///
    /// 功能: 循环 run_until_parked + 泵 ui 事件，直到选择器离开 loading；
    /// 输入: cx — 测试上下文；term — 组件实体；
    /// 输出: 无返回；超时则停在 loading（后续 esc 断言会报错定位）。
    fn wait_overlay_loaded(cx: &mut TestAppContext, term: &gpui_kit::Entity<GPUIAgentTerminal>) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            cx.run_until_parked();
            cx.update(|cx| term.update(cx, |term, cx| term.pump_ui_events(cx)));
            let loading = cx.update(|cx| {
                matches!(
                    term.read(cx).state.app.overlay,
                    Some(
                        crate::app::overlay::Overlay::ThinkingSelect { loading: true, .. }
                            | crate::app::overlay::Overlay::ModelSelect { loading: true, .. }
                    )
                )
            });
            if !loading || std::time::Instant::now() >= deadline {
                break;
            }
        }
    }

    /// 打开键经拦截进共享分发（task 6.3；spec: 悬浮层入口）。
    ///
    /// 功能: alt+t → ThinkingSelect 打开（快照含标题）→ esc 关闭；
    ///       ctrl+l → ModelSelect 打开（镜像 key_intent，TUI 同键）→ esc 关闭；
    /// 输入: 真实按键路径 + fake pi（异步数据回填）；
    /// 输出: 断言通过（浮层变体与快照逐步相等）。
    #[gpui_kit::test]
    fn open_keys_route_to_selectors(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "open-keys");
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("prime");
        cx.run_until_parked();

        // alt+t → ThinkingSelect，标题入快照。
        cx.update_window(handle, |_, window, cx| window.press("alt-t", cx))
            .expect("alt-t");
        cx.run_until_parked();
        let is_thinking = cx.update(|cx| {
            matches!(
                term.read(cx).state.app.overlay,
                Some(crate::app::overlay::Overlay::ThinkingSelect { .. })
            )
        });
        assert!(is_thinking, "alt+t 应打开 ThinkingSelect");
        let texts = cx
            .update_window(handle, |_, window, _| {
                gpui_kit::base::test_support::texts(window)
            })
            .expect("texts");
        let joined = texts
            .iter()
            .map(|t| t.text().to_string())
            .collect::<Vec<_>>()
            .join("");
        assert!(joined.contains("Thinking level (alt+t)"), "快照应含标题: {joined:?}");

        // 等 fake-pi 回填（loading=false）；loading 中 overlay_key 吞键是
        // TUI 同款语义（SessionSelect 例外地允许 loading esc，不在本变体）。
        wait_overlay_loaded(cx, &term);
        cx.update_window(handle, |_, window, cx| window.press("escape", cx))
            .expect("esc close thinking");
        cx.run_until_parked();
        let open = cx.update(|cx| term.read(cx).state.app.overlay.is_some());
        assert!(!open, "esc 应关闭 ThinkingSelect");

        // ctrl+l → ModelSelect，标题入快照。
        cx.update_window(handle, |_, window, cx| window.press("ctrl-l", cx))
            .expect("ctrl-l");
        cx.run_until_parked();
        let is_model = cx.update(|cx| {
            matches!(
                term.read(cx).state.app.overlay,
                Some(crate::app::overlay::Overlay::ModelSelect { .. })
            )
        });
        assert!(is_model, "ctrl+l 应打开 ModelSelect");
        let texts = cx
            .update_window(handle, |_, window, _| {
                gpui_kit::base::test_support::texts(window)
            })
            .expect("texts");
        let joined = texts
            .iter()
            .map(|t| t.text().to_string())
            .collect::<Vec<_>>()
            .join("");
        assert!(joined.contains("Model (ctrl+l)"), "快照应含标题: {joined:?}");

        wait_overlay_loaded(cx, &term);
        cx.update_window(handle, |_, window, cx| window.press("escape", cx))
            .expect("esc close model");
        cx.run_until_parked();
        let open = cx.update(|cx| term.read(cx).state.app.overlay.is_some());
        assert!(!open, "esc 应关闭 ModelSelect");
    }

    /// 流式×浮层双开的语义序（task 6.4；spec: 悬浮层按键与滚轮路由）。
    ///
    /// 功能: 流式 + 浮层打开时——ctrl+c 走全局清空且浮层保持、首次 esc
    ///       只关浮层不发 abort、再按 esc 才走流式 abort（分支优先级锁定）；
    /// 输入: 录制型 fake pi（命令落盘）+ `agent_start` 注入流式；
    /// 输出: 断言通过（浮层开关/输入清空/日志命令序列逐步相等）。
    #[gpui_kit::test]
    fn streaming_overlay_key_priority(cx: &mut TestAppContext) {
        let (path, log) = write_recording_stub("overlay-order");
        let session = RpcSession::spawn(&path, &[], None).expect("spawn 应成功");
        let (handle, term) = open_with_session(cx, session);

        // 进入流式（reduce 注入 agent_start）。
        cx.update(|cx| {
            term.update(cx, |term, _| {
                let raw = serde_json::json!({"type": "agent_start"});
                term.state
                    .reduce(&ClientEvent::Event(raw), &term.commander, &term.ui_tx);
            });
        });

        // 输入非空 + 打开浮层。
        cx.update_window(handle, |_, window, cx| window.input("keep me", cx))
            .expect("type");
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state.app.overlay = Some(crate::app::overlay::Overlay::Info {
                    title: "Order".into(),
                    lines: vec![],
                    loading: false,
                    seq: 1,
                });
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();

        // ctrl+c：全局清空输入，浮层保持（ctrl+c 分支先于浮层分支）。
        cx.update_window(handle, |_, window, cx| window.press("ctrl-c", cx))
            .expect("ctrl-c");
        cx.run_until_parked();
        let (value, open) = cx.update(|cx| {
            let term = term.read(cx);
            (
                term.textarea.read(cx).value().to_string(),
                term.state.app.overlay.is_some(),
            )
        });
        assert_eq!(value, "", "ctrl+c 应清空输入");
        assert!(open, "ctrl+c 浮层之下仍全局，浮层应保持打开");

        // 首次 esc：只关浮层，不发 abort。
        cx.update_window(handle, |_, window, cx| window.press("escape", cx))
            .expect("esc close overlay");
        cx.run_until_parked();
        let open = cx.update(|cx| term.read(cx).state.app.overlay.is_some());
        assert!(!open, "esc 应先关浮层");
        std::thread::sleep(std::time::Duration::from_millis(100));
        cx.run_until_parked();
        let content = fs::read_to_string(&log).unwrap_or_default();
        assert!(
            !content.contains(r#""type":"abort""#),
            "关浮层不应发 abort: {content}"
        );

        // 再按 esc：流式 → abort 落盘。
        cx.update_window(handle, |_, window, cx| window.press("escape", cx))
            .expect("esc abort");
        assert!(
            wait_log(cx, &log, r#""type":"abort""#, 3000),
            "再按 esc 应发 abort: log={:?}",
            fs::read_to_string(&log).unwrap_or_default()
        );
    }

    /// 可开浮层命令逐个开对应浮层（task 7.3；spec: 斜杠可开浮层命令）。
    ///
    /// 功能: 表驱动经真实提交路径逐命令断言浮层变体匹配、且无“暂不支持”
    ///       桩（/import 用临时 .jsonl 路径触发二次确认浮层）；
    /// 输入: 构造的命令文本 + fake pi；
    /// 输出: 断言通过（逐命令变体名相等、无桩）。
    #[gpui_kit::test]
    fn slash_overlay_commands_open_selectors(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "slash-overlay");
        let tmp = std::env::temp_dir().join(format!(
            "gpui-import-{}.jsonl",
            std::process::id()
        ));
        fs::write(&tmp, "{\"type\":\"session\",\"cwd\":\"/tmp\"}\n").unwrap();
        let cases: Vec<(String, &'static str)> = vec![
            ("/tree".into(), "Tree"),
            ("/fork".into(), "Tree"),
            ("/session".into(), "Info"),
            ("/resume".into(), "SessionSelect"),
            ("/settings".into(), "Settings"),
            ("/hotkeys".into(), "Hotkeys"),
            ("/trust".into(), "Trust"),
            ("/scoped-models".into(), "ScopedModels"),
            (format!("/import {}", tmp.display()), "ImportConfirm"),
        ];

        fn overlay_name(
            cx: &mut TestAppContext,
            term: &gpui_kit::Entity<GPUIAgentTerminal>,
        ) -> Option<&'static str> {
            cx.update(|cx| match &term.read(cx).state.app.overlay {
                Some(crate::app::overlay::Overlay::Tree { .. }) => Some("Tree"),
                Some(crate::app::overlay::Overlay::Info { .. }) => Some("Info"),
                Some(crate::app::overlay::Overlay::SessionSelect { .. }) => {
                    Some("SessionSelect")
                }
                Some(crate::app::overlay::Overlay::Settings { .. }) => Some("Settings"),
                Some(crate::app::overlay::Overlay::Hotkeys { .. }) => Some("Hotkeys"),
                Some(crate::app::overlay::Overlay::Trust { .. }) => Some("Trust"),
                Some(crate::app::overlay::Overlay::ScopedModels { .. }) => {
                    Some("ScopedModels")
                }
                Some(crate::app::overlay::Overlay::ImportConfirm { .. }) => {
                    Some("ImportConfirm")
                }
                Some(crate::app::overlay::Overlay::ModelSelect { .. }) => Some("ModelSelect"),
                Some(crate::app::overlay::Overlay::ThinkingSelect { .. }) => {
                    Some("ThinkingSelect")
                }
                _ => None,
            })
        }

        for (cmd, want) in &cases {
            // 清场：关浮层 + 清提示。
            cx.update(|cx| {
                term.update(cx, |term, _| {
                    term.state.app.overlay = None;
                    term.state.app.notice = None;
                });
            });
            cx.update_window(handle, |_, window, cx| {
                window.input(cmd, cx);
                window.press("enter", cx);
            })
            .unwrap_or_else(|e| panic!("submit {cmd}: {e}"));
            cx.run_until_parked();
            let got = overlay_name(cx, &term);
            assert_eq!(got, Some(*want), "{cmd} 应开 {want} 浮层");
            let stubbed = cx.update(|cx| {
                term.read(cx)
                    .state
                    .app
                    .notice
                    .as_ref()
                    .map(|(t, _)| t.contains("暂不支持"))
                    .unwrap_or(false)
            });
            assert!(!stubbed, "{cmd} 不应给未支持桩");
        }

        let _ = fs::remove_file(&tmp);
    }

    /// 补全菜单滚动窗口跟随选中（task 9.1；spec: slash-autocomplete 超出上限滚动）。
    ///
    /// 功能: 候选 >5 时 ↓ 越窗——选中项入窗、首项滚出窗口、计数行 (i/n)
    ///       同步（菜单过滤快照行等判定，避免子串误命中）；
    /// 输入: 真实按键路径（`/` 打开全量命令候选）；
    /// 输出: 断言通过（选中态/窗口内容/计数逐步相等）。
    #[gpui_kit::test]
    fn completion_menu_scrolls_selection_into_view(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "comp-scroll");
        cx.update_window(handle, |_, window, cx| window.input("/", cx))
            .expect("type slash");
        cx.run_until_parked();
        let item_name = |items: &[crate::app::autocomplete::CompletionItem], i: usize| {
            let it = &items[i];
            if it.label.is_empty() {
                it.value.clone()
            } else {
                it.label.clone()
            }
        };
        let (total, first_label, selected0) = cx.update(|cx| {
            let c = &term.read(cx).state.app.completion;
            assert!(c.open, "/ 应打开补全菜单");
            assert!(c.items.len() > 6, "候选应 >6: {}", c.items.len());
            let first = item_name(&c.items, 0);
            (c.items.len(), first, c.selected)
        });

        // ↓ ×6 → 越窗（max=5，窗口必须跟随）。
        for _ in 0..6 {
            cx.update_window(handle, |_, window, cx| window.press("down", cx))
                .expect("down");
        }
        cx.run_until_parked();
        let target = selected0 + 6;
        assert!(target >= 5, "测试前提：选中应越过窗口上限（target={target}）");
        let (selected, wanted) = cx.update(|cx| {
            let c = &term.read(cx).state.app.completion;
            (c.selected, item_name(&c.items, target))
        });
        assert_eq!(selected, target, "↓×6 应移选中");

        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();
        let texts = cx
            .update_window(handle, |_, window, _| {
                gpui_kit::base::test_support::texts(window)
            })
            .expect("texts");
        let menu: String = texts
            .iter()
            .filter(|t| t.path().contains(&ElementId::Name("gpui-completion".into())))
            .map(|t| t.text().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            menu.lines().any(|l| l.trim() == wanted.trim()),
            "选中项应入窗: wanted={wanted:?} menu={menu:?}"
        );
        assert!(
            !menu.lines().any(|l| l.trim() == first_label.trim()),
            "首项应滚出窗口: first={first_label:?} menu={menu:?}"
        );
        assert!(
            menu.contains(&format!("({}/{})", target + 1, total)),
            "计数行应同步: menu={menu:?}"
        );
    }

    /// ModelSelect 渲染/导航/应用/关闭（task 4.1；spec: 悬浮层渲染）。
    ///
    /// 功能: 注入两模型 → 快照含标题与选中前缀 → down 移光标 → enter 应用
    ///       （set_model 落盘 + 浮层关）→ 再开 → esc 关；
    /// 输入: 录制型 fake pi + 状态注入 + 真实按键；
    /// 输出: 断言通过（快照/光标/命令序列逐步相等）。
    #[gpui_kit::test]
    fn model_select_render_nav_apply(cx: &mut TestAppContext) {
        let (path, log) = write_recording_stub("model-select");
        let session = RpcSession::spawn(&path, &[], None).expect("spawn 应成功");
        let (handle, term) = open_with_session(cx, session);
        let item = |id: &str, name: &str| crate::app::overlay::ModelItem {
            provider: "prov".into(),
            id: id.into(),
            name: name.into(),
        };
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state.app.overlay = Some(crate::app::overlay::Overlay::ModelSelect {
                    items: vec![item("m1", "Model One"), item("m2", "Model Two")],
                    loading: false,
                    cursor: 0,
                    seq: 1,
                });
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();
        let snap = |cx: &mut TestAppContext, handle: _| {
            cx.update_window(handle, |_, window, _| {
                gpui_kit::base::test_support::texts(window)
            })
            .expect("texts")
            .iter()
            .map(|t| t.text().to_string())
            .collect::<Vec<_>>()
            .join("\n")
        };
        let joined = snap(cx, handle);
        assert!(joined.contains("Model (ctrl+l)"), "标题: {joined:?}");
        assert!(joined.contains("> Model One"), "选中行: {joined:?}");
        assert!(joined.contains("Model Two"), "次行: {joined:?}");

        // down → 光标移到第二项，快照前缀跟随。
        cx.update_window(handle, |_, window, cx| window.press("down", cx))
            .expect("down");
        cx.run_until_parked();
        let cursor = cx.update(|cx| match &term.read(cx).state.app.overlay {
            Some(crate::app::overlay::Overlay::ModelSelect { cursor, .. }) => *cursor,
            other => panic!("浮层应保持: {other:?}"),
        });
        assert_eq!(cursor, 1, "down 应移光标");
        let joined = snap(cx, handle);
        assert!(joined.contains("> Model Two"), "快照前缀应跟随: {joined:?}");

        // enter → 应用 set_model(m2) 并关浮层。
        cx.update_window(handle, |_, window, cx| window.press("enter", cx))
            .expect("apply");
        assert!(
            wait_log(cx, &log, r#""modelId":"m2""#, 3000),
            "enter 应发 set_model(m2): log={:?}",
            fs::read_to_string(&log).unwrap_or_default()
        );
        cx.run_until_parked();
        let open = cx.update(|cx| term.read(cx).state.app.overlay.is_some());
        assert!(!open, "应用后应关浮层");

        // 再开 → esc 关闭。
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state.app.overlay = Some(crate::app::overlay::Overlay::ModelSelect {
                    items: vec![item("m1", "Model One")],
                    loading: false,
                    cursor: 0,
                    seq: 2,
                });
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| window.press("escape", cx))
            .expect("esc");
        cx.run_until_parked();
        let open = cx.update(|cx| term.read(cx).state.app.overlay.is_some());
        assert!(!open, "esc 应关浮层");
    }

    /// ThinkingSelect 与 ScopedModels 冒烟（task 4.2；spec: 悬浮层渲染）。
    ///
    /// 功能: Thinking loading→已加载→导航→esc；ScopedModels 说明行+列表+esc；
    /// 输入: 状态注入 + 真实按键；
    /// 输出: 断言通过（快照与光标/开关逐步相等）。
    #[gpui_kit::test]
    fn thinking_and_scoped_select_smoke(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "thinking-scoped");
        let snap = |cx: &mut TestAppContext, handle: _| {
            cx.update_window(handle, |_, window, _| {
                gpui_kit::base::test_support::texts(window)
            })
            .expect("texts")
            .iter()
            .map(|t| t.text().to_string())
            .collect::<Vec<_>>()
            .join("\n")
        };

        // Thinking loading 相位。
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state.app.overlay = Some(crate::app::overlay::Overlay::ThinkingSelect {
                    levels: vec![],
                    loading: true,
                    cursor: 0,
                    seq: 1,
                });
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render loading");
        cx.run_until_parked();
        let joined = snap(cx, handle);
        assert!(
            joined.contains("Thinking level (alt+t)") && joined.contains("loading…"),
            "loading 相位: {joined:?}"
        );

        // 已加载 + 导航。
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state.app.overlay = Some(crate::app::overlay::Overlay::ThinkingSelect {
                    levels: vec!["off".into(), "low".into(), "high".into()],
                    loading: false,
                    cursor: 0,
                    seq: 1,
                });
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render loaded");
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| window.press("down", cx))
            .expect("down");
        cx.run_until_parked();
        let cursor = cx.update(|cx| match &term.read(cx).state.app.overlay {
            Some(crate::app::overlay::Overlay::ThinkingSelect { cursor, .. }) => *cursor,
            other => panic!("浮层应保持: {other:?}"),
        });
        assert_eq!(cursor, 1, "down 应移光标");
        let joined = snap(cx, handle);
        assert!(joined.contains("> low"), "选中行: {joined:?}");
        cx.update_window(handle, |_, window, cx| window.press("escape", cx))
            .expect("esc");
        cx.run_until_parked();
        let open = cx.update(|cx| term.read(cx).state.app.overlay.is_some());
        assert!(!open, "esc 应关 ThinkingSelect");

        // ScopedModels：说明行 + 只读列表。
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state.app.overlay = Some(crate::app::overlay::Overlay::ScopedModels {
                    models: vec!["prov/m1".into(), "prov/m2".into()],
                    loading: false,
                    seq: 1,
                    cursor: 1,
                });
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render scoped");
        cx.run_until_parked();
        let joined = snap(cx, handle);
        assert!(joined.contains("Scoped Models（只读）"), "标题: {joined:?}");
        assert!(joined.contains("本列表只读"), "说明行: {joined:?}");
        assert!(joined.contains("prov/m2"), "列表行: {joined:?}");
        cx.update_window(handle, |_, window, cx| window.press("escape", cx))
            .expect("esc scoped");
        cx.run_until_parked();
        let open = cx.update(|cx| term.read(cx).state.app.overlay.is_some());
        assert!(!open, "esc 应关 ScopedModels");
    }

    /// SessionSelect 渲染/两段确认/关闭（task 4.3；spec: 悬浮层渲染）。
    ///
    /// 功能: 注入会话列表（当前会话标记 + 标题）→ 确认态标题 → esc 退确认
    ///       态（浮层保持）→ 再 esc 关；
    /// 输入: 状态注入 + 真实按键；
    /// 输出: 断言通过（快照/确认态/开关逐步相等）。
    #[gpui_kit::test]
    fn session_select_render_and_confirm(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "session-select");
        let item = crate::app::sessions::SessionItem {
            path: std::path::PathBuf::from("/tmp/sess-a.jsonl"),
            name: Some("Session A".into()),
            first_message: "first msg".into(),
            modified: Some(std::time::SystemTime::now()),
            cwd: "/tmp".into(),
        };
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state.app.session_file = Some("/tmp/sess-a.jsonl".into());
                term.state.app.overlay = Some(crate::app::overlay::Overlay::SessionSelect {
                    items: vec![item],
                    cursor: 0,
                    loading: false,
                    seq: 1,
                    confirming: false,
                });
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();
        let snap = |cx: &mut TestAppContext, handle: _| {
            cx.update_window(handle, |_, window, _| {
                gpui_kit::base::test_support::texts(window)
            })
            .expect("texts")
            .iter()
            .map(|t| t.text().to_string())
            .collect::<Vec<_>>()
            .join("\n")
        };
        let joined = snap(cx, handle);
        assert!(joined.contains("Resume (Enter 切换"), "标题: {joined:?}");
        assert!(joined.contains("• > Session A"), "当前标记+选中行: {joined:?}");

        // 确认态标题切换（直接注入确认态）→ esc 只退确认。
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state.app.overlay = Some(crate::app::overlay::Overlay::SessionSelect {
                    items: vec![],
                    cursor: 0,
                    loading: false,
                    seq: 1,
                    confirming: true,
                });
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render confirming");
        cx.run_until_parked();
        let joined = snap(cx, handle);
        assert!(joined.contains("再按 ctrl+d 确认删除"), "确认态标题: {joined:?}");
        cx.update_window(handle, |_, window, cx| window.press("escape", cx))
            .expect("esc exit confirm");
        cx.run_until_parked();
        let (open, confirming) = cx.update(|cx| match &term.read(cx).state.app.overlay {
            Some(crate::app::overlay::Overlay::SessionSelect { confirming, .. }) => {
                (true, *confirming)
            }
            _ => (false, false),
        });
        assert!(open && !confirming, "首次 esc 应只退确认态（open={open} confirming={confirming}）");
        cx.update_window(handle, |_, window, cx| window.press("escape", cx))
            .expect("esc close");
        cx.run_until_parked();
        let open = cx.update(|cx| term.read(cx).state.app.overlay.is_some());
        assert!(!open, "再次 esc 应关浮层");
    }

    /// Tree 渲染各相位（task 4.5；spec: 悬浮层渲染）。
    ///
    /// 功能: 树行（缩进/标记/当前叶子）+ 标题 → 零命中搜索警示行 → esc 关；
    /// 输入: 状态注入 + 真实按键；
    /// 输出: 断言通过（快照逐步相等）。
    #[gpui_kit::test]
    fn tree_render_states(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "tree");
        let snap = |cx: &mut TestAppContext, handle: _| {
            cx.update_window(handle, |_, window, _| {
                gpui_kit::base::test_support::texts(window)
            })
            .expect("texts")
            .iter()
            .map(|t| t.text().to_string())
            .collect::<Vec<_>>()
            .join("\n")
        };
        let row = |id: &str, depth: usize, text: &str, leaf: bool, kids: bool| {
            crate::app::tree::TreeRow {
                id: id.into(),
                depth,
                text: text.into(),
                labeled: false,
                is_leaf: leaf,
                has_children: kids,
            }
        };
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state.app.overlay = Some(crate::app::overlay::Overlay::Tree {
                    tree: vec![],
                    leaf_id: Some("n2".into()),
                    rows: vec![
                        row("n1", 0, "root branch", false, true),
                        row("n2", 1, "leaf session", true, false),
                    ],
                    cursor: 1,
                    folded: Default::default(),
                    filter: crate::app::tree::FilterMode::Default,
                    search: String::new(),
                    loading: false,
                    seq: 1,
                });
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();
        let joined = snap(cx, handle);
        assert!(joined.contains("Tree (filter:default)"), "标题: {joined:?}");
        assert!(joined.contains("Enter=分支"), "标题提示: {joined:?}");
        assert!(joined.contains("▾ root branch"), "分支行: {joined:?}");
        assert!(
            joined.contains(">   leaf session ◀ 当前") || joined.contains("leaf session ◀ 当前"),
            "当前叶子行: {joined:?}"
        );

        // 零命中搜索 → 黄字警示行（保留查询于标题）。
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state.app.overlay = Some(crate::app::overlay::Overlay::Tree {
                    tree: vec![],
                    leaf_id: None,
                    rows: vec![],
                    cursor: 0,
                    folded: Default::default(),
                    filter: crate::app::tree::FilterMode::Default,
                    search: "zzz".into(),
                    loading: false,
                    seq: 1,
                });
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render search");
        cx.run_until_parked();
        let joined = snap(cx, handle);
        assert!(joined.contains("search:\"zzz\""), "搜索标题: {joined:?}");
        assert!(joined.contains("无匹配"), "零命中警示: {joined:?}");
        // esc 先清搜索（TUI 语义），查询已空再按才关。
        cx.update_window(handle, |_, window, cx| window.press("escape", cx))
            .expect("esc clear search");
        cx.run_until_parked();
        let (open, search) = cx.update(|cx| match &term.read(cx).state.app.overlay {
            Some(crate::app::overlay::Overlay::Tree { search, .. }) => (true, search.clone()),
            _ => (false, String::new()),
        });
        assert!(open && search.is_empty(), "首次 esc 应只清搜索（open={open} search={search:?}）");
        cx.update_window(handle, |_, window, cx| window.press("escape", cx))
            .expect("esc close tree");
        cx.run_until_parked();
        let open = cx.update(|cx| term.read(cx).state.app.overlay.is_some());
        assert!(!open, "再次 esc 应关 Tree");
    }

    /// Hotkeys 键位表渲染（task 4.6；spec: 悬浮层渲染）。
    ///
    /// 功能: 注入键位表 → 快照含分组头与键位行 → esc 关；
    /// 输入: 状态注入 + 真实按键；
    /// 输出: 断言通过（快照/开关逐步相等）。
    #[gpui_kit::test]
    fn hotkeys_table_renders(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "hotkeys");
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state.app.overlay = Some(crate::app::overlay::Overlay::Hotkeys { cursor: 0 });
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();
        let joined = cx
            .update_window(handle, |_, window, _| {
                gpui_kit::base::test_support::texts(window)
            })
            .expect("texts")
            .iter()
            .map(|t| t.text().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("Hotkeys"), "标题: {joined:?}");
        assert!(joined.contains("── 提交与中断 ──"), "分组头: {joined:?}");
        assert!(
            joined.contains("发送（流式中发送排队消息）"),
            "键位行: {joined:?}"
        );
        cx.update_window(handle, |_, window, cx| window.press("escape", cx))
            .expect("esc");
        cx.run_until_parked();
        let open = cx.update(|cx| term.read(cx).state.app.overlay.is_some());
        assert!(!open, "esc 应关 Hotkeys");
    }

    /// Settings 浮层渲染/导航/关闭（task 5.1；spec: 悬浮层渲染）。
    ///
    /// 功能: loading 相位 → 已加载行（开关值+提示行）→ down 移光标 → esc 关；
    /// 输入: 状态注入 + 真实按键；
    /// 输出: 断言通过（快照/光标/开关逐步相等）。
    #[gpui_kit::test]
    fn settings_smoke(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "settings");
        let snap = |cx: &mut TestAppContext, handle: _| {
            cx.update_window(handle, |_, window, _| {
                gpui_kit::base::test_support::texts(window)
            })
            .expect("texts")
            .iter()
            .map(|t| t.text().to_string())
            .collect::<Vec<_>>()
            .join("\n")
        };
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state.app.overlay = Some(crate::app::overlay::Overlay::Settings {
                    cursor: 0,
                    loading: true,
                    seq: 1,
                    auto_compaction: None,
                    auto_retry: None,
                    double_esc: None,
                    model: None,
                });
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render loading");
        cx.run_until_parked();
        let joined = snap(cx, handle);
        assert!(
            joined.contains("Settings（简化）") && joined.contains("loading…"),
            "loading 相位: {joined:?}"
        );

        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state.app.overlay = Some(crate::app::overlay::Overlay::Settings {
                    cursor: 0,
                    loading: false,
                    seq: 1,
                    auto_compaction: Some(true),
                    auto_retry: None,
                    double_esc: Some("tree".into()),
                    model: Some("prov/m1".into()),
                });
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render loaded");
        cx.run_until_parked();
        let joined = snap(cx, handle);
        assert!(joined.contains("autoCompact"), "行1: {joined:?}");
        assert!(joined.contains("autoRetry"), "行2: {joined:?}");
        assert!(joined.contains("doubleEsc"), "行3: {joined:?}");
        assert!(joined.contains("prov/m1"), "只读模型行: {joined:?}");
        assert!(joined.contains("enter 切换开关"), "提示行: {joined:?}");

        cx.update_window(handle, |_, window, cx| window.press("down", cx))
            .expect("down");
        cx.run_until_parked();
        let cursor = cx.update(|cx| match &term.read(cx).state.app.overlay {
            Some(crate::app::overlay::Overlay::Settings { cursor, .. }) => *cursor,
            other => panic!("浮层应保持: {other:?}"),
        });
        assert_eq!(cursor, 1, "down 应移光标");
        cx.update_window(handle, |_, window, cx| window.press("escape", cx))
            .expect("esc");
        cx.run_until_parked();
        let open = cx.update(|cx| term.read(cx).state.app.overlay.is_some());
        assert!(!open, "esc 应关 Settings");
    }

    /// Trust 与 ImportConfirm 渲染/关闭（task 5.2；spec: 悬浮层渲染）。
    ///
    /// 功能: Trust 标题含当前态+选项行；ImportConfirm 标题含文件名+两选项；
    ///       各自 esc 关闭；
    /// 输入: 状态注入 + 真实按键；
    /// 输出: 断言通过（快照/开关逐步相等）。
    #[gpui_kit::test]
    fn trust_import_render_and_close(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "trust-import");
        let snap = |cx: &mut TestAppContext, handle: _| {
            cx.update_window(handle, |_, window, _| {
                gpui_kit::base::test_support::texts(window)
            })
            .expect("texts")
            .iter()
            .map(|t| t.text().to_string())
            .collect::<Vec<_>>()
            .join("\n")
        };
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state.app.overlay = Some(crate::app::overlay::Overlay::Trust {
                    cursor: 0,
                    current: Some(true),
                });
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render trust");
        cx.run_until_parked();
        let joined = snap(cx, handle);
        assert!(joined.contains("Trust（当前: 已信任）"), "标题: {joined:?}");
        assert!(joined.contains("✓ 信任本项目"), "选项: {joined:?}");
        cx.update_window(handle, |_, window, cx| window.press("escape", cx))
            .expect("esc trust");
        cx.run_until_parked();
        let open = cx.update(|cx| term.read(cx).state.app.overlay.is_some());
        assert!(!open, "esc 应关 Trust");

        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state.app.overlay = Some(crate::app::overlay::Overlay::ImportConfirm {
                    path: "/tmp/backup.jsonl".into(),
                    cursor: 0,
                });
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render import");
        cx.run_until_parked();
        let joined = snap(cx, handle);
        assert!(
            joined.contains("Import backup.jsonl（替换当前会话）"),
            "标题含文件名: {joined:?}"
        );
        assert!(joined.contains("✓ 导入并切换"), "选项: {joined:?}");
        cx.update_window(handle, |_, window, cx| window.press("escape", cx))
            .expect("esc import");
        cx.run_until_parked();
        let open = cx.update(|cx| term.read(cx).state.app.overlay.is_some());
        assert!(!open, "esc 应关 ImportConfirm");
    }

    /// ExtConfirm 与 ExtInput 渲染/取消回发（task 5.3；spec: 悬浮层渲染）。
    ///
    /// 功能: 确认对话（消息+双按钮+提示）与输入对话（占位/缓冲）渲染；
    ///       esc 取消时回发 cancelled 载荷（extension_ui_response）；
    /// 输入: 状态注入 + 录制型 fake pi；
    /// 输出: 断言通过（快照与回发落盘逐步相等）。
    #[gpui_kit::test]
    fn ext_confirm_input_render_and_cancel(cx: &mut TestAppContext) {
        let (path, log) = write_recording_stub("ext-confirm");
        let session = RpcSession::spawn(&path, &[], None).expect("spawn 应成功");
        let (handle, term) = open_with_session(cx, session);
        let snap = |cx: &mut TestAppContext, handle: _| {
            cx.update_window(handle, |_, window, _| {
                gpui_kit::base::test_support::texts(window)
            })
            .expect("texts")
            .iter()
            .map(|t| t.text().to_string())
            .collect::<Vec<_>>()
            .join("\n")
        };

        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state.app.overlay = Some(crate::app::overlay::Overlay::ExtConfirm {
                    id: "c1".into(),
                    title: "Confirm".into(),
                    message: "Do the thing?".into(),
                    confirm_selected: true,
                });
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render confirm");
        cx.run_until_parked();
        let joined = snap(cx, handle);
        assert!(joined.contains("Confirm"), "标题: {joined:?}");
        assert!(joined.contains("Do the thing?"), "消息: {joined:?}");
        assert!(joined.contains("确认") && joined.contains("取消"), "按钮: {joined:?}");
        assert!(joined.contains("←/→/Tab 切换"), "提示: {joined:?}");
        cx.update_window(handle, |_, window, cx| window.press("escape", cx))
            .expect("esc confirm");
        cx.run_until_parked();
        assert!(
            wait_log(cx, &log, r#""id":"c1","cancelled":true"#, 3000),
            "esc 应回发 cancelled: log={:?}",
            fs::read_to_string(&log).unwrap_or_default()
        );
        let open = cx.update(|cx| term.read(cx).state.app.overlay.is_some());
        assert!(!open, "esc 应关 ExtConfirm");

        // ExtInput：空缓冲显占位，非空显缓冲。
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state.app.overlay = Some(crate::app::overlay::Overlay::ExtInput {
                    id: "i1".into(),
                    title: "Prompt".into(),
                    placeholder: "type here…".into(),
                    buffer: String::new(),
                });
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render input empty");
        cx.run_until_parked();
        let joined = snap(cx, handle);
        assert!(joined.contains("Prompt (Enter 提交"), "标题: {joined:?}");
        assert!(joined.contains("type here…"), "占位: {joined:?}");

        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state.app.overlay = Some(crate::app::overlay::Overlay::ExtInput {
                    id: "i1".into(),
                    title: "Prompt".into(),
                    placeholder: "type here…".into(),
                    buffer: "draft text".into(),
                });
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render input filled");
        cx.run_until_parked();
        let joined = snap(cx, handle);
        assert!(joined.contains("draft text"), "缓冲: {joined:?}");
        cx.update_window(handle, |_, window, cx| window.press("escape", cx))
            .expect("esc input");
        cx.run_until_parked();
        assert!(
            wait_log(cx, &log, r#""id":"i1","cancelled":true"#, 3000),
            "esc 应回发 cancelled: log={:?}",
            fs::read_to_string(&log).unwrap_or_default()
        );
    }

    /// ExtEditor 多行编辑渲染（task 5.4；spec: 悬浮层渲染）。
    ///
    /// 功能: 多行内容 + 当前行字符光标高亮块入快照；esc 取消回发；
    /// 输入: 状态注入 + 录制型 fake pi；
    /// 输出: 断言通过（快照与回发落盘逐步相等）。
    #[gpui_kit::test]
    fn ext_editor_render_and_cancel(cx: &mut TestAppContext) {
        let (path, log) = write_recording_stub("ext-editor");
        let session = RpcSession::spawn(&path, &[], None).expect("spawn 应成功");
        let (handle, term) = open_with_session(cx, session);
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state.app.overlay = Some(crate::app::overlay::Overlay::ExtEditor {
                    id: "e1".into(),
                    title: "Editor".into(),
                    lines: vec!["hello".into(), "world".into()],
                    row: 1,
                    col: 1,
                });
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();
        let joined = cx
            .update_window(handle, |_, window, _| {
                gpui_kit::base::test_support::texts(window)
            })
            .expect("texts")
            .iter()
            .map(|t| t.text().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains("Editor (Enter 提交 · Alt+Enter 换行"),
            "标题: {joined:?}"
        );
        assert!(joined.contains("hello"), "行1: {joined:?}");
        assert!(joined.contains("w▏orld"), "当前行光标切分: {joined:?}");
        cx.update_window(handle, |_, window, cx| window.press("escape", cx))
            .expect("esc");
        cx.run_until_parked();
        assert!(
            wait_log(cx, &log, r#""id":"e1","cancelled":true"#, 3000),
            "esc 应回发 cancelled: log={:?}",
            fs::read_to_string(&log).unwrap_or_default()
        );
        let open = cx.update(|cx| term.read(cx).state.app.overlay.is_some());
        assert!(!open, "esc 应关 ExtEditor");
    }

    /// 扩展请求全链路：事件 → 浮层 → 键驱动 → 回发（task 5.5；spec: 悬浮层入口）。
    ///
    /// 功能: apply_core_event 收 extension_ui_request(select) → ExtSelect 入队
    ///       并渲染 → down+enter 回发选中值；再发 select → esc 回发 cancelled；
    ///       被动 notify 不开浮层只置提示；
    /// 输入: 原始事件 JSON + 录制型 fake pi + 真实按键；
    /// 输出: 断言通过（浮层/快照/回发序列逐步相等）。
    #[gpui_kit::test]
    fn extension_request_chain_builds_dialog_and_replies(cx: &mut TestAppContext) {
        let (path, log) = write_recording_stub("ext-chain");
        let session = RpcSession::spawn(&path, &[], None).expect("spawn 应成功");
        let (handle, term) = open_with_session(cx, session);

        // select 请求 → ExtSelect 入队。
        let raw = serde_json::json!({
            "type": "extension_ui_request",
            "id": "s1",
            "method": "select",
            "title": "Pick one",
            "options": ["Alpha", "Beta"]
        });
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state
                    .app
                    .apply_core_event(&raw, &term.commander, &term.ui_tx);
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render ext select");
        cx.run_until_parked();
        let joined = cx
            .update_window(handle, |_, window, _| {
                gpui_kit::base::test_support::texts(window)
            })
            .expect("texts")
            .iter()
            .map(|t| t.text().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("Pick one"), "标题: {joined:?}");
        assert!(joined.contains("Alpha") && joined.contains("Beta"), "选项: {joined:?}");

        // down + enter → 回发选中值 Beta 并关浮层。
        cx.update_window(handle, |_, window, cx| {
            window.press("down", cx);
        })
        .expect("down");
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| window.press("enter", cx))
            .expect("apply");
        assert!(
            wait_log(cx, &log, r#""id":"s1","value":"Beta""#, 3000),
            "enter 应回发选中值: log={:?}",
            fs::read_to_string(&log).unwrap_or_default()
        );
        cx.run_until_parked();
        let open = cx.update(|cx| term.read(cx).state.app.overlay.is_some());
        assert!(!open, "应用后应关浮层");

        // 第二个 select → esc 回发 cancelled。
        let raw2 = serde_json::json!({
            "type": "extension_ui_request",
            "id": "s2",
            "method": "select",
            "title": "Pick two",
            "options": ["X"]
        });
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state
                    .app
                    .apply_core_event(&raw2, &term.commander, &term.ui_tx);
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render ext select 2");
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| window.press("escape", cx))
            .expect("cancel");
        assert!(
            wait_log(cx, &log, r#""id":"s2","cancelled":true"#, 3000),
            "esc 应回发 cancelled: log={:?}",
            fs::read_to_string(&log).unwrap_or_default()
        );

        // 被动 notify：不开浮层（立即应用，只置提示）。
        let raw3 = serde_json::json!({
            "type": "extension_ui_request",
            "id": "n1",
            "method": "notify",
            "message": "hello ext"
        });
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state
                    .app
                    .apply_core_event(&raw3, &term.commander, &term.ui_tx);
            });
        });
        cx.run_until_parked();
        let (open, notice) = cx.update(|cx| {
            let term = term.read(cx);
            (term.state.app.overlay.is_some(), term.state.app.notice.is_some())
        });
        assert!(!open, "被动请求不应开浮层");
        assert!(notice, "notify 应置提示");
    }

    /// 三分区占位布局（spec: 三分区布局与状态指示 —— 无头 ElementSnapshot 断言）。
    ///
    /// 功能: 无头窗口渲染组件，断言消息区/状态行/输入区三者在场、
    ///       垂直堆叠（消息区在上、状态行居中、输入区在底）、宽度一致；
    /// 输入: 无（fake pi 提供会话）；
    /// 输出: 断言通过（测试结束实体 Drop 回收 fake pi）。
    #[gpui_kit::test]
    fn three_pane_layout_splits_window(cx: &mut TestAppContext) {
        let path = write_stub("layout");
        let session = RpcSession::spawn(&path, &[], None).expect("spawn 应成功");

        cx.update(gpui_kit::init);
        let (handle, _) = cx.update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Point::default(),
                        size: size(px(800.), px(600.)),
                    })),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| GPUIAgentTerminal::new(session, window, cx)),
            )
            .expect("open test window")
        });

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
        })
        .expect("render frame");
        cx.run_until_parked();

        let (messages, status, input) = cx
            .update_window(handle, |_, window, _| {
                (
                    window.find("gpui-messages").bounds(),
                    window.find("gpui-status").bounds(),
                    window.find("gpui-input").bounds(),
                )
            })
            .expect("find panes");

        let m_y = f32::from(messages.origin.y);
        let s_y = f32::from(status.origin.y);
        let i_y = f32::from(input.origin.y);
        let m_h = f32::from(messages.size.height);
        let s_h = f32::from(status.size.height);
        let i_h = f32::from(input.size.height);
        let m_w = f32::from(messages.size.width);
        let s_w = f32::from(status.size.width);
        let i_w = f32::from(input.size.width);

        // 三区在场且有高度
        assert!(m_h > 0. && s_h > 0. && i_h > 0., "三区都应有高度");
        // 垂直堆叠：消息区在最上、状态行其次、输入区在底（1px 容差）
        assert!(m_y <= s_y + 1., "消息区应在状态行上方 (m={m_y}, s={s_y})");
        assert!(s_y + s_h <= i_y + 1., "状态行应在输入区上方 (s={s_y}+{s_h}, i={i_y})");
        // 消息区占大头（flex-1）
        assert!(m_h > s_h && m_h > i_h, "消息区应占最大高度 (m={m_h})");
        // 宽度一致（同窗等宽）
        assert!(
            (m_w - s_w).abs() < 1. && (m_w - i_w).abs() < 1.,
            "三区宽度应一致 (m={m_w}, s={s_w}, i={i_w})"
        );
        // 输入区高度 = 内建 Textarea auto_grow(1,6)：空输入 1 行、封顶 6 行
        //（用渲染同源行高复核区间）。
        let line_h = cx
            .update_window(handle, |_, window, _| grid_metrics(window))
            .expect("grid metrics")
            .1;
        assert!(
            i_h >= f32::from(line_h) - 1. && i_h <= f32::from(line_h) * 6. + 2.,
            "输入区应为 1..=6 行高 (i={i_h}, line_h={line_h})"
        );
    }

    /// 网格换算纯函数：像素区域 ÷ 单元格 → (cols, rows)，向下取整。
    ///
    /// 功能: 锁定 spec「等宽字符网格渲染模型」的换算口径；
    /// 输入: 构造的区域/单元格尺寸；
    /// 输出: 断言通过（整除、非整除取整、退化尺寸为 0）。
    #[test]
    fn grid_size_floors_to_whole_cells() {
        // 整除：800×600 ÷ 8×16 = (100, 37.5→37)
        assert_eq!(
            grid_size(Size::new(px(800.), px(600.)), Size::new(px(8.), px(16.))),
            (100, 37)
        );
        // 非整除：810÷8 = 101.25→101
        assert_eq!(
            grid_size(Size::new(px(810.), px(160.)), Size::new(px(8.), px(16.))),
            (101, 10)
        );
        // 退化：区域小于单元格 → 0
        assert_eq!(
            grid_size(Size::new(px(4.), px(8.)), Size::new(px(8.), px(16.))),
            (0, 0)
        );
    }

    /// fixture 路径（同 `tests/session_replay.rs` 口径）。
    fn fixture(scenario: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/probe")
            .join(scenario)
            .join("snapshot/transcript.jsonl")
    }

    /// 归一化条目列表：清掉 `started_at`（归约时的墙钟打点，非 fixture 数据，
    /// 两次运行必然不同——对拍时剔除）。
    ///
    /// 功能: 深拷贝并置空所有 ToolExecution.started_at；
    /// 输入: entries — 任一侧的 transcript 条目切片；
    /// 输出: 可 `==` 对比的归一化列表。
    fn strip_started_at(entries: &[Entry]) -> Vec<Entry> {
        entries
            .iter()
            .map(|e| match e {
                Entry::Assistant { turn, blocks, usage } => Entry::Assistant {
                    turn: *turn,
                    blocks: blocks
                        .iter()
                        .map(|b| match b {
                            Block::ToolCall { id, name, args_json, tool } => Block::ToolCall {
                                id: id.clone(),
                                name: name.clone(),
                                args_json: args_json.clone(),
                                tool: ToolExecution {
                                    started_at: None,
                                    ..tool.clone()
                                },
                            },
                            other => other.clone(),
                        })
                        .collect(),
                    usage: usage.clone(),
                },
                other => other.clone(),
            })
            .collect()
    }

    /// 组件内嵌归约（spec: 核心薄层 RPC 路由的「不做状态归约」半边）。
    ///
    /// 功能: 把 fixture 的入站事件逐条走组件归约路径（`TerminalState::reduce`），
    ///       与 session 层参考实现（`replay_file`）对拍——验证组件侧状态与
    ///       session 层一致（离线、无 pi）；
    /// 输入: 三个已提交快照场景（text_only/tool_call/failed_tool）；
    /// 输出: 断言通过（transcript 条目、相位、用量三项逐一对齐）。
    #[test]
    fn component_reduction_matches_session_layer_on_fixtures() {
        // 离线 commander（fake pi 应答）：仅 compaction_end 补发边沿会用到。
        let stub = write_stub("fixture-reduce");
        let client = crate::rpc::RpcClient::spawn(&stub, &[]).expect("spawn fake pi");
        let commander = client.command_sender();
        let (ui_tx, _ui_rx) = std::sync::mpsc::channel::<UiEvent>();
        for scenario in ["text_only", "tool_call", "failed_tool"] {
            let path = fixture(scenario);

            // 参考实现：session 层直接回放。
            let expected = replay_file(&path).expect("reference replay");

            // 组件路径：入站 raw → ClientEvent::Event → TerminalState::reduce。
            let file = fs::File::open(&path).expect("open fixture");
            let mut component = TerminalState::new();
            for line in BufReader::new(file).lines() {
                let line = line.expect("read line");
                if line.trim().is_empty() {
                    continue;
                }
                let Ok(wrapper) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                let is_inbound = wrapper
                    .get("dir")
                    .and_then(|d| d.as_str())
                    .is_some_and(|d| d.eq_ignore_ascii_case("in"));
                if !is_inbound {
                    continue;
                }
                if let Some(raw) = wrapper.get("raw") {
                    component.reduce(&ClientEvent::Event(raw.clone()), &commander, &ui_tx);
                }
            }

            assert_eq!(
                strip_started_at(component.session().entries()),
                strip_started_at(expected.entries()),
                "{scenario}: transcript 条目不一致"
            );
            assert_eq!(component.session().phase, expected.phase, "{scenario}: 相位不一致");
            assert_eq!(component.session().usage, expected.usage, "{scenario}: 用量不一致");
            // 归约后处于空闲态（fixture 都以 agent_settled 结尾）。
            assert_eq!(component.session().phase, Phase::Idle, "{scenario}: 应已 settled");
            assert!(
                component
                    .session()
                    .entries()
                    .iter()
                    .any(|e| matches!(e, Entry::Assistant { .. })),
                "{scenario}: 应有 assistant 条目"
            );
            assert!(!component.closed, "{scenario}: fixture 回放不应出现关闭");
        }
    }

    /// 关闭事件语义（spec: 事件扇出/关闭感知——Closed → 通知 + 关闭标志）。
    ///
    /// 功能: 锁定 `ClientEvent::Closed` 的组件侧行为；
    /// 输入: 构造的 Closed 事件；
    /// 输出: 断言通过（closed 标志置位、通知文案与 ratatui 同源）。
    #[test]
    fn closed_event_sets_flag_and_notice() {
        let stub = write_stub("closed");
        let client = crate::rpc::RpcClient::spawn(&stub, &[]).expect("spawn fake pi");
        let commander = client.command_sender();
        let (ui_tx, _ui_rx) = std::sync::mpsc::channel::<UiEvent>();
        let mut state = TerminalState::new();
        assert!(!state.closed);
        state.reduce(&ClientEvent::Closed, &commander, &ui_tx);
        assert!(state.closed, "Closed 应置关闭标志");
        assert_eq!(
            state.app.notice.as_ref().map(|(t, _)| t.as_str()),
            Some("pi 连接已关闭"),
            "通知文案与 ratatui 同源"
        );
        assert_eq!(
            state.app.notice.as_ref().map(|(_, c)| *c),
            Some(Color::Red),
            "关闭通知应为红色"
        );
    }

    /// 打开一个已聚焦输入窗的无头窗口（4.3 输入通路测试共用）。
    ///
    /// 功能: spawn 组件实体 → 开窗 → 聚焦输入窗 → 渲染一帧落地焦点/布局；
    /// 输入: cx — 测试上下文；name — fake pi 目录区分名；
    /// 输出: (窗口句柄, 组件实体)（fake pi 随测试结束回收）。
    fn open_focused(
        cx: &mut TestAppContext,
        name: &str,
    ) -> (gpui_kit::AnyWindowHandle, gpui_kit::Entity<GPUIAgentTerminal>) {
        let path = write_stub(name);
        let session = RpcSession::spawn(&path, &[], None).expect("spawn 应成功");
        open_with_session(cx, session)
    }

    /// 用给定会话开窗并聚焦输入窗（task 6.2 需要自建 recording 会话）。
    ///
    /// 功能: 开窗内嵌组件 → 聚焦输入窗 → 渲染一帧落地焦点/布局；
    /// 输入: cx — 测试上下文；session — 已探活的会话句柄；
    /// 输出: (窗口句柄, 组件实体)。
    fn open_with_session(
        cx: &mut TestAppContext,
        session: RpcSession,
    ) -> (gpui_kit::AnyWindowHandle, gpui_kit::Entity<GPUIAgentTerminal>) {
        cx.update(gpui_kit::init);
        let (handle, entity) = cx
            .update(|cx| {
                gpui_kit::open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds {
                            origin: Point::default(),
                            size: size(px(800.), px(600.)),
                        })),
                        ..Default::default()
                    },
                    cx,
                    |window, cx| {
                        let terminal =
                            cx.new(|cx| GPUIAgentTerminal::new(session, window, cx));
                        terminal.read(cx).input_focus().focus(window, cx);
                        terminal
                    },
                )
                .expect("open test window")
            });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render frame");
        cx.run_until_parked();
        (handle, entity)
    }

    /// 把 fixture 入站事件全量灌进组件归约（与离线回放同一口径）。
    ///
    /// 功能: 逐行解析 JSONL，`dir=="in"` 的 raw 走 `TerminalState::reduce`；
    /// 输入: term — 组件实体（直接可变借用）；name — fixture 场景名；
    /// 输出: 无返回值；副作用为组件状态推进。
    fn feed_fixture(term: &mut GPUIAgentTerminal, name: &str) {
        let path = fixture(name);
        let file = fs::File::open(&path).expect("open fixture");
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            if v.get("dir").and_then(|d| d.as_str()) != Some("in") {
                continue;
            }
            if let Some(raw) = v.get("raw") {
                // 响应类不进归约：get_state 应答按真实形态经探活应用
                //（apply_get_state，同组件 new() 对 probe 的处理）。
                if raw.get("type").and_then(|t| t.as_str()) == Some("response") {
                    if let Some(data) = raw.get("data") {
                        term.state.app.state.apply_get_state(data);
                    }
                    continue;
                }
                term.state.reduce(
                    &ClientEvent::Event(raw.clone()),
                    &term.commander,
                    &term.ui_tx,
                );
            }
        }
    }

    /// 注入 n 行用户消息（滚动测试的内容底料）。
    ///
    /// 功能: 构造含 n 行文本的 user `message_start` 并归约；
    /// 输入: term — 组件实体；n — 行数；
    /// 输出: 无返回值；副作用为状态推进（内容总行数 = n）。
    fn feed_user_lines(term: &mut GPUIAgentTerminal, n: usize) {
        let body = (0..n)
            .map(|i| format!("L{i:03}"))
            .collect::<Vec<_>>()
            .join("\n");
        let raw = serde_json::json!({
            "type": "message_start",
            "message": {"role": "user", "content": [{"type": "text", "text": body}]}
        });
        term.state
            .reduce(&ClientEvent::Event(raw), &term.commander, &term.ui_tx);
    }

    /// 记录命令的 fake pi（task 6.2：入站行逐条落盘 + 通用 success 应答；
    /// `clear_queue` 专答回填数据）。
    ///
    /// 功能: 落盘可执行脚本，每条入站行追加到 commands.log；
    /// 输入: name — 目录区分名；
    /// 输出: (脚本路径, 日志路径)（unix 上 chmod 755）。
    fn write_recording_stub(name: &str) -> (String, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("gpui-rec-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let log = dir.join("commands.log");
        let body = r#"#!/bin/sh
LOG='@@LOG@@'
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  id=$(printf '%s' "$line" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
  case "$line" in
    *'"type":"clear_queue"'*)
      printf '{"type":"response","id":"%s","success":true,"data":{"steering":["s1"],"followUp":["queued msg"]}}\n' "$id" ;;
    *)
      printf '{"type":"response","id":"%s","success":true}\n' "$id" ;;
  esac
done
"#
        .replace("@@LOG@@", &log.display().to_string());
        let path = dir.join("fake-pi.sh");
        fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        (path.to_string_lossy().into_owned(), log)
    }

    /// 轮询命令日志直到出现子串（假 pi 异步落盘；每次推进前台任务泵）。
    ///
    /// 功能: 每 20ms `run_until_parked` + 读日志；
    /// 输入: cx — 测试上下文；log — 日志路径；needle — 待出现子串；
    ///       ms — 超时毫秒；
    /// 输出: true = 命中；false = 超时。
    fn wait_log(cx: &mut TestAppContext, log: &std::path::Path, needle: &str, ms: u64) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        loop {
            cx.run_until_parked();
            if fs::read_to_string(log).is_ok_and(|t| t.contains(needle)) {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// 推进前台任务泵并读输入框全文（回报回填落地判定用）。
    ///
    /// 功能: `run_until_parked` 后读组件编辑缓冲；
    /// 输入: cx — 测试上下文；term — 组件实体；
    /// 输出: 当前输入框全文。
    fn editor_text(
        cx: &mut TestAppContext,
        term: &gpui_kit::Entity<GPUIAgentTerminal>,
    ) -> String {
        cx.run_until_parked();
        cx.update(|cx| {
            term.update(cx, |term, cx| term.pump_ui_events(cx));
            // 断言内建编辑器（用户可见）文本，而非 App 镜像（task 6.2 措辞）。
            term.read(cx).textarea.read(cx).value().to_string()
        })
    }

    /// 斜杠补全通路（task 4.3 重写：候选复用 autocomplete、应用回写内建编辑器）。
    ///
    /// 功能: 经内建 Textarea 输入 `/mo` → Change 镜像进 App → 补全菜单在场
    ///       （状态 + 面板文本）；Tab（全局键）应用选中项 → 回写 Textarea 值；
    /// 输入: 无（fake pi 提供会话）；
    /// 输出: 断言通过（菜单开合与编辑器值逐步推进）。
    #[gpui_kit::test]
    fn keystrokes_open_slash_completion(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "completion");

        // 输入 /mo（编辑键放行给 Textarea；Change 订阅镜像 + 重算补全）。
        cx.update_window(handle, |_, window, cx| window.input("/mo", cx))
            .expect("input slash query");
        let (open, has_model) = cx.update(|cx| {
            let app = &term.read(cx).state.app;
            (
                app.completion.open,
                app.completion.items.iter().any(|i| i.value == "model"),
            )
        });
        assert!(open, "输入 /mo 应打开补全菜单");
        assert!(has_model, "候选应 fuzzy 命中 model");

        // 菜单面板文本在场（无头快照）。
        let texts = cx
            .update_window(handle, |_, window, _| {
                gpui_kit::base::test_support::texts(window)
            })
            .expect("texts");
        let menu: String = texts
            .iter()
            .filter(|t| t.path().contains(&ElementId::Name("gpui-completion".into())))
            .map(|t| t.text().to_string())
            .collect::<Vec<_>>()
            .join("");
        assert!(menu.contains("model"), "菜单应渲染候选 model: {menu:?}");

        // Tab（全局键）→ 共享层应用选中项 → 回写内建编辑器。
        cx.update_window(handle, |_, window, cx| window.press("tab", cx))
            .expect("tab apply");
        let (value, closed) = cx.update(|cx| {
            let term = term.read(cx);
            (
                term.textarea.read(cx).value().to_string(),
                !term.state.app.completion.open,
            )
        });
        assert_eq!(value, "/model ", "Tab 应用补全后为 /model + 空格");
        assert!(closed, "应用后菜单应关闭");
    }

    /// 内建编辑器基本能力（task 5.4：插入/换行/删除/撤销 + auto_grow 高度上限）。
    ///
    /// 功能: 输入、shift+enter 换行、退格删除、ctrl-z 撤销均在内建 Textarea
    ///       内完成；多行使输入区增高、超过 6 行夹紧且消息区仍可见；
    /// 输入: 无（fake pi 提供会话）；
    /// 输出: 断言通过（值/高度/焦点逐步推进）。
    #[gpui_kit::test]
    fn builtin_textarea_edits_and_height_cap(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "textarea");

        // 焦点在内建编辑器。
        let focused = cx
            .update_window(handle, |_, window, cx| window.focused(cx).is_some())
            .expect("query focus");
        assert!(focused, "开窗后焦点应在内建编辑器");

        // 插入 → 换行（enter 是全局提交键，shift+enter 才是换行）→ 删除。
        cx.update_window(handle, |_, window, cx| window.input("hello", cx))
            .expect("input first line");
        // 单行高度做标尺（Textarea 自身行高与网格行高不同源）。
        let h1 = f32::from(
            cx.update_window(handle, |_, window, _| window.find("gpui-input").bounds())
                .expect("find input")
                .size
                .height,
        );
        cx.update_window(handle, |_, window, cx| {
            window.press("shift-enter", cx);
            window.input("world", cx);
            window.press("backspace", cx);
        })
        .expect("edit sequence");
        let value = cx.update(|cx| term.read(cx).textarea.read(cx).value().to_string());
        assert_eq!(value, "hello\nworl", "插入/换行/删除应在内建编辑器内完成");

        // 单步撤销：程序化 set_value 不入 undo，随后一次插入可撤销回原值。
        cx.update_window(handle, |_, window, cx| {
            term.update(cx, |term, cx| {
                term.textarea
                    .update(cx, |ta, cx| ta.set_value("base", window, cx));
            });
        })
        .expect("set value");
        cx.update_window(handle, |_, window, cx| window.input("X", cx))
            .expect("insert once");
        let value = cx.update(|cx| term.read(cx).textarea.read(cx).value().to_string());
        assert_eq!(value, "Xbase", "光标位插入单字");
        // 撤销（mac 绑定 cmd-z、其余平台 ctrl-z——内建键位表按平台分叉）。
        #[cfg(target_os = "macos")]
        let undo_key = "cmd-z";
        #[cfg(not(target_os = "macos"))]
        let undo_key = "ctrl-z";
        cx.update_window(handle, |_, window, cx| window.press(undo_key, cx))
            .expect("undo");
        let value = cx.update(|cx| term.read(cx).textarea.read(cx).value().to_string());
        assert_eq!(value, "base", "{undo_key} 应撤销单步插入");

        // 高度上限：20 行内容 → 输入区 ≤ 6 行高、消息区仍可见。
        let twenty = (0..20)
            .map(|i| format!("row{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        cx.update_window(handle, |_, window, cx| {
            term.update(cx, |term, cx| {
                term.textarea
                    .update(cx, |ta, cx| ta.set_value(twenty.clone(), window, cx));
            });
        })
        .expect("set multiline");
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render cap");
        let (input_b, msg_b) = cx
            .update_window(handle, |_, window, _| {
                (
                    window.find("gpui-input").bounds(),
                    window.find("gpui-messages").bounds(),
                )
            })
            .expect("find panes");
        let h20 = f32::from(input_b.size.height);
        assert!(
            h20 > h1 && h20 <= h1 * 6. + 4.,
            "20 行输入应增长但夹在单行高度的 6 倍内（h1={h1}, h20={h20}）"
        );
        assert!(
            f32::from(msg_b.size.height) > 0.,
            "消息区仍应可见（不被挤压致不可见）"
        );
    }

    /// 虚拟滚动与贴底跟随（task 5.2：内建 MessageScroller 承载回看）。
    ///
    /// 功能: 多条目高内容打开即贴底跟随；滚动到首行离开跟随；新事件到达
    ///       不打断回看锚点（行数对账走尾部 append，不重置滚动位置）；
    ///       滚轮命中链属 kit 组件内部，wheel 由 PTY 人工验收（task 7）；
    /// 输入: 构造的事件序列 + 状态级滚动；
    /// 输出: 断言通过（跟随态与滚动锚点逐步推进）。
    #[gpui_kit::test]
    fn wheel_scrolls_message_viewport(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "wheel");
        // 10 条 × 60 行：内容远超视口，钉住才有意义。
        cx.update(|cx| {
            term.update(cx, |term, _| {
                for _ in 0..10 {
                    feed_user_lines(term, 60);
                }
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("prime render");
        cx.run_until_parked();

        // 初始：贴底跟随（ListState 默认 FollowMode::Tail）。
        let following =
            cx.update(|cx| term.read(cx).scroller.read(cx).is_following_tail());
        assert!(following, "初始应贴底跟随");

        // 滚动到首行 → 离开跟随（等价“用户上滚回看”）。
        cx.update(|cx| {
            term.update(cx, |term, cx| {
                let _ = term.scroller.update(cx, |sc, cx| sc.scroll_to_item(0, cx));
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render after scroll");
        cx.run_until_parked();
        let (following, anchor) = cx.update(|cx| {
            let sc = term.read(cx).scroller.read(cx);
            (sc.is_following_tail(), sc.logical_scroll_top())
        });
        assert!(!following, "上滚后应离开跟随");
        assert_eq!(anchor.item_ix, 0, "应回到首行");

        // 新事件（追加第 11 条消息）→ 回看锚点保持。
        cx.update(|cx| {
            term.update(cx, |term, _| feed_user_lines(term, 5));
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render after event");
        cx.run_until_parked();
        let (following, anchor2) = cx.update(|cx| {
            let sc = term.read(cx).scroller.read(cx);
            (sc.is_following_tail(), sc.logical_scroll_top())
        });
        assert!(!following, "新事件不应恢复跟随");
        assert_eq!(anchor2.item_ix, anchor.item_ix, "新事件不应打断回看锚点");
    }

    /// 斜杠查询子集的可见反馈（task 6.3 人工验收复现：/changelog 与 /model 无参）。
    ///
    /// 功能: 经真实按键路径提交 `/changelog` → 输入清空 + 出现反馈（会话区
    ///       What's New 或状态条提示）；`/model` 无参 → 用法提示（不静默）；
    /// 输入: 无（fake pi 提供会话）；
    /// 输出: 断言通过。
    #[gpui_kit::test]
    fn slash_query_commands_show_visible_feedback(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "slash-query");

        // /changelog：提交后输入清空，且出现可见反馈。
        cx.update_window(handle, |_, window, cx| {
            window.input("/changelog", cx);
            window.press("enter", cx);
        })
        .expect("submit changelog");
        assert_eq!(editor_text(cx, &term), "", "changelog 提交后输入应清空");
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();
        let texts = cx
            .update_window(handle, |_, window, _| {
                gpui_kit::base::test_support::texts(window)
            })
            .expect("texts");
        let all: String = texts
            .iter()
            .map(|t| t.text().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            all.to_lowercase().contains("changelog"),
            "changelog 应有可见反馈（会话区或状态条），实际: {all:?}"
        );

        // /model 无参：真开选择器浮层（task 7.2，与 TUI 同语义）。
        cx.update_window(handle, |_, window, cx| {
            window.input("/model", cx);
            window.press("enter", cx);
        })
        .expect("submit model");
        assert_eq!(editor_text(cx, &term), "", "model 提交后输入应清空");
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();
        let is_model = cx.update(|cx| {
            matches!(
                term.read(cx).state.app.overlay,
                Some(crate::app::overlay::Overlay::ModelSelect { .. })
            )
        });
        assert!(is_model, "/model 无参应开选择器浮层");
        let texts = cx
            .update_window(handle, |_, window, _| {
                gpui_kit::base::test_support::texts(window)
            })
            .expect("texts");
        let all: String = texts
            .iter()
            .map(|t| t.text().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains("Model (ctrl+l)"), "选择器标题应可见: {all:?}");
        // 关浮层，继续后续带参断言（浮层开着时按键全被接管）；先等异步
        // 回填（loading 中 overlay_key 吞键，TUI 同款）。
        wait_overlay_loaded(cx, &term);
        cx.update_window(handle, |_, window, cx| window.press("escape", cx))
            .expect("esc close model");
        cx.run_until_parked();
        let open = cx.update(|cx| term.read(cx).state.app.overlay.is_some());
        assert!(!open, "esc 应关选择器");

        // /model 带参（异步反馈路径）：RPC → Info 经回报泵落地 → 状态条提示。
        cx.update_window(handle, |_, window, cx| {
            window.input("/model no-such-model", cx);
            window.press("enter", cx);
        })
        .expect("submit model with args");
        assert_eq!(editor_text(cx, &term), "", "带参 model 提交后输入应清空");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            cx.update(|cx| term.update(cx, |term, cx| term.pump_ui_events(cx)));
            cx.update_window(handle, |_, window, cx| window.render_frame(cx))
                .expect("render");
            cx.run_until_parked();
            let texts = cx
                .update_window(handle, |_, window, _| {
                    gpui_kit::base::test_support::texts(window)
                })
                .expect("texts");
            let all: String = texts
                .iter()
                .map(|t| t.text().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            if all.contains("未找到匹配模型") || all.contains("Model:") {
                break; // 异步 Info/OpenModelSelect 反馈已落地
            }
            assert!(
                std::time::Instant::now() < deadline,
                "带参 model 应有异步反馈（Info/提示），实际: {all:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// 攒批核心（task 5.5）：首条之后取空通道、保序、可重复。
    ///
    /// 功能: 构造无界通道灌 5 条 → drain_pending 一次取空；
    /// 输入: 无；输出: 断言通过（批内容 = 首条 + 按序后续，通道取空）。
    #[test]
    fn pump_batching_drains_all_pending() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<u32>();
        for i in 1..5 {
            tx.send(i).unwrap();
        }
        let mut batch = vec![0]; // 首条（recv 已取）
        drain_pending(&mut rx, &mut batch);
        assert_eq!(batch, vec![0, 1, 2, 3, 4], "批应含首条与全部后续且保序");
        assert!(rx.try_recv().is_err(), "通道应被取空");
    }

    /// 消息区渲染面（task 5.2：fixture 会话 → 无头文本快照断言）。
    ///
    /// 功能: 把 text_only / tool_call fixture 全量灌进组件归约，渲染一帧后
    ///       经文本快照断言消息区出现用户/助手文本与工具卡头（含未知工具
    ///       通用卡 `● agent_browser`），并校验行文本按行高对齐；
    /// 输入: 两个已提交快照场景；
    /// 输出: 断言通过（消息区非空、关键内容在场、行距 = 行高）。
    #[gpui_kit::test]
    fn fixture_renders_message_grid_snapshot(cx: &mut TestAppContext) {
        for (name, needles) in [
            (
                "text_only",
                vec!["You:", "Reply with exactly the word: pong", "pong"],
            ),
            (
                "tool_call",
                vec!["You:", "Use the read tool", "● read", "[package]"],
            ),
            (
                // 错误/重试提示面：失败工具卡（· error）+ markdown 错误说明
                //（含代码块，顺带覆盖高亮路径）。
                "failed_tool",
                vec!["You:", "· error", "failed with an ENOENT"],
            ),
        ] {
            let (handle, term) = open_focused(cx, name);
            // 灌入 fixture 入站事件（与离线回放同一口径）。
            cx.update(|cx| {
                term.update(cx, |term, _| feed_fixture(term, name));
            });
            cx.update_window(handle, |_, window, cx| window.render_frame(cx))
                .expect("render");
            cx.run_until_parked();

            // 消息区行文本（路径含 gpui-messages 的文本快照）。
            let texts = cx
                .update_window(handle, |_, window, _| {
                    gpui_kit::base::test_support::texts(window)
                })
                .expect("texts");
            let rows: Vec<_> = texts
                .iter()
                .filter(|t| t.path().contains(&ElementId::Name("gpui-messages".into())))
                .collect();
            let msg: String = rows
                .iter()
                .map(|t| t.text().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            assert!(!msg.is_empty(), "{name}: 消息区不应为空");
            for needle in needles {
                assert!(
                    msg.contains(needle),
                    "{name}: 消息区应含 {needle:?}，实际: {msg:?}"
                );
            }

        }
    }

    /// 未知工具通用卡兜底（task 5.2：不 panic、卡头在场）。
    ///
    /// 功能: 向组件归约注入 read/bash/edit 之外的工具调用（agent_browser），
    ///       渲染一帧后断言通用卡头 `● agent_browser` 出现在消息区；
    /// 输入: 构造的 toolcall/tool_execution 事件序列；
    /// 输出: 断言通过（通用卡渲染、不 panic）。
    #[gpui_kit::test]
    fn unknown_tool_renders_generic_card(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "generic-card");
        cx.update(|cx| {
            term.update(cx, |term, _| {
                use serde_json::json;
                let steps = [
                    json!({"type":"message_start","message":{"role":"assistant","content":[]}}),
                    json!({"type":"message_update","assistantMessageEvent":{"type":"toolcall_start","contentIndex":0,"id":"c1","toolName":"agent_browser"}}),
                    json!({"type":"message_update","assistantMessageEvent":{"type":"toolcall_delta","contentIndex":0,"delta":"{\"url\":\"https://example.com\"}"}}),
                    json!({"type":"tool_execution_start","toolCallId":"c1","toolName":"agent_browser","args":{"url":"https://example.com"}}),
                ];
                for raw in steps {
                    term.state.reduce(&ClientEvent::Event(raw), &term.commander, &term.ui_tx);
                }
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();

        let texts = cx
            .update_window(handle, |_, window, _| {
                gpui_kit::base::test_support::texts(window)
            })
            .expect("texts");
        let msg: String = texts
            .iter()
            .filter(|t| t.path().contains(&ElementId::Name("gpui-messages".into())))
            .map(|t| t.text().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            msg.contains("● agent_browser"),
            "未知工具应走通用卡，实际: {msg:?}"
        );
    }

    /// 流式增量绘制（task 5.2）：部分 delta 到达即渲染当前已归约文本，
    /// 无需等待 message_end。
    ///
    /// 功能: 注入 user + 流式中的 assistant（text_start/text_delta），
    ///       渲染一帧后断言消息区含已到的部分文本；
    /// 输入: 构造的增量事件序列；输出: 断言通过。
    #[gpui_kit::test]
    fn partial_streaming_renders_increments(cx: &mut TestAppContext) {
        use serde_json::json;
        let (handle, term) = open_focused(cx, "streaming");
        cx.update(|cx| {
            term.update(cx, |term, _| {
                let steps = [
                    json!({"type":"message_start","message":{"role":"user","content":[{"type":"text","text":"stream me"}]}}),
                    json!({"type":"message_start","message":{"role":"assistant","content":[]}}),
                    json!({"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":0}}),
                    json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"partial answ"}}),
                ];
                for raw in steps {
                    term.state
                        .reduce(&ClientEvent::Event(raw), &term.commander, &term.ui_tx);
                }
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();

        let texts = cx
            .update_window(handle, |_, window, _| {
                gpui_kit::base::test_support::texts(window)
            })
            .expect("texts");
        let msg: String = texts
            .iter()
            .filter(|t| t.path().contains(&ElementId::Name("gpui-messages".into())))
            .map(|t| t.text().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(msg.contains("stream me"), "用户行应在场: {msg:?}");
        assert!(msg.contains("partial answ"), "流式中部分文本应已绘制: {msg:?}");
    }

    /// 状态行渲染面（task 5.3：无头快照——phase/model/用量与 ratatui
    /// `render_header` 同源装配）。
    ///
    /// 功能: text_only fixture 灌入后渲染一帧，断言状态行文本快照含
    ///       phase 标签、model 前缀与用量串；
    /// 输入: fixture 场景；输出: 断言通过。
    #[gpui_kit::test]
    fn status_line_snapshot_shows_model_and_usage(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "status");
        cx.update(|cx| {
            term.update(cx, |term, _| feed_fixture(term, "text_only"));
        });
        // 瞬时提示（status_bar 右段）：注入 ParseError → 红色 notice。
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state.reduce(
                    &ClientEvent::ParseError { raw: "boom".into() },
                    &term.commander,
                    &term.ui_tx,
                );
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();

        let texts = cx
            .update_window(handle, |_, window, _| {
                gpui_kit::base::test_support::texts(window)
            })
            .expect("texts");
        let status: String = texts
            .iter()
            .filter(|t| t.path().contains(&ElementId::Name("gpui-status".into())))
            .map(|t| t.text().to_string())
            .collect::<Vec<_>>()
            .join("");
        assert!(status.contains("○ idle"), "phase 标签应在场: {status:?}");
        assert!(status.contains("thinking:low"), "thinking 应在场: {status:?}");
        assert!(status.contains("model:doubao"), "model 应在场: {status:?}");
        assert!(status.contains("↑↓/"), "用量应在场: {status:?}");
        assert!(
            status.contains("[parse_error] boom"),
            "瞬时提示（右段）应在场: {status:?}"
        );
    }

    /// prompt/steer/esc/ctrl+c 语义（task 6.2：假 pi 驱动，断言 JSON 命令
    /// 序列与输入框状态）。
    ///
    /// 功能: 端到端走组件按键通路——空闲 enter → prompt（无 steer）、
    ///       流式 enter → prompt+steer、流式 alt+enter → follow_up、
    ///       流式 esc → abort+clear_queue 且回填输入框、空闲 esc 无操作、
    ///       ctrl+c 清空且不退出；
    /// 输入: recording fake pi（命令落盘；clear_queue 回填数据）；
    /// 输出: 断言通过（命令序列与输入状态逐步对齐）。
    #[gpui_kit::test]
    fn prompt_esc_ctrlc_semantics_via_fake_pi(cx: &mut TestAppContext) {
        let (path, log) = write_recording_stub("cmdseq");
        let session = RpcSession::spawn(&path, &[], None).expect("spawn 应成功");
        let (handle, term) = open_with_session(cx, session);

        // 1) 空闲提交：prompt 不带 streamingBehavior，输入清空。
        cx.update_window(handle, |_, window, cx| {
            window.input("hello world", cx);
            window.press("enter", cx);
        })
        .expect("submit idle");
        assert!(
            wait_log(cx, &log, r#""type":"prompt""#, 3000),
            "空闲 prompt 应落盘: log={:?}",
            fs::read_to_string(&log).unwrap_or_default()
        );
        let logged = fs::read_to_string(&log).unwrap_or_default();
        let line = logged
            .lines()
            .find(|l| l.contains("hello world"))
            .unwrap_or("")
            .to_string();
        assert!(
            line.contains(r#""type":"prompt""#),
            "消息体应经 prompt 发出: {line}"
        );
        assert!(
            !line.contains("streamingBehavior"),
            "空闲 prompt 不应带 steer: {line}"
        );
        assert_eq!(editor_text(cx, &term), "", "提交后输入清空");

        // 2) 流式：enter → prompt 携带 streamingBehavior=steer。
        cx.update(|cx| {
            term.update(cx, |term, _| {
                let raw = serde_json::json!({"type": "agent_start"});
                term.state
                    .reduce(&ClientEvent::Event(raw), &term.commander, &term.ui_tx);
            });
        });
        cx.update_window(handle, |_, window, cx| {
            window.input("during stream", cx);
            window.press("enter", cx);
        })
        .expect("submit streaming");
        assert!(
            wait_log(cx, &log, "during stream", 3000),
            "流式 prompt 应落盘"
        );
        let logged = fs::read_to_string(&log).unwrap_or_default();
        let line = logged
            .lines()
            .find(|l| l.contains("during stream"))
            .unwrap_or("")
            .to_string();
        assert!(
            line.contains(r#""streamingBehavior":"steer""#),
            "流式提交必须带 steer: {line}"
        );
        assert_eq!(editor_text(cx, &term), "", "流式提交后输入清空");

        // 3) 流式 alt+enter → follow_up 排队；esc → abort + clear_queue 回填。
        cx.update_window(handle, |_, window, cx| {
            window.input("queued msg", cx);
            window.press("alt-enter", cx);
        })
        .expect("queue follow_up");
        assert!(
            wait_log(cx, &log, r#""type":"follow_up""#, 3000),
            "follow_up 应落盘"
        );
        assert_eq!(editor_text(cx, &term), "", "排队后输入清空");

        cx.update_window(handle, |_, window, cx| window.press("escape", cx))
            .expect("esc abort");
        assert!(
            wait_log(cx, &log, r#""type":"abort""#, 3000),
            "abort 应落盘"
        );
        assert!(
            wait_log(cx, &log, r#""type":"clear_queue""#, 3000),
            "clear_queue 应落盘"
        );
        // 回填经回报泵异步落地（RestoreQueued → restore_queued）。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let text = editor_text(cx, &term);
            if text.contains("queued msg") && text.contains("s1") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "排队消息应回填输入框，实际: {text:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        // 4) 空闲 esc 无操作：不发命令、不清输入（输入非空直接返回）。
        cx.update(|cx| {
            term.update(cx, |term, _| {
                let raw = serde_json::json!({"type": "agent_settled"});
                term.state
                    .reduce(&ClientEvent::Event(raw), &term.commander, &term.ui_tx);
            });
        });
        let input_before = editor_text(cx, &term);
        assert!(!input_before.is_empty(), "前置：回填后输入非空");
        let before_lines = fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .count();
        cx.update_window(handle, |_, window, cx| window.press("escape", cx))
            .expect("idle esc");
        std::thread::sleep(std::time::Duration::from_millis(200));
        cx.run_until_parked();
        let after_lines = fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .count();
        assert_eq!(after_lines, before_lines, "空闲 esc 不应发命令");
        assert_eq!(editor_text(cx, &term), input_before, "空闲 esc 不应清输入");

        // 5) ctrl+c 清空输入、不退出（单击）。
        cx.update_window(handle, |_, window, cx| window.press("ctrl-c", cx))
            .expect("ctrl-c");
        assert_eq!(editor_text(cx, &term), "", "ctrl+c 应清空输入");
        let should_quit = cx.update(|cx| term.read(cx).state().app.should_quit);
        assert!(!should_quit, "单击 ctrl+c 不应退出");
    }
}
