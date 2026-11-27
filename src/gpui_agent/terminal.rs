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
use std::time::{Duration, Instant};

use gpui_kit::base::{Collapsible, TestTextExt};
use gpui_kit::component::input::{InputEvent, Position, Textarea, TextareaState};
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::text::TextView;
use gpui_kit::*;
use gpui_kit::prelude::FluentBuilder as _; // 容器链上条件边框用 `.map`（spec: 输入区简约边框）
use ratatui::style::{Color, Modifier, Style};

use crate::app::theme::TuiTheme;
use crate::app::{
    App as AppState, GridLine, Seg, SegRole, UiEvent, bash_entry_model, notice_model,
    thinking_seqs, thinking_visible, tool_card_model,
};
use crate::gpui_agent::actor::RpcSession;
use crate::gpui_agent::scroll::{ScrollMetrics, jump_target, page_down_target, page_up_target};
use crate::gpui_agent::input::keystroke_to_keyevents;
use crate::rpc::{ClientEvent, CommandSender};
use crate::session::{Block, Entry, SessionState, ToolExecution, ToolStatus};
/// 主题调色板双表（spec: 独立窗口主题三模式；design D2/D6）。
///
/// `LIGHT`/`DARK` 两套31 字段常数：语义 token 由 pi `light.json`/
/// `dark.json` 编写期预计算（pi-tui 同算法，校验 5/5 命中既有 dark 现值），
/// 面板底类为自有值（浅色现值、深色手调；input 边框深色取 pi dark border）。
/// 消费点按 resolved mode 取表（render 入口解析一次，纯函数链穿 `pal`
/// 参数——task1.2/1.3 已接通，旧单表常数已删除）。
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

    /// 随 mode 解析的完整调色板（每字段均有 light/dark 两值）。
    ///（`gpui_kit::Rgba` 无 `Debug`，断言经 `rgb()` 元组转译）
    #[derive(Clone, Copy, PartialEq)]
    pub(super) struct Palette {
        // —— 自有面板面（pi 无 bg token：终端留白）——
        /// 根/消息区/补全菜单底。
        pub(super) chat_bg: Rgba,
        /// 状态行底。
        pub(super) status_bg: Rgba,
        /// 输入区边框（浅=纯黑 spec 钉定；深=pi dark border）。
        pub(super) input_border: Rgba,
        /// 补全选中行底。
        pub(super) comp_selected_bg: Rgba,
        /// 补全普通行文字。
        pub(super) comp_text: Rgba,
        /// 补全选中行文字。
        pub(super) comp_text_selected: Rgba,
        /// 补全计数行文字。
        pub(super) comp_count_text: Rgba,
        // —— pi 同源语义 token（light.json / dark.json 预计算）——
        /// "You:" 标签（← accent）。
        pub(super) user_label: Rgba,
        /// thinking 正文/占位（← thinkingText）。
        pub(super) thinking_text: Rgba,
        /// bash 输出摘要（← muted）。
        pub(super) bash_body: Rgba,
        /// bash 命令行（← warning 系）。
        pub(super) bash_cmd: Rgba,
        /// bash 运行中/取消/truncated 提示（← warning）。
        pub(super) warn: Rgba,
        /// 通知：错误（← error）。
        pub(super) notice_error: Rgba,
        /// 通知：重试/警告（← warning）。
        pub(super) notice_retry: Rgba,
        /// 通知：普通（← muted）。
        pub(super) notice_default: Rgba,
        /// bash `exit 0`（← success）。
        pub(super) exit_ok: Rgba,
        /// bash `exit N`（N≠0，← error）。
        pub(super) exit_bad: Rgba,
        /// 工具卡结果预览（← toolOutput）。
        pub(super) tool_preview_text: Rgba,
        /// 工具卡头行参数/正文（← toolTitle）。
        pub(super) card_text: Rgba,
        /// 工具卡计时/折叠提示（← dim）。
        pub(super) card_dim: Rgba,
        /// 工具卡状态：运行中（← warning）。
        pub(super) card_status_run: Rgba,
        /// 工具卡状态：成功（← success）。
        pub(super) card_status_ok: Rgba,
        /// 工具卡状态：失败（← error）。
        pub(super) card_status_err: Rgba,
        /// 工具卡 `Fg` 段 ANSI 红（diff 删除行，← toolDiffRemoved）。
        pub(super) card_fg_red: Rgba,
        /// 工具卡 `Fg` 段 ANSI 绿（diff 新增行，← toolDiffAdded）。
        pub(super) card_fg_green: Rgba,
        /// 工具卡 `Fg` 段 ANSI 黄（截断警告，← warning）。
        pub(super) card_fg_yellow: Rgba,
        /// 工具卡 `Fg` 段 ANSI 暗灰（← syntaxComment）。
        pub(super) card_fg_gray: Rgba,
        /// 工具卡面板底：运行中（← toolPendingBg；混血根源，随模式）。
        pub(super) card_bg_pending: Rgba,
        /// 工具卡面板底：成功（← toolSuccessBg）。
        pub(super) card_bg_ok: Rgba,
        /// 工具卡面板底：失败（← toolErrorBg）。
        pub(super) card_bg_err: Rgba,
        /// 工具卡 `Fg` 段 ANSI 青（pi 无青 token，自有：浅手调/深现值）。
        pub(super) card_fg_cyan: Rgba,
    }

    /// 浅色表（pi light.json 预计算 + 自有浅色现值）。
    pub(super) const LIGHT: Palette = Palette {
        chat_bg: hex(0xffffff),
        status_bg: hex(0xf0f0f2),
        input_border: hex(0x000000),
        comp_selected_bg: hex(0xd8e6fb),
        comp_text: hex(0x4a4a4a),
        comp_text_selected: hex(0x1f1f1f),
        comp_count_text: hex(0x707070),
        user_label: hex(0x7459b4),
        thinking_text: hex(0x7c868c),
        bash_body: hex(0x677176),
        bash_cmd: hex(0x8f6802),
        warn: hex(0x8f6802),
        notice_error: hex(0xc8253d),
        notice_retry: hex(0x8f6802),
        notice_default: hex(0x677176),
        exit_ok: hex(0x337e58),
        exit_bad: hex(0xc8253d),
        tool_preview_text: hex(0x677176),
        card_text: hex(0x3b3f41),
        card_dim: hex(0x879095),
        card_status_run: hex(0x8f6802),
        card_status_ok: hex(0x337e58),
        card_status_err: hex(0xc8253d),
        card_fg_red: hex(0xc8253d),
        card_fg_green: hex(0x337e58),
        card_fg_yellow: hex(0x8f6802),
        card_fg_gray: hex(0x677176),
        card_bg_pending: hex(0xe4e5e6),
        card_bg_ok: hex(0xdee9e1),
        card_bg_err: hex(0xeee2e1),
        card_fg_cyan: hex(0x0097a7),
    };

    /// 深色表（pi dark.json 预计算 + 自有深色手调）。
    pub(super) const DARK: Palette = Palette {
        chat_bg: hex(0x1e1e1e),
        status_bg: hex(0x26262a),
        input_border: hex(0x5fa8cc),
        comp_selected_bg: hex(0x213b49),
        comp_text: hex(0xdee0e1),
        comp_text_selected: hex(0xdee0e1),
        comp_count_text: hex(0x9da5a9),
        user_label: hex(0xa798d7),
        thinking_text: hex(0x96a0a4),
        bash_body: hex(0x9da5a9),
        bash_cmd: hex(0xcd9a22),
        warn: hex(0xcd9a22),
        notice_error: hex(0xea7f81),
        notice_retry: hex(0xcd9a22),
        notice_default: hex(0x9da5a9),
        exit_ok: hex(0x68b78d),
        exit_bad: hex(0xea7f81),
        tool_preview_text: hex(0x9da5a9),
        card_text: hex(0xdee0e1),
        card_dim: hex(0x7e888e),
        card_status_run: hex(0xcd9a22),
        card_status_ok: hex(0x68b78d),
        card_status_err: hex(0xea7f81),
        card_fg_red: hex(0xea7f81),
        card_fg_green: hex(0x68b78d),
        card_fg_yellow: hex(0xcd9a22),
        card_fg_gray: hex(0x9da5a9),
        card_bg_pending: hex(0x34383a),
        card_bg_ok: hex(0x254131),
        card_bg_err: hex(0x5b282a),
        card_fg_cyan: hex(0x6ec6c6),
    };

    /// 按 resolved 是否深色取表（task2 接 Theme 全局 mode；本任务先纯函数可测）。
    ///
    /// 输入: is_dark — resolved mode（system 已解析为 Light/Dark）
    /// 输出: 指向对应静态表的引用
    pub(super) const fn for_dark(is_dark: bool) -> &'static Palette {
        if is_dark { &DARK } else { &LIGHT }
    }

    /// 双表取值与源一致性（task1.1 验证；无 TTY 纯逻辑）。
    #[cfg(test)]
    mod tests {
        use super::*;

        /// Rgba → u8 三元组（`Rgba` 无 `Debug`，断言经此转译）。
        fn rgb(c: Rgba) -> (u8, u8, u8) {
            (
                (c.r * 255.0).round() as u8,
                (c.g * 255.0).round() as u8,
                (c.b * 255.0).round() as u8,
            )
        }

        /// 深色卡三态必须与 `TuiTheme::default`（pi dark 同源）逐值一致。
        #[test]
        fn dark_card_panels_match_tui_theme_defaults() {
            let t = crate::app::theme::TuiTheme::default();
            let rat = |c: ratatui::style::Color| match c {
                ratatui::style::Color::Rgb(r, g, b) => (r, g, b),
                other => panic!("应为 Rgb 色: {other:?}"),
            };
            assert_eq!(rgb(DARK.card_bg_pending), rat(t.tool_running_bg));
            assert_eq!(rgb(DARK.card_bg_err), rat(t.tool_error_bg));
            assert_eq!(rgb(DARK.card_bg_ok), rat(t.tool_success_bg));
        }

        /// 浅色卡三态为浅值（pi light 同源），与深色表不同。
        #[test]
        fn light_card_panels_are_light() {
            assert_eq!(rgb(LIGHT.card_bg_pending), (0xe4, 0xe5, 0xe6));
            assert_eq!(rgb(LIGHT.card_bg_ok), (0xde, 0xe9, 0xe1));
            assert_eq!(rgb(LIGHT.card_bg_err), (0xee, 0xe2, 0xe1));
            assert_ne!(rgb(LIGHT.card_bg_pending), rgb(DARK.card_bg_pending));
        }

        /// 面板面：浅=现值（白底/黑边 spec 钉定），深=手调底 + pi dark 边框。
        #[test]
        fn panel_surfaces_per_mode() {
            assert_eq!(rgb(LIGHT.chat_bg), (255, 255, 255));
            assert_eq!(rgb(LIGHT.input_border), (0, 0, 0));
            assert_eq!(rgb(DARK.chat_bg), (0x1e, 0x1e, 0x1e));
            assert_eq!(rgb(DARK.input_border), (0x5f, 0xa8, 0xcc));
        }

        /// 青色自有（pi 无青 token，syntaxType 实为紫）：两模式各自取值。
        #[test]
        fn cyan_self_owned_per_mode() {
            assert_eq!(rgb(LIGHT.card_fg_cyan), (0x00, 0x97, 0xa7));
            assert_eq!(rgb(DARK.card_fg_cyan), (0x6e, 0xc6, 0xc6));
        }

        /// `for_dark` 按 resolved 选表（`const` 提升使地址不稳，故值比较）。
        #[test]
        fn for_dark_selects_table() {
            assert!(*for_dark(true) == DARK, "is_dark 应选 DARK 表");
            assert!(*for_dark(false) == LIGHT, "浅色应选 LIGHT 表");
            assert_eq!(rgb(for_dark(true).chat_bg), rgb(DARK.chat_bg));
            assert_eq!(rgb(for_dark(false).chat_bg), rgb(LIGHT.chat_bg));
        }
    }
}


/// 输入区边框宽度规则（spec: 输入区简约边框 MODIFIED——常态 1px、聚焦加粗
/// 2px；两模式同规，纯逻辑无 TTY 可测）。
///
/// 输入: focused — 输入区当前焦点态；
/// 输出: 边框像素宽（1 或 2）
fn input_border_width(focused: bool) -> u16 {
    if focused { 2 } else { 1 }
}

/// 测试探针：Rgba → 0xRRGGBB 打包（仅 test 构建；task2.3 帧色断言）。
#[cfg(test)]
fn pack_rgb(c: gpui_kit::Rgba) -> u32 {
    (((c.r * 255.0).round() as u32) << 16)
        | (((c.g * 255.0).round() as u32) << 8)
        | (c.b * 255.0).round() as u32
}

#[cfg(test)]
thread_local! {
    /// 测试探针（task2.3/3.2）：最近一帧消息区底 / 输入边框 / 卡面板底
    /// （0xRRGGBB；按测试线程隔离——进程级 static 会被并行测试互踩）。
    static PROBE_CHAT_BG: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static PROBE_INPUT_BORDER: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static PROBE_CARD_BG: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// 读测试探针：最近一帧消息区底色（仅 test 构建；task2.3/3.2 断言）。
#[cfg(test)]
fn probe_chat_bg() -> u32 {
    PROBE_CHAT_BG.with(|c| c.get())
}

/// 读测试探针：最近一帧输入边框色（仅 test 构建）。
#[cfg(test)]
fn probe_input_border() -> u32 {
    PROBE_INPUT_BORDER.with(|c| c.get())
}

/// 读测试探针：最近渲染的卡面板底色（仅 test 构建）。
#[cfg(test)]
fn probe_card_bg() -> u32 {
    PROBE_CARD_BG.with(|c| c.get())
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
    /// 输入框焦点进出订阅（边框加粗重绘，spec: 输入区简约边框；RAII 反注册）。
    _focus_in_sub: Subscription,
    _focus_out_sub: Subscription,
    /// app 退出钩子订阅（RAII：实体销毁即反注册，防悬垂）。
    _quit: Subscription,
    /// 归约泵任务（持有至实体销毁；实体释放后泵自动退出）。
    /// 归约泵任务（仅生产态；测试态不装——见归约泵处注释）。
    #[cfg(not(test))]
    _reduce_task: Task<()>,
    /// 计时走字任务（task 4.2）：存在运行中工具期间每 1s notify 重绘
    /// （`Elapsed …` 走字）；渲染点按 `has_running_tool` 对账起停（Drop
    /// 即取消），weak 实体升级失败自行退出。
    timer_task: Option<Task<()>>,
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
        let mut app = AppState::new();
        // GPUI 表面标记：/settings 含 Theme 行（共享层只认标记不认类型）。
        app.set_surface(crate::app::SurfaceKind::Gpui);
        Self {
            app,
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
        // 内建虚拟滚动（task 5.2）：初始行数 = 打开时条目数（fresh 会话为 0，
        // 随后由渲染点对账 append/reset 追平）。
        let scroller = cx.new(|cx| {
            MessageScrollerState::new(state.session().entries().len(), cx)
        });

        // 内建多行编辑器（task 5.4）：auto_grow(1,6) = 随行数增长、内容行
        // 上限 6（与 ratatui editor_height 的内容行 clamp(1,6) 同源）。
        let textarea = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 6));
        let focus = textarea.focus_handle(cx);
        // 焦点进出 → 实体重绘（spec: 输入区简约边框）：render 时读
        // is_focused 切换 border_1/2；焦点变化不自动重绘实体，故显式 notify。
        let focus_in_sub = cx.on_focus_in(&focus, window, |_, _, cx| cx.notify());
        let focus_out_sub = cx.on_focus_out(&focus, window, |_, _, _, cx| cx.notify());
        // 文本变更 → 镜像进 App.editor + 重算补全（task 4.3：补全随输入刷新；
        // push 回写用 set_value，不发 Change，不会形成回环）。
        let change_sub = cx.subscribe(&textarea, |this, _ta, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                // 内容变更退出浏览（task4.2）：进 Change 前记浏览条目，同步后
                // 文本偏离该条目 → 用户改内容 → 退出；程序化回写（历史载入
                // 推的就是条目原文）文本相等 → 不退出，天然免 guard（design D4）。
                let browsed = this.state.app.editor.history_current().map(str::to_string);
                this.sync_editor_from_textarea(cx);
                if let Some(entry) = browsed
                    && this.state.app.editor.text() != entry
                {
                    this.state.app.editor.exit_history_browsing();
                }
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
                    // 输入历史分派（task4.1）：条件成立 → 共享（ToApp 分支
                    // 自带 sync+push 桥，与 TUI 同一 history_or_* 规则）；
                    // 否则保持原路由（菜单/浮层/修饰键优先级不变）。
                    let route = if term.history_route(&keystroke, cx) {
                        Route::ToApp
                    } else {
                        route
                    };
                    match route {
                        Route::ToEditor => return, // 编辑键交内建编辑器
                        Route::ToScroller => {
                            // 滚动键（task 3.1）：直达 scroller，不碰编辑器镜像
                            // （spec：滚动键 MUST NOT 改动输入编辑器内容）。
                            term.handle_scroller_key(&keystroke, window, cx);
                            cx.stop_propagation();
                            cx.notify();
                            return;
                        }
                        Route::ToApp => {}
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
            _focus_in_sub: focus_in_sub,
            _focus_out_sub: focus_out_sub,
            #[cfg(not(test))]
            _reduce_task: reduce_task,
            timer_task: None,
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
        let (row, col) = (self.state.app.editor.row(), self.state.app.editor.col());
        let position = Position {
            line: row as u32,
            character: col as u32,
        };
        let ta = self.textarea.read(cx);
        let value_same = ta.value().as_str() == text.as_str();
        let cursor = ta.cursor_position();
        let cursor_same = cursor.line == position.line && cursor.character == position.character;
        if value_same && cursor_same {
            return; // 无变化：不重放 set_value（避免无谓重建）
        }
        self.textarea.update(cx, |ta, cx| {
            // 文本同则只对齐光标（task4.1：home/历史载入等同文本改光标的
            // 语义键必须把光标推下去——早先整段早返回会吞掉它）。
            if !value_same {
                ta.set_value(text, window, cx);
            }
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

    /// Up/Down 是否走共享历史分派（task4.1；与 TUI `history_or_*` 同一
    /// 分区条件，design D2 逻辑行近似 pi 视觉行）。
    ///
    /// 功能: 修饰键/浮层/菜单不入（既有路由优先）；行位判定读【Textarea
    ///       实时光标/行数】——原生移动不回写镜像，镜像行位会滞后；进
    ///       ToApp 后 sync 先行，共享层再按镜像的空/浏览/列细分历史与
    ///       行首/行尾；
    /// 输入: ks — 键击；cx — gpui App（读 Textarea 实时状态）；
    /// 输出: true = 交共享层；false = 放行原生/维持原路由
    fn history_route(&self, ks: &Keystroke, cx: &App) -> bool {
        if ks.modifiers.control || ks.modifiers.alt || ks.modifiers.shift {
            return false; // 修饰键走既有路由（ctrl+↑滚动跳转、alt+up 取回…）
        }
        if self.state.app.overlay.is_some() || self.state.app.completion.open {
            return false; // 浮层/菜单优先（route 原样生效）
        }
        let ta = self.textarea.read(cx);
        let line = ta.cursor_position().line as usize;
        match ks.key.as_str() {
            // 首行（空/行中/行首都在共享层细分）；非首行 → 原生上移
            "up" => line == 0,
            // 末行（浏览→翻新、非浏览→行尾，与 TUI/pi 一致）；非末行 → 原生
            "down" => line + 1 >= ta.value().split('\n').count(),
            _ => false,
        }
    }

    /// 消息区滚动键处理（task 3.1/3.2/3.3：`Route::ToScroller` 执行端）。
    ///
    /// 功能: ctrl+home/end → 绝对顶 / 贴底恢复跟随；pageup/pagedown → S1 估算
    ///       整页（shift 为反向半页）；ctrl+↑/↓ → 消息边界跳转（镜像 TUI
    ///       `prompt_jump_target`：prev = 严格低于顶的最后消息起点、顶在消息
    ///       中部 → 该消息起点、无 → 归顶；next = 严格高于顶的第一个、无 →
    ///       贴底恢复跟随）；滚动真相 = `MessageScrollerState`（design D1）；
    /// 输入: ks — 键击（路由已保证为滚动键）；window — 视口观测 bounds 与
    ///       网格度量来源；cx — 实体上下文；
    /// 输出: 无返回值；副作用为 scroller 滚动 + notify 重绘；输入编辑器不动；
    ///       视口观测缺失或无内容时静默返回（不硬猜页长）。
    fn handle_scroller_key(
        &mut self,
        ks: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ctrl = ks.modifiers.control;
        let shift = ks.modifiers.shift;
        let key = ks.key.as_str();
        let is_scroll = matches!(key, "pageup" | "pagedown")
            || (ctrl && matches!(key, "up" | "down" | "home" | "end"));
        if !is_scroll {
            return;
        }
        let entries = self.state.session().entries().to_vec();
        if entries.is_empty() {
            return;
        }
        let Some(vp) = gpui_kit::base::test_support::find(
            window,
            &[],
            &ElementId::from("gpui-messages"),
        )
        .map(|s| s.bounds()) else {
            return; // 视口不可得：不硬猜页长/端点
        };
        let vp_w = f32::from(vp.size.width);
        let vp_h = f32::from(vp.size.height);
        let (_, grid_px) = grid_metrics(window);
        let grid = f32::from(grid_px);
        let metrics = ScrollMetrics::production(vp_w, grid);
        let page_rows = ((vp_h / grid).floor() - 4.0).max(1.0); // TUI 页 = 视口−4 行
        let page_px = page_rows * grid;
        let top = self.scroller.read(cx).logical_scroll_top();
        let from_ix = top.item_ix.min(entries.len().saturating_sub(1));
        let from_off = f32::from(top.offset_in_item);
        let app = &self.state.app;
        // 目标：Some(ix) = 对齐条目顶；None = 贴底恢复跟随（end 与 next 端点）。
        let target: Option<usize> = match key {
            "home" if ctrl => Some(0),
            "end" if ctrl => None,
            "pageup" if shift => {
                let half = (page_rows / 2.0).max(1.0) * grid;
                Some(page_up_target(&entries, app, from_ix, from_off, half, &metrics))
            }
            "pageup" => Some(page_up_target(&entries, app, from_ix, from_off, page_px, &metrics)),
            "pagedown" if shift => {
                let half = (page_rows / 2.0).max(1.0) * grid;
                Some(page_down_target(&entries, app, from_ix, from_off, half, &metrics))
            }
            "pagedown" => Some(page_down_target(&entries, app, from_ix, from_off, page_px, &metrics)),
            "up" if ctrl => jump_target(&entries, from_ix, from_off, -1),
            "down" if ctrl => jump_target(&entries, from_ix, from_off, 1),
            _ => return,
        };
        let scroller = self.scroller.clone();
        scroller.update(cx, |s, cx| match target {
            Some(ix) => {
                s.scroll_to_item(ix, cx);
            }
            None => s.scroll_to_end(cx),
        });
        cx.notify();
    }

    /// 计时走字任务对账（task 4.2）：有运行中工具且无任务 → 起 1s interval
    /// （每 tick notify 重绘，实体销毁即退）；无运行中工具且有任务 → Drop
    /// 取消——与 ratatui `timer_redraw_due` 同门控（仅 Running 走字）。
    ///
    /// 输入: cx — 实体上下文（spawn/Drop 通道）；
    /// 输出: 无返回值；副作用为起/停 `timer_task`（仅渲染点调用，
    ///       归约泵不直接碰任务，防 notify 自旋）。
    fn sync_timer_task(&mut self, cx: &mut Context<Self>) {
        let running = self.state.session().has_running_tool();
        if running && self.timer_task.is_none() {
            self.timer_task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_secs(1))
                        .await;
                    // weak 升级失败（实体已销毁）→ 退出；否则 notify 走字。
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            }));
        } else if !running && self.timer_task.is_some() {
            self.timer_task = None; // Drop 即取消后续 tick
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

    /// 滚动行数 = 已落条目 + 流式 in-flight 尾块 + 运行中 bash 尾槽
    /// （task 5.2 + 冒烟 B2：流式块与 `!cmd` 运行期都要有行，flush/落条目
    /// 后行数自然收敛）。
    ///
    /// 输入: 无（读会话状态）；
    /// 输出: 虚拟列表应有行数（bash 尾槽占一个易失项）。
    fn scroller_rows(&self) -> usize {
        let session = self.state.session();
        session.entries().len() + session.in_flight().len() + usize::from(session.bash_running)
    }

    /// 应用一条回报事件（回报泵的任务体入口，task 6.2）。
    ///
    /// 功能: Info/SendFailed → notice（绿/红，同 ratatui），AsyncData/
    ///       CommandsFetched → 缓存回填 + 补全重算，RestoreQueued → 回填
    ///       输入框，OpenModelSelect → 选择器不可用提示（P0 无浮层），
    ///       ThemeCycle → 循环三态并应用（task3.1）；
    ///       其余（外部编辑器等）P0 静默丢弃；
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
            UiEvent::ThemeCycle => {
                // /settings Theme 行（task3.1）：循环三态并应用；A3 不落盘，
                // 重绘由 Theme::change 的 refresh_windows 驱动。
                crate::gpui_agent::theme::cycle_and_apply(cx);
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

/// 键击路由结果（task 6.1：三区模型；task 3.1 增第三态）。
/// `To*` 同前缀为刻意的分层命名（进共享/进编辑器/进滚动器）。
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    /// 交共享语义层（`App::handle_key`）并停止传播。
    ToApp,
    /// 放行给内建编辑器（Textarea 自理）。
    ToEditor,
    /// 消息区滚动键直达内建虚拟滚动状态（design D1：滚动真相归 scroller，
    /// 不经共享 `App::scroll`，不碰输入编辑器）。
    ToScroller,
}

/// 键击路由判定（task 6.1 三区模型 + task 2.1/3.1 全键位表，纯函数可单测）。
///
/// 功能: 按 design D1 优先序分派——浮层 → 共享语义键 → 补全菜单键（不吞
///       ctrl 组合）→ 滚动键 → 编辑器；
/// 输入: overlay_open — 浮层打开（A 区全量接管）；completion_open — 补全菜单；
///       ks — gpui 键击；
/// 输出: [`Route::ToApp`] / [`Route::ToEditor`] / [`Route::ToScroller`]。
pub(crate) fn route_keystroke(overlay_open: bool, completion_open: bool, ks: &Keystroke) -> Route {
    if overlay_open {
        return Route::ToApp;
    }
    let ctrl = ks.modifiers.control;
    let alt = ks.modifiers.alt;
    let shift = ks.modifiers.shift;
    match ks.key.as_str() {
        "enter" if !shift => Route::ToApp, // shift+enter = 换行（编辑器自理）
        "escape" => Route::ToApp,          // 菜单关闭 / 流式中断 / 空闲无操作
        "c" if ctrl => Route::ToApp,       // ctrl+c 清空
        // 打开键（task 6.3）：镜像 `app::key_intent` —— ctrl+l → model、
        // alt+t → thinking（不门控菜单状态，与 TUI 意图分发同层）。
        "l" if ctrl && !shift => Route::ToApp,
        "t" if alt => Route::ToApp,
        // 全局折叠开关（task 4.1）：ctrl+t → thinking、ctrl+o → 工具卡。
        "t" if ctrl => Route::ToApp,
        "o" if ctrl => Route::ToApp,
        // —— 共享键臂（task 2.1，镜像 key_intent）——
        "x" if ctrl && !shift => Route::ToApp, // ctrl+x 复制最后回复
        "p" if ctrl => Route::ToApp,          // ctrl+p / ctrl+shift+p 模型循环
        "up" if alt => Route::ToApp,          // alt+up 取回排队
        "tab" if shift => Route::ToApp,       // shift+tab thinking 循环（gpui: tab+shift）
        // —— 补全菜单（B 区）：导航/翻页/应用，不吞 ctrl 组合（task 2.1 收紧，
        // 使 ctrl+↑/↓ 在菜单开着时仍进滚动层，对齐 TUI「菜单不消费 ctrl+↑」）——
        "up" | "down" | "pageup" | "pagedown" | "tab" if completion_open && !ctrl => Route::ToApp,
        "tab" => Route::ToApp, // 菜单关闭：强制路径补全
        // —— 消息区滚动（task 3.1 → ToScroller；菜单开着的 plain 页键已被上臂
        // 接管为菜单翻页，此处剩 ctrl+页键与菜单外全部页键）——
        "pageup" | "pagedown" => Route::ToScroller,
        "home" | "end" if ctrl => Route::ToScroller, // 绝对顶/底
        "up" | "down" if ctrl => Route::ToScroller,  // 消息边界跳转（±shift 同义）
        _ => Route::ToEditor,
    }
}

impl Render for GPUIAgentTerminal {
    /// 三分区占位渲染：消息区（弹性占满）/ 状态行（一行）/ 输入区（三行）。
    ///
    /// 输入: window — 网格度量来源（字体表/像素吸附）；cx — 实体上下文（未用）；
    /// 输出: 垂直 flex 元素树；三个分区各带 id 与观测锚点（无头测试 find 断言）。
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 调色板随全局 mode 解析一次（task1.2：内建控件随 kit Theme 切换，
        // 我们的 token 走同一 mode；system/默认在 task2 接入）。
        let pal =
            palette::for_dark(gpui_kit::component::theme::Theme::global(cx).mode.is_dark());
        // 测试探针（task2.3/3.2）：记录本帧解析出的底色/边框（仅 test 构建）。
        #[cfg(test)]
        {
            PROBE_CHAT_BG.with(|c| c.set(pack_rgb(pal.chat_bg)));
            PROBE_INPUT_BORDER.with(|c| c.set(pack_rgb(pal.input_border)));
        }
        //（桥接注意：渲染点不能盲目回写——Change 订阅投递可能落后于渲染，
        //  会用旧镜像清空正在输入的文本。App→Textarea 回写只发生在语义键
        //  拦截后与异步回填（RestoreQueued）两处。）
        // 渲染探针（task 5.5）：记录构帧耗时，收尾打印（关闭时零开销）。
        let probe_t0 = probe_enabled().then(Instant::now);
        // 虚拟滚动行数对账（task 5.2；渲染点只对行数，归约泵才重测行高，
        // 防 notify 自旋）。
        self.sync_scroller(cx, false);
        // 计时走字对账（task 4.2）：Running 起 1s interval、结束即停。
        self.sync_timer_task(cx);
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
        // thinking 全局序号前缀（共享规则 `thinking_seqs`：空块也消耗、
        // 跨条目穿透；每帧一次前缀扫描，行回调 O(1) 取基值，design D2）。
        let session = self.state.session();
        let entries = session.entries();
        let mut seq_bases = Vec::with_capacity(entries.len() + 1);
        let mut cursor = 0usize;
        for entry in entries {
            seq_bases.push(cursor);
            if let Entry::Assistant { blocks, .. } = entry {
                cursor = thinking_seqs(blocks, cursor).1;
            }
        }
        seq_bases.push(cursor); // 流式尾块基值 = 全部条目出口
        let in_flight_seqs = thinking_seqs(session.in_flight(), cursor).0;
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
                // 条目渲染上下文（task 2.2）：共享覆盖表生效值查询 +
                // 点击回路弱引用（元素只持有 owned 数据，构造完即还）。
                let ctx = EntryCtx {
                    app: &term.state.app,
                    this: this.clone(),
                };
                let entries = session.entries();
                if let Some(entry) = entries.get(index) {
                    entry_element(entry, index, &ctx, seq_bases[index], pal)
                } else if let Some(block) = session.in_flight().get(index - entries.len()) {
                    // 流式尾块（未 message_end）：与条目同样式逐块渲染。
                    let seq = in_flight_seqs.get(index - entries.len()).copied().flatten();
                    assistant_block(block, index, index - entries.len(), &ctx, seq, pal)
                } else if session.bash_running
                    && index == entries.len() + session.in_flight().len()
                {
                    // 运行中 bash 尾槽（TUI layout_bash_tail 同源：共享
                    // bash_entry_model(live)——命令行 + 实时输出 + 运行状态行）。
                    bash_row(
                        &session.bash_command,
                        &session.bash_output,
                        None,
                        false,
                        false,
                        None,
                        session.bash_excluded,
                        true,
                        index,
                        pal,
                    )
                } else {
                    empty
                }
            },
        )
        .scrollbar(true); // 内建滚动条显式启用（task 4.1；默认即 true，自文档化防上游默认变化）
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
                    row = row.bg(pal.comp_selected_bg);
                }
                completion_pane.push(
                    row.child(
                        div()
                            .text_sm()
                            .text_color(if selected {
                                pal.comp_text_selected
                            } else {
                                pal.comp_text
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
                                .text_color(pal.comp_count_text)
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
        // 队列条（task 3.2）：消息区与输入区之间；空队列不占位（复用
        // view::queue_lines 同源输出：前缀行 + 超 2 行 `… N more`）。
        let queue_el = {
            let queue_rows = self.state.app.queue_grid_lines();
            if queue_rows.is_empty() {
                None
            } else {
                let mut strip = div().id("gpui-queue").flex().flex_col().px_2();
                for (i, row) in queue_rows.iter().enumerate() {
                    strip = strip.child(
                        grid_row_with_id(format!("gpui-queue-{i}"), row, line_h_px, &family)
                            .test_text(grid_row_text(row)),
                    );
                }
                Some(strip)
            }
        };
        // extension widget 面板（task 3.3）：` widget ` 标题边框 + 青色文字；
        // 数据经 RPC setWidget → handle_extension_request 写入（null → 清空），
        // 未设置/已清除不占位（spec: extension widget 面板）。
        let widget_el = {
            let lines = self.state.app.widget_lines();
            if lines.is_empty() {
                None
            } else {
                let cyan = to_hsla(Color::Cyan);
                let mut panel = div()
                    .id("gpui-widget")
                    .flex()
                    .flex_col()
                    .border_t_1()
                    .border_color(cyan)
                    .child(
                        div()
                            .px_2()
                            .text_color(cyan)
                            .test_text(" widget ".to_string())
                            .child(" widget "),
                    );
                for line in lines {
                    panel = panel.child(
                        div()
                            .px_2()
                            .text_color(cyan)
                            .test_text(line.clone())
                            .child(line.clone()),
                    );
                }
                Some(panel)
            }
        };
        let root = div()
            .size_full()
            .relative() // 浮层绝对定位的锚（task 3.2）
            .flex()
            .flex_col()
            .bg(pal.chat_bg)
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
            .map(|b| match queue_el {
                Some(strip) => b.child(strip),
                None => b,
            })
            .map(|b| match widget_el {
                Some(panel) => b.child(panel),
                None => b,
            })
            .child(
                // 状态行（task 5.3）：内建 status_bar 分段——左 = header_line
                // 同源数据（phase/会话名/thinking/model/用量/费用/状态文本），
                // 右 = 瞬时提示。
                div()
                    .id("gpui-status")
                    .h(line_h)
                    .bg(pal.status_bg)
                    // 验收反馈（task 1.4）：状态行再上拉 10px，与输入区留隙
                    // 消除重叠。
                    .mb(px(10.))
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
                // 输入区（spec: 输入区简约边框）：白底（随根）、纯黑直角边框
                // 常态 1px / 聚焦 2px（render 读焦点，焦点监听触发重绘）；
                // Textarea appearance(false) 剥 kit 底色/圆角/边框/聚焦光环；
                // 编辑语义仍由内建编辑器自理（task 5.4）。
                div()
                    .id("gpui-input")
                    .flex()
                    .flex_col()
                    .px_2()
                    .map(|b| {
                        if input_border_width(self.focus.is_focused(window)) == 2 {
                            b.border_2()
                        } else {
                            b.border_1()
                        }
                    })
                    .border_color(pal.input_border)
                    .test_support()
                    .child(Textarea::new(&self.textarea).appearance(false)),
            )
            .child(
                // 补全菜单（输入区正下方，task 4.3）。
                div()
                    .id("gpui-completion")
                    .flex()
                    .flex_col()
                    .bg(pal.chat_bg)
                    // 验收反馈（task 1.4）：底部留 10px——空菜单时把输入框
                    // 整体上拉离底边；放 completion 而非根容器，避免影响
                    // 浮层 absolute 锚定。
                    .mb(px(10.))
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

/// 条目渲染上下文（task 2.2）：共享覆盖表查询 + 点击回路。
///
/// 功能: 把 `App` 的覆盖表生效值查询（含全局开关与两张覆盖表的共享
///       生效规则）与组件弱引用一并带入条目/块/卡片渲染（design D1
///       单一事实源——`render_flags()` 接线由 `&App` 承接，补传覆盖表）；
/// 输入: 构造于 MessageScroller 行回调，仅在元素构造期间借用；
/// 输出: 条目元素；其中点击闭包捕获 `this` 弱引用（'static，
///       实体已销毁时静默放弃点击）。
struct EntryCtx<'a> {
    /// 共享 App 状态（全局开关 + 覆盖表生效值查询；仅渲染期借用，不进元素树）。
    app: &'a AppState,
    /// 组件弱引用（点击切换覆盖表后 notify 重绘）。
    this: WeakEntity<GPUIAgentTerminal>,
}

/// 会话条目 → 内建元素（task 5.2：条目级渲染，虚拟滚动逐行回调）。
///
/// 输入: entry — 会话条目；index — 条目序号（元素 id 稳定性）；
///       ctx — 条目渲染上下文（渲染偏好/覆盖表查询/点击回路）；
///       seq_base — 本条目的 thinking 全局序号基值（前缀扫描出口）；
///       pal — 当前 mode 调色板（面板/文字随模式取表）；
/// 输出: 条目行元素（User 标签+markdown / Assistant 分块 / Notice 色条 /
///       Bash 命令+输出摘要；未知内容不 panic）。
fn entry_element(
    entry: &Entry,
    index: usize,
    ctx: &EntryCtx,
    seq_base: usize,
    pal: &'static palette::Palette,
) -> Div {
    match entry {
        Entry::User { text } => div()
            .flex()
            .flex_col()
            .px_2()
            .py_1()
            .child(
                div()
                    .text_sm()
                    .text_color(pal.user_label)
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
            // thinking 全局序号（共享规则：空块也占号，design D2）。
            let (seqs, _) = thinking_seqs(blocks, seq_base);
            let mut col = div().flex().flex_col().px_2().py_1();
            for (bi, block) in blocks.iter().enumerate() {
                col = col.child(assistant_block(block, index, bi, ctx, seqs[bi], pal));
            }
            col
        }
        Entry::Notice { kind, text } => notice_row(kind, text, index, pal),
        Entry::Bash {
            command,
            output,
            exit_code,
            cancelled,
            truncated,
            full_output_path,
            excluded,
        } => bash_row(
            command,
            output,
            *exit_code,
            *cancelled,
            *truncated,
            full_output_path.as_deref(),
            *excluded,
            false, // 完成条目（非运行中尾槽）
            index,
            pal,
        ),
    }
}

/// assistant 块 → 元素（markdown / thinking 折叠 / 工具卡）。
///
/// 输入: block — 内容块；index/bi — 条目与块序号（元素 id）；
///       ctx — 条目渲染上下文（渲染偏好 + 覆盖表 + 点击回路）；
///       thinking_seq — thinking 全局序号（`thinking_seqs` 分配；
///       非 Thinking 块为 None，Thinking 块必为 Some）；
///       pal — 当前 mode 调色板（下行各子块共用）；
/// 输出: 块元素（空文本块给零高占位）。
fn assistant_block(
    block: &Block,
    index: usize,
    bi: usize,
    ctx: &EntryCtx,
    thinking_seq: Option<usize>,
    pal: &'static palette::Palette,
) -> Div {
    let _ = (index, bi);
    match block {
        Block::Text { text } if !text.is_empty() => div().w_full().child(
            div()
                .test_text(text.clone())
                .child(TextView::markdown(format!("md-{index}-{bi}"), text.clone())),
        ),
        Block::Text { .. } => div(),
        Block::Thinking { text } => {
            thinking_block(text, ctx, thinking_seq.expect("Thinking 块必有序号"), pal)
        }
        Block::ToolCall {
            id,
            name,
            args_json,
            tool,
        } => tool_card(id, name, args_json, tool, ctx, index, bi, pal),
    }
}

/// thinking 块 → 元素（空块零占位、占位/正文经共享模型、点击单块切换；
/// design D1/D3，spec「thinking 块语义对齐」）。
///
/// 输入: text — thinking 正文；ctx — 条目渲染上下文；seq — 全局渲染序号
///       （`thinking_seqs` 分配，空块也占号）；pal — 当前 mode 调色板；
/// 输出: 块元素——空块给零高 `div()`（不占行，折叠态也不显占位）；非空块
///       经共享 [`thinking_visible`] 得占位/正文行，带 id 与点击监听
///       （仅非空块可点；点击翻转该块覆盖表，不碰全局与它块）。
fn thinking_block(text: &str, ctx: &EntryCtx, seq: usize, pal: &'static palette::Palette) -> Div {
    // 生效值：单块覆盖优先、否则全局（与 chat.rs append_blocks 同构）。
    let expanded = ctx.app.thinking_expanded_effective(seq);
    // 语义在 thinking_visible（空块 → None 不占行，design D1）。
    let Some(rows) = thinking_visible(text, expanded) else {
        return div();
    };
    let this = ctx.this.clone();
    let block = div()
        .id(format!("thinking-{seq}"))
        .w_full()
        .on_click(move |_, _, cx| {
            let Some(entity) = this.upgrade() else {
                return;
            };
            entity.update(cx, |term, cx| {
                term.state.app.toggle_thinking_block(seq);
                cx.notify();
            });
        });
    let mut block = block.test_support();
    for row in &rows {
        block = block.child(segs_row(row, pal));
    }
    // 观测/点击壳外套一层普通容器保持 Div 返回类型（同 tool_card）。
    div().flex().flex_col().child(block)
}

/// 工具卡面板底三态 → 按 mode 取表（task1.3 纯逻辑，无 TTY 可测）。
///
/// 输入: status — 执行状态；is_error — 失败标记（仅 Ended 生效）；
///       pal — 当前 mode 调色板（双表之一）；
/// 输出: 面板底 Rgba（运行/失败/成功三态，浅深两套值）
fn card_panel_bg(status: &ToolStatus, is_error: bool, pal: &'static palette::Palette) -> Rgba {
    match status {
        ToolStatus::Pending | ToolStatus::Running => pal.card_bg_pending,
        ToolStatus::Ended if is_error => pal.card_bg_err,
        ToolStatus::Ended => pal.card_bg_ok,
    }
}

/// 工具卡（消费中性模型 `tool_card_model`：头行 + 折叠/展开体 + 提示行 +
/// 受保护尾部；面板底色随状态三态，design D1）。
///
/// 输入: id — toolCallId（元素 id 与单卡覆盖键）；name — 工具名；
///       args_json — 调用参数原文；tool — 执行状态；
///       ctx — 条目渲染上下文（生效展开态查询 + 点击回路）；
///       index/bi — 元素 id 序号；pal — 当前 mode 调色板（面板底随模式）；
/// 输出: 卡片元素（行内着色由段角色 → palette 映射；仅 `result.is_some()`
///       的卡挂点击监听——与 TUI 命中门控同源，design D2/D4）。
#[allow(clippy::too_many_arguments)] // pal 穿参（task1.2）+ bash_row 同款
fn tool_card(
    id: &str,
    name: &str,
    args_json: &str,
    tool: &ToolExecution,
    ctx: &EntryCtx,
    index: usize,
    bi: usize,
    pal: &'static palette::Palette,
) -> Div {
    let _ = (index, bi);
    // 单卡生效展开态来自共享覆盖表（task 2.2：与 ratatui 同一事实源）。
    let expanded = ctx.app.tool_expanded_effective(id);
    // read/write 正文预览的语法主题走共享 TuiTheme 默认（syntax 不在本
    // change 的 spec 语义 token 清单内）；面板底随 mode 取表（task1.3）。
    let model = tool_card_model(name, args_json, tool, expanded, &TuiTheme::default());
    let panel = card_panel_bg(&tool.status, tool.is_error, pal);
    // 测试探针（task3.2）：记录最近渲染的卡面板底色（仅 test 构建）。
    #[cfg(test)]
    PROBE_CARD_BG.with(|c| c.set(pack_rgb(panel)));
    let mut card = div()
        .id(format!("tool-card-{id}"))
        .flex()
        .flex_col()
        .px_2()
        .py_1()
        .bg(panel);
    // 点击切换单卡覆盖：仅已有最终结果的卡响应（与 TUI 命中门控同源）。
    if tool.result.is_some() {
        let this = ctx.this.clone();
        let tool_id = id.to_string();
        card = card.on_click(move |_, _, cx| {
            let Some(entity) = this.upgrade() else {
                return;
            };
            entity.update(cx, |term, cx| {
                term.state.app.toggle_tool_expanded(&tool_id);
                cx.notify();
            });
        });
    }
    let mut card = card.test_support();
    // 头行（状态相关，恒显）。
    card = card.child(segs_row(&model.head, pal));
    // 折叠面（预览 + 提示行）仅折叠态作为常显子节点；展开面正文经内建
    // Collapsible 挂载（open=生效态：不打开即不挂载、不占位，design D2）。
    card = card.when(!expanded, |c| {
        let mut c = c.child(segs_col(&model.body, pal));
        if let Some(hint) = &model.hint {
            c = c.child(segs_row(std::slice::from_ref(hint), pal));
        }
        c
    });
    card = card.child(
        Collapsible::new()
            .open(expanded)
            .content(segs_col(&model.body, pal)),
    );
    // 受保护尾部（exit 状态行/截断警告）恒显。
    for row in &model.tail {
        card = card.child(segs_row(row, pal));
    }
    // 观测/点击壳外再套一层普通容器以保持 Div 返回类型；flex_col 维持
    // 卡片整行拉伸（与直接挂在条目列同宽，布局无损）。
    div().flex().flex_col().child(card)
}

/// 中性段行 → 内建文本行（GPUI 侧适配：逐段 runs 着色/字重，design D1）。
///
/// 输入: segs — 单行语义段（头行/正文行/提示行/尾部行）；
///       pal — 当前 mode 调色板（段色随模式取表）；
/// 输出: 行元素（空行以单个空格保行高；test_text 记录整行文本供快照断言）
fn segs_row(segs: &[Seg], pal: &'static palette::Palette) -> Div {
    let full: String = segs.iter().map(|s| s.text.as_str()).collect();
    let mut text = String::new();
    let mut runs: Vec<TextRun> = Vec::new();
    for seg in segs {
        if seg.text.is_empty() {
            continue;
        }
        let start = text.len();
        text.push_str(&seg.text);
        let (color, weight, style) = seg_visual(seg.role, pal);
        runs.push(TextRun {
            len: text.len() - start,
            font: Font {
                weight,
                style,
                ..Font::default()
            },
            color,
            ..Default::default()
        });
    }
    if text.is_empty() {
        text.push(' '); // 空行保行高（正文空行占位）
    }
    let mut styled = StyledText::new(text);
    if !runs.is_empty() {
        styled = styled.with_runs(runs);
    }
    div().child(div().test_text(full).child(styled))
}

/// 中性段行序列 → 垂直列元素（卡片正文/预览多行装配，design D1）。
///
/// 输入: rows — 行段序列（模型 body 面：展开全文或折叠预览）；
///       pal — 当前 mode 调色板（下传逐行）；
/// 输出: flex_col 容器（逐行 [`segs_row`]；空序列给空列）。
fn segs_col(rows: &[Vec<Seg>], pal: &'static palette::Palette) -> Div {
    let mut col = div().flex().flex_col();
    for row in rows {
        col = col.child(segs_row(row, pal));
    }
    col
}

/// 段角色 → (颜色, 字重, 字形)：palette 收拢的纯视图映射（design D2）。
///
/// 输入: role — 中性段角色（卡片角色面向卡面板、通知/bash/thinking
///       角色面向消息区）；pal — 当前 mode 调色板（双表按模式取值）；
/// 输出: 三元组（GPUI 前景色、字重、字形）
fn seg_visual(role: SegRole, pal: &'static palette::Palette) -> (Hsla, FontWeight, FontStyle) {
    let regular = FontWeight::default();
    let normal = FontStyle::default();
    let bold = FontWeight::BOLD;
    let italic = FontStyle::Italic;
    match role {
        // —— 工具卡（面板色随 mode）——
        SegRole::StatusRun => (pal.card_status_run.into(), regular, normal),
        SegRole::StatusOk => (pal.card_status_ok.into(), regular, normal),
        SegRole::StatusErr => (pal.card_status_err.into(), regular, normal),
        SegRole::Name => (pal.card_text.into(), bold, normal),
        SegRole::Arg => (pal.card_text.into(), regular, normal),
        SegRole::Timer | SegRole::Hint => (pal.card_dim.into(), regular, normal),
        SegRole::Body => (pal.tool_preview_text.into(), regular, normal),
        SegRole::Fg(c) => (fg_on_card(c, pal), regular, normal),
        // —— 消息区 ——
        SegRole::NoticeError => (pal.notice_error.into(), bold, normal),
        SegRole::NoticeRetry => (pal.notice_retry.into(), regular, normal),
        SegRole::Notice => (pal.notice_default.into(), regular, italic),
        SegRole::ThinkingText => (pal.thinking_text.into(), regular, italic),
        SegRole::Cmd => (pal.bash_cmd.into(), bold, normal),
        SegRole::Warn => (pal.warn.into(), regular, normal),
        SegRole::ExitOk => (pal.exit_ok.into(), regular, normal),
        SegRole::ExitBad => (pal.exit_bad.into(), bold, normal),
        SegRole::Dim => (pal.bash_body.into(), regular, normal),
    }
}

/// `Fg` 段在卡片面板上的颜色：ANSI 命名色按 mode 取表（浅底加深、深底
/// 提亮），Rgb 主题色（语法高亮等）原样透传。
///
/// 输入: c — 段携带的 ratatui 前景色；pal — 当前 mode 调色板；
/// 输出: 提亮/透传后的 Hsla
fn fg_on_card(c: Color, pal: &'static palette::Palette) -> Hsla {
    match c {
        Color::Red => pal.card_fg_red.into(),
        Color::Green => pal.card_fg_green.into(),
        Color::Yellow => pal.card_fg_yellow.into(),
        Color::Cyan => pal.card_fg_cyan.into(),
        Color::DarkGray => pal.card_fg_gray.into(),
        other => to_hsla(other),
    }
}

/// 通知行（消费共享 `notice_model`：`[kind]` 前缀并入文本，色/粗/斜由段
/// 角色 → palette 映射——对齐 TUI notice_line 语义，design D1）。
///
/// 输入: kind — 通知类别（"error"/"retry"/其它）；text — 通知正文；
///       index — 元素 id 序号；pal — 当前 mode 调色板；
/// 输出: 色条行元素（`[kind] text` 单段；test_text 记录整行）。
fn notice_row(kind: &str, text: &str, index: usize, pal: &'static palette::Palette) -> Div {
    let _ = index;
    div()
        .px_2()
        .py_1()
        .child(segs_row(&notice_model(kind, text), pal))
}

/// bash 条目（消费共享 `bash_entry_model`：命令行 + 尾部 500 行截断 +
/// 状态行——与 TUI `append_bash` 同源薄适配，design D1）。
///
/// 输入: command/output — 命令与输出；exit_code/cancelled/truncated/
///       full_output_path — 结果态；excluded — `!!` 不进上下文；
///       live — 运行中尾槽（状态行为 `⟳ 运行中…`，结果字段传 None/false）；
///       index — 元素 id 序号；pal — 当前 mode 调色板；
/// 输出: 行元素（逐行 [`segs_row`]；完成条目 live=false、运行中尾槽
///       live=true——语义都在共享 `bash_entry_model`，design D1）。
#[allow(clippy::too_many_arguments)]
fn bash_row(
    command: &str,
    output: &str,
    exit_code: Option<i64>,
    cancelled: bool,
    truncated: bool,
    full_output_path: Option<&str>,
    excluded: bool,
    live: bool,
    index: usize,
    pal: &'static palette::Palette,
) -> Div {
    let _ = index;
    let mut row = div().flex().flex_col().px_2().py_1();
    // 语义在 bash_entry_model（尾部截断/状态行/不进上下文标记，design D1）。
    for segs in bash_entry_model(
        command,
        output,
        exit_code,
        cancelled,
        truncated,
        full_output_path,
        excluded,
        live,
    ) {
        row = row.child(segs_row(&segs, pal));
    }
    row
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
        // 全局折叠开关（task 4.1）：ctrl+t / ctrl+o 进 App，裸 t/o 放行
        assert_eq!(
            route_keystroke(false, false, &k("t", true, false, false)),
            Route::ToApp
        );
        assert_eq!(
            route_keystroke(false, false, &k("o", true, false, false)),
            Route::ToApp
        );
        assert_eq!(
            route_keystroke(false, true, &k("o", true, false, false)),
            Route::ToApp
        );
        assert_eq!(
            route_keystroke(false, false, &k("t", false, false, false)),
            Route::ToEditor
        );
        assert_eq!(
            route_keystroke(false, false, &k("o", false, false, false)),
            Route::ToEditor
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

        // —— 共享键臂（task 2.1，镜像 key_intent）——
        assert_eq!(
            route_keystroke(false, false, &k("x", true, false, false)),
            Route::ToApp,
            "ctrl+x 复制"
        );
        assert_eq!(
            route_keystroke(false, false, &k("p", true, false, false)),
            Route::ToApp,
            "ctrl+p 模型循环"
        );
        assert_eq!(
            route_keystroke(false, false, &k("p", true, true, false)),
            Route::ToApp,
            "ctrl+shift+p 上一个模型"
        );
        assert_eq!(
            route_keystroke(false, false, &k("up", false, false, true)),
            Route::ToApp,
            "alt+up 取回排队"
        );
        assert_eq!(
            route_keystroke(false, false, &k("tab", false, true, false)),
            Route::ToApp,
            "shift+tab thinking 循环（gpui: tab+shift）"
        );
        assert_eq!(
            route_keystroke(false, false, &k("tab", false, false, false)),
            Route::ToApp,
            "菜单关闭 tab = 强制路径补全"
        );

        // —— 补全臂收紧（task 2.1）：不吞 ctrl 组合 ——
        assert_eq!(
            route_keystroke(false, true, &k("tab", false, false, false)),
            Route::ToApp,
            "菜单开 tab 应用候选"
        );
        assert_eq!(
            route_keystroke(false, true, &k("pageup", false, false, false)),
            Route::ToApp,
            "菜单开 plain pageup = 菜单翻页"
        );
        assert_eq!(
            route_keystroke(false, true, &k("down", true, false, false)),
            Route::ToScroller,
            "菜单开 ctrl+↓ 不进菜单臂（D1 关键收紧）"
        );
        assert_eq!(
            route_keystroke(false, true, &k("up", true, false, false)),
            Route::ToScroller,
            "菜单开 ctrl+↑ 照常跳消息区（TUI 同语义）"
        );
        assert_eq!(
            route_keystroke(false, true, &k("pageup", true, false, false)),
            Route::ToScroller,
            "菜单开 ctrl+pageup 仍滚动消息区"
        );

        // —— 滚动键臂（task 3.1 → ToScroller）——
        for key in ["pageup", "pagedown"] {
            assert_eq!(
                route_keystroke(false, false, &k(key, false, false, false)),
                Route::ToScroller,
                "{key} 整页"
            );
            assert_eq!(
                route_keystroke(false, false, &k(key, false, true, false)),
                Route::ToScroller,
                "{key} shift 半页"
            );
        }
        assert_eq!(
            route_keystroke(false, false, &k("home", true, false, false)),
            Route::ToScroller,
            "ctrl+home 归顶"
        );
        assert_eq!(
            route_keystroke(false, false, &k("end", true, false, false)),
            Route::ToScroller,
            "ctrl+end 贴底"
        );
        assert_eq!(
            route_keystroke(false, false, &k("home", false, false, false)),
            Route::ToEditor,
            "裸 home 仍是编辑键"
        );
        assert_eq!(
            route_keystroke(false, false, &k("up", true, false, false)),
            Route::ToScroller,
            "ctrl+↑ 消息跳转"
        );
        assert_eq!(
            route_keystroke(false, false, &k("down", true, true, false)),
            Route::ToScroller,
            "ctrl+shift+↓ 同义"
        );
        assert_eq!(
            route_keystroke(false, false, &k("up", false, false, false)),
            Route::ToEditor,
            "裸 up 仍归编辑器"
        );
    }

    /// 共享键端到端（task 2.2；spec: 全局键位补全 P1）：逐键断言进共享分发
    /// 且不落输入编辑器——ctrl+p 换模型（命令入日志、输入不变）、shift+tab
    /// 换 thinking、alt+up 取回排队回填输入、tab 打开路径补全菜单。
    ///
    /// 功能: 录制桩窗口逐键验证四个 P1 共享键的可观测效果；
    /// 输入: cx — 测试上下文；
    /// 输出: 断言通过（四键全部经共享层生效且输入区不被误改）。
    #[gpui_kit::test]
    fn p1_shared_keys_route_to_shared_dispatch(cx: &mut TestAppContext) {
        let (path, log) = write_recording_stub("p1-keys");
        let session = RpcSession::spawn(&path, &[], None).expect("spawn 应成功");
        let (handle, term) = open_with_session(cx, session);
        let render = |cx: &mut TestAppContext| {
            cx.update_window(handle, |_, window, cx| window.render_frame(cx))
                .expect("render");
            cx.run_until_parked();
        };
        let press = |cx: &mut TestAppContext, key: &str| {
            cx.update_window(handle, |_, window, cx| window.press(key, cx))
                .unwrap_or_else(|e| panic!("press {key}: {e}"));
            cx.run_until_parked();
        };
        let input_value = |cx: &mut TestAppContext| {
            cx.update(|cx| term.read(cx).textarea.read(cx).value().to_string())
        };
        render(cx);

        // 1) ctrl+p 模型循环：cycle_model 入日志，输入区文本不被改动。
        cx.update_window(handle, |_, window, cx| window.input("keep-me", cx))
            .expect("input");
        render(cx);
        press(cx, "ctrl-p");
        assert_eq!(input_value(cx), "keep-me", "ctrl+p 不应改动输入区");
        assert!(
            wait_log(cx, &log, "cycle_model", 2000),
            "模型循环命令入日志: {:?}",
            fs::read_to_string(&log).unwrap_or_default()
        );

        // 2) shift+tab thinking 级别循环（cycle_thinking_level 入日志）。
        press(cx, "shift-tab");
        assert!(
            wait_log(cx, &log, "cycle_thinking_level", 2000),
            "thinking 级别命令入日志: {:?}",
            fs::read_to_string(&log).unwrap_or_default()
        );

        // 3) alt+up 取回排队 → 输入框回填。
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state.reduce(
                    &ClientEvent::Event(serde_json::json!({
                        "type":"queue_update","steering":[],"followUp":["queued-msg"]
                    })),
                    &term.commander,
                    &term.ui_tx,
                );
            });
        });
        render(cx);
        press(cx, "alt-up");
        // 等 clear_queue 落盘（请求门控已过）→ 显式拉回报泵（测试调度器
        // 不靠后台线程唤醒）→ 假 pi 应答回填固定文案「queued msg」。
        assert!(
            wait_log(cx, &log, "clear_queue", 2000),
            "dequeue 应发 clear_queue: {:?}",
            fs::read_to_string(&log).unwrap_or_default()
        );
        // 应答 → UiEvent → ui_rx 是异步链：轮询拉回报泵直到回填（≤2s）。
        let mut restored = String::new();
        for _ in 0..80 {
            cx.update(|cx| term.update(cx, |term, cx| term.pump_ui_events(cx)));
            restored = input_value(cx);
            if restored.contains("queued msg") {
                break;
            }
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(
            restored.contains("queued msg"),
            "alt+up 取回排队到输入框: {restored:?}"
        );

        // 4) tab（菜单关闭）→ 路径补全菜单开（共享层状态，非文本插入）。
        //    ctrl+c 仅此一次（500ms 内双击 = 退出，会关窗）。
        press(cx, "ctrl-c");
        assert_eq!(input_value(cx), "", "ctrl+c 清空输入");
        press(cx, "tab");
        let (open, n) = cx.update(|cx| {
            let a = &term.read(cx).state.app.completion;
            (a.open, a.items.len())
        });
        assert!(open && n > 0, "tab 打开路径补全: open={open} items={n}");
    }

    /// 写应答 `get_last_assistant_text` 带文本的假 pi（task 2.3 ctrl+x 正例）。
    ///
    /// 功能: 落盘可执行脚本（取回复命令回填 data.text，其余通用 success）；
    /// 输入: name — 目录区分名；
    /// 输出: (脚本路径, 命令日志路径)（unix chmod 755）。
    fn write_text_stub(name: &str) -> (String, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("gpui-copy-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let log = dir.join("commands.log");
        let body = r#"#!/bin/sh
LOG='@@LOG@@'
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  id=$(printf '%s' "$line" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
  case "$line" in
    *'"type":"get_last_assistant_text"'*)
      printf '{"type":"response","id":"%s","success":true,"data":{"text":"hello from assistant"}}\n' "$id" ;;
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

    /// ctrl+x 复制最后回复（task 2.3；spec: 窗口形态剪贴板复制正例）：
    /// 注入 `copy_text` sink → ctrl+x → `get_last_assistant_text` 应答回填 →
    /// sink 收到文本（与 /copy 同通路；出口 sink 优先、stdout 落回）。
    ///
    /// 功能: 录制窗口逐键验证复制链；
    /// 输入: cx — 测试上下文；
    /// 输出: 断言通过（sink 收到应答文本）。
    #[gpui_kit::test]
    fn ctrl_x_copies_last_reply_via_sink(cx: &mut TestAppContext) {
        use crate::app::TerminalSinks;
        use std::sync::{Arc, Mutex};
        let (path, log) = write_text_stub("ctrlx");
        let session = RpcSession::spawn(&path, &[], None).expect("spawn 应成功");
        let (handle, term) = open_with_session(cx, session);
        let got = Arc::new(Mutex::new(String::new()));
        cx.update(|cx| {
            term.update(cx, |term, _| {
                let g = got.clone();
                term.state.app.set_terminal_sinks(TerminalSinks {
                    copy_text: Some(Box::new(move |t| *g.lock().unwrap() = t)),
                    ..Default::default()
                });
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();

        cx.update_window(handle, |_, window, cx| window.press("ctrl-x", cx))
            .expect("ctrl-x");
        assert!(
            wait_log(cx, &log, "get_last_assistant_text", 2000),
            "ctrl+x 应发取回复命令: {:?}",
            fs::read_to_string(&log).unwrap_or_default()
        );
        // 应答 → AsyncData → 回报泵（测试需显式拉）→ sink 落文本。
        let mut copied = String::new();
        for _ in 0..80 {
            cx.update(|cx| term.update(cx, |term, cx| term.pump_ui_events(cx)));
            copied = got.lock().unwrap().clone();
            if !copied.is_empty() {
                break;
            }
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert_eq!(copied, "hello from assistant", "sink 收到最后回复文本");
    }

    /// ctrl+x 无回复 → 红条（task 2.3；spec: 复制最后回复场景的无内容分支）。
    ///
    /// 功能: 通用桩（应答无 data）→ ctrl+x → 拉泵 → notice 红条；
    /// 输入: cx — 测试上下文；
    /// 输出: 断言通过（"暂无可复制" 红色通知）。
    #[gpui_kit::test]
    fn ctrl_x_without_reply_is_red_notice(cx: &mut TestAppContext) {
        let (path, log) = write_recording_stub("ctrlx-empty");
        let session = RpcSession::spawn(&path, &[], None).expect("spawn 应成功");
        let (handle, term) = open_with_session(cx, session);
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();

        cx.update_window(handle, |_, window, cx| window.press("ctrl-x", cx))
            .expect("ctrl-x");
        assert!(
            wait_log(cx, &log, "get_last_assistant_text", 2000),
            "ctrl+x 应发取回复命令: {:?}",
            fs::read_to_string(&log).unwrap_or_default()
        );
        let mut notice = None;
        for _ in 0..80 {
            cx.update(|cx| term.update(cx, |term, cx| term.pump_ui_events(cx)));
            notice = cx.update(|cx| term.read(cx).state.app.notice.clone());
            if notice.is_some() {
                break;
            }
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        let (text, color) = notice.expect("应有通知");
        assert!(text.contains("暂无可复制"), "红条文案: {text}");
        assert!(
            matches!(color, ratatui::style::Color::Red),
            "应为红色: {color:?}"
        );
    }

    /// ctrl+v 文本粘贴（task 2.3；spec: 文本粘贴场景）：写测试剪贴板 →
    /// 派发宿主 `Paste` 动作（与平台 Cmd+V 同路径，内建编辑器自理）→
    /// 光标处插入文本。
    ///
    /// 功能: 剪贴板文本经内建粘贴行为入输入区；
    /// 输入: cx — 测试上下文；
    /// 输出: 断言通过（文本插入；不通则停下回看——spec 契约依赖内建行为）。
    #[gpui_kit::test]
    fn ctrl_v_pastes_clipboard_text(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "paste-text");
        cx.update(|cx| {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("pasted text".into()))
        });
        cx.update_window(handle, |_, window, cx| {
            window.dispatch_action(Box::new(gpui_kit::base::input::Paste), cx)
        })
        .expect("dispatch paste");
        cx.run_until_parked();
        let mut v = String::new();
        for _ in 0..40 {
            v = cx.update(|cx| term.read(cx).textarea.read(cx).value().to_string());
            if v.contains("pasted text") {
                break;
            }
            cx.update_window(handle, |_, window, cx| window.render_frame(cx))
                .ok();
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(v.contains("pasted text"), "剪贴板文本插入光标处: {v:?}");
    }

    /// 被闸命令逐个验证（task 5.2；spec: 斜杠命令全集分发）：原子集外命令
    /// 全部真执行——RPC 入日志 / 执行提示非桩 / 桩同款 / 未知交普通文本。
    ///
    /// 功能: 录制桩窗口逐条输入命令 + 回车，断言各自可观测效果；
    /// 输入: cx — 测试上下文；
    /// 输出: 断言通过（全集矩阵；/export 走本地会话文件分支故断言提示非桩）。
    #[gpui_kit::test]
    fn gpui_slash_full_set_e2e(cx: &mut TestAppContext) {
        let (path, log) = write_recording_stub("slash-full");
        let session = RpcSession::spawn(&path, &[], None).expect("spawn 应成功");
        let (handle, term) = open_with_session(cx, session);
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();
        let submit = |cx: &mut TestAppContext, text: &str| {
            cx.update_window(handle, |_, window, cx| window.input(text, cx))
                .unwrap_or_else(|e| panic!("input {text}: {e}"));
            cx.update_window(handle, |_, window, cx| window.press("enter", cx))
                .expect("enter");
            cx.run_until_parked();
        };
        // RPC 型：命令入日志即证真执行（桩不发 RPC）。
        for (text, needle) in [
            ("/new", "new_session"),
            ("/clone", "clone"),
            ("/name renamed", "set_session_name"),
            ("/reload", "get_commands"),
            ("/copy", "get_last_assistant_text"),
        ] {
            submit(cx, text);
            assert!(
                wait_log(cx, &log, needle, 2000),
                "{text} 应发 {needle}: {:?}",
                fs::read_to_string(&log).unwrap_or_default()
            );
        }

        // /export：无会话文件时走本地红条分支（仍非桩、不开浮层）。
        submit(cx, "/export");
        let notice = cx
            .update(|cx| term.read(cx).state.app.notice.clone())
            .expect("/export 应有执行提示");
        assert!(
            !notice.0.contains("暂不支持"),
            "/export 应执行而非桩: {}",
            notice.0
        );

        // 桩型：/share 与 TUI 同款网络上报桩（同步提示，不发 RPC）。
        submit(cx, "/share");
        let notice = cx
            .update(|cx| term.read(cx).state.app.notice.clone())
            .expect("/share 应给桩");
        assert!(notice.0.contains("暂不支持"), "/share 桩同 TUI: {}", notice.0);

        // 未知命令：按普通 prompt 发送（消息体含原文）。
        submit(cx, "/definitely-not-a-cmd");
        assert!(
            wait_log(cx, &log, "definitely-not-a-cmd", 2000),
            "未知斜杠交普通发送: {:?}",
            fs::read_to_string(&log).unwrap_or_default()
        );
    }

    /// ctrl+j 换行验证（task 2.4；design D5）：内建键位表无 ctrl-j 绑定，
    /// 实测原生行为并记录结论——**不桥接 App.editor**（GPUI 文本所有权留
    /// Textarea 是既定边界）；若 kit 未来加绑定则翻转断言并补路由。
    ///
    /// 功能: 输入 → 按 ctrl+j → 续输入 → 断言是否产生换行；
    /// 输入: cx — 测试上下文；
    /// 输出: 断言通过（原生不换行 = 已知差异）或失败翻转（原生换行则改路由断言）。
    #[gpui_kit::test]
    fn ctrl_j_newline_native_behavior(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "ctrl-j");
        cx.update_window(handle, |_, window, cx| window.input("line1", cx))
            .expect("input");
        cx.update_window(handle, |_, window, cx| window.press("ctrl-j", cx))
            .expect("ctrl-j");
        cx.update_window(handle, |_, window, cx| window.input("line2", cx))
            .expect("input2");
        cx.run_until_parked();
        let v = cx.update(|cx| term.read(cx).textarea.read(cx).value().to_string());
        eprintln!("[ctrl+j] value={v:?}");
        assert!(
            !v.contains('\n'),
            "kit 原生无 ctrl+j 换行（已知差异，记 Docs；若此断言失败 = kit 已支持，改补路由断言）: {v:?}"
        );
    }

    /// 滚动键端到端（task 3.2/3.3；spec: 消息区滚动命令对齐）：顶/底、
    /// 整页/半页、消息跳转的落点与端点、输入区不变、菜单与浮层下不滚。
    ///
    /// 功能: 长会话 + 预填输入 → 逐键断言 `logical_scroll_top`/跟随态；
    ///       菜单开时页键归菜单（滚动不动）；浮层开时滚动键不生效；
    /// 输入: cx — 测试上下文；
    /// 输出: 断言通过（滚动真相 = scroller，输入全程不变）。
    #[gpui_kit::test]
    fn scroller_keys_scroll_and_leave_input_alone(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "scroll-keys");
        cx.update(|cx| {
            term.update(cx, |term, _| {
                for _ in 0..12 {
                    feed_user_lines(term, 60);
                }
            });
        });
        let render = |cx: &mut TestAppContext| {
            cx.update_window(handle, |_, window, cx| window.render_frame(cx))
                .expect("render");
            cx.run_until_parked();
        };
        let press = |cx: &mut TestAppContext, key: &str| {
            cx.update_window(handle, |_, window, cx| window.press(key, cx))
                .unwrap_or_else(|e| panic!("press {key}: {e}"));
            cx.run_until_parked();
        };
        let top = |cx: &mut TestAppContext| {
            cx.update(|cx| {
                let s = term.read(cx).scroller.read(cx);
                (s.is_following_tail(), s.logical_scroll_top())
            })
        };
        render(cx);
        // 预填输入：全程断言滚动键不碰输入区。
        cx.update_window(handle, |_, window, cx| window.input("keep", cx))
            .expect("input");
        render(cx);
        let value = |cx: &mut TestAppContext| {
            cx.update(|cx| term.read(cx).textarea.read(cx).value().to_string())
        };

        // 顶：ctrl+home → 首条、离开跟随。
        press(cx, "ctrl-home");
        let (follow, t) = top(cx);
        assert_eq!(t.item_ix, 0, "ctrl+home 归顶");
        assert!(!follow, "顶点不跟随");

        // 整页向下/向上：item_ix 增/减（条目粒度估算步进，S1 结论）。
        press(cx, "pagedown");
        let (_, t_down) = top(cx);
        assert!(t_down.item_ix > 0, "pagedown 向后: {:?}", t_down.item_ix);
        press(cx, "pageup");
        let (_, t_up) = top(cx);
        assert!(t_up.item_ix < t_down.item_ix, "pageup 向前");

        // 半页（shift+pagedown 反向语义：从 pageup 落点再向后，确有移动）。
        press(cx, "shift-pagedown");
        let (_, t_half) = top(cx);
        assert!(t_half.item_ix >= t_up.item_ix, "shift+pagedown 向后移动");

        // 消息跳转（全 user 条目 = 每条都是消息起点）：ctrl+↓ 下一条、ctrl+↑ 回。
        press(cx, "ctrl-down");
        let (_, t_j1) = top(cx);
        assert_eq!(t_j1.item_ix, t_half.item_ix + 1, "ctrl+↓ 下一条消息");
        press(cx, "ctrl-up");
        let (_, t_j2) = top(cx);
        assert_eq!(t_j2.item_ix, t_half.item_ix, "ctrl+↑ 回原消息");

        // 底：ctrl+end → 贴底恢复跟随。
        press(cx, "ctrl-end");
        let (follow_end, _) = top(cx);
        assert!(follow_end, "ctrl+end 贴底恢复跟随");

        // 输入全程不变（滚动不碰编辑器）。
        assert_eq!(value(cx), "keep", "滚动键不应改动输入区");

        // 菜单下：pageup 归菜单整页翻页，滚动不动。
        press(cx, "ctrl-c"); // 单次清空（双击会关窗）
        assert_eq!(value(cx), "", "清空输入");
        press(cx, "tab");
        let (open, n) = cx.update(|cx| {
            let a = &term.read(cx).state.app.completion;
            (a.open, a.items.len())
        });
        assert!(open && n > 0, "菜单开: {open}/{n}");
        // 从贴底位起测（若页键漏进 scroller 会明显回退 → 可检测）。
        let (_, before_menu) = top(cx);
        press(cx, "pageup");
        let (_, after_menu) = top(cx);
        assert_eq!(
            before_menu.item_ix, after_menu.item_ix,
            "菜单开时页键不滚消息区"
        );
        press(cx, "escape"); // 关菜单

        // 浮层下：滚动键不生效（浮层消费焦点键），浮层保持打开。
        // 造一个可检测态：顶点且非跟随（若 ctrl+end 漏生效会变贴底跟随）。
        press(cx, "ctrl-home");
        let (follow_before, before_overlay) = top(cx);
        assert_eq!(before_overlay.item_ix, 0, "已到顶");
        assert!(!follow_before, "顶点非跟随");
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state.app.overlay = Some(crate::app::overlay::Overlay::Info {
                    title: "S".into(),
                    lines: vec!["x".into()],
                    loading: false,
                    seq: 1,
                });
            });
        });
        render(cx);
        press(cx, "ctrl-end");
        let (follow_after, after_overlay) = top(cx);
        assert_eq!(
            (before_overlay.item_ix, follow_before),
            (after_overlay.item_ix, follow_after),
            "浮层下滚动键不滚消息区"
        );
        assert!(
            cx.update(|cx| term.read(cx).state.app.overlay.is_some()),
            "浮层保持打开"
        );
    }

    /// 滚动条（task 4.1；spec: 消息区滚动条三场景）：内建滚动条默认启用
    /// （MessageScroller `scrollbar: true`）——验证我方边界：不足一屏拖动
    /// 无效果、超视口拖动改变视口、浮层遮罩下拖动无效果；绘制/淡入淡出/
    /// 连续拖动手感为宿主内建行为（design D3，组件自身套件覆盖）。
    ///
    /// 功能: 手工分发 hover→down→move→up 事件序列于消息区右缘；
    /// 输入: cx — 测试上下文；
    /// 输出: 断言通过（存在性命中 + 溢出才可拖 + 遮罩阻断）。
    #[gpui_kit::test]
    fn scrollbar_drag_only_when_overflow_and_unoccluded(cx: &mut TestAppContext) {
        use gpui_kit::{MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PlatformInput};
        let (handle, term) = open_focused(cx, "scrollbar");
        let render = |cx: &mut TestAppContext| {
            cx.update_window(handle, |_, window, cx| window.render_frame(cx))
                .expect("render");
            cx.run_until_parked();
        };
        let top = |cx: &mut TestAppContext| {
            cx.update(|cx| {
                let s = term.read(cx).scroller.read(cx);
                (s.is_following_tail(), s.logical_scroll_top())
            })
        };
        let press_key = |cx: &mut TestAppContext, key: &str| {
            cx.update_window(handle, |_, window, cx| window.press(key, cx))
                .unwrap_or_else(|e| panic!("press {key}: {e}"));
            cx.run_until_parked();
        };
        // 右缘拖动：hover → down → move×2 → up（hover-to-show 先手）。
        let drag = |cx: &mut TestAppContext, x: f32, y: f32, dy: f32| {
            let mods = Modifiers::default();
            let at = |yy: f32| point(px(x), px(yy));
            cx.update_window(handle, |_, window, cx| {
                let _ = window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: at(y),
                        pressed_button: None,
                        modifiers: mods,
                    }),
                    cx,
                );
            })
            .expect("hover");
            // hover-to-show 入场动画按真实时间推进：多帧 + 短睡让命中区就位。
            for _ in 0..6 {
                cx.update_window(handle, |_, window, cx| window.render_frame(cx))
                    .ok();
                cx.run_until_parked();
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
            cx.update_window(handle, |_, window, cx| {
                let _ = window.dispatch_event(
                    PlatformInput::MouseDown(MouseDownEvent {
                        button: MouseButton::Left,
                        position: at(y),
                        modifiers: mods,
                        click_count: 1,
                        first_mouse: true,
                    }),
                    cx,
                );
                for k in [1.0f32, 2.0] {
                    let _ = window.dispatch_event(
                        PlatformInput::MouseMove(MouseMoveEvent {
                            position: at(y + dy * k),
                            pressed_button: Some(MouseButton::Left),
                            modifiers: mods,
                        }),
                        cx,
                    );
                }
                let _ = window.dispatch_event(
                    PlatformInput::MouseUp(MouseUpEvent {
                        button: MouseButton::Left,
                        position: at(y + dy * 2.0),
                        modifiers: mods,
                        click_count: 1,
                    }),
                    cx,
                );
            })
            .expect("dispatch mouse");
            cx.run_until_parked();
        };
        let vp_bounds = |cx: &mut TestAppContext| {
            cx.update_window(handle, |_, window, _| {
                gpui_kit::base::test_support::find(window, &[], &ElementId::from("gpui-messages"))
                    .expect("视口观测")
                    .bounds()
            })
            .expect("vp")
        };

        // 1) 不足一屏：右缘拖动无效果（无滚动条命中）。
        cx.update(|cx| {
            term.update(cx, |term, _| feed_user_lines(term, 3));
        });
        render(cx);
        let vp1 = vp_bounds(cx);
        let (f1, t1) = top(cx);
        drag(
            cx,
            f32::from(vp1.origin.x + vp1.size.width) - 6.0,
            f32::from(vp1.origin.y) + 6.0,
            120.0,
        );
        let (f1b, t1b) = top(cx);
        assert_eq!((f1, t1), (f1b, t1b), "不足一屏拖动不滚消息区");

        // 2) 超视口：拖动改变视口（从顶起，拇指在顶部区域）。
        cx.update(|cx| {
            term.update(cx, |term, _| {
                for _ in 0..9 {
                    feed_user_lines(term, 60);
                }
            });
        });
        render(cx);
        press_key(cx, "ctrl-home");
        let (_, t2) = top(cx);
        assert_eq!(t2.item_ix, 0, "到顶");
        let vp2 = vp_bounds(cx);
        let right = f32::from(vp2.origin.x + vp2.size.width);
        let vtop = f32::from(vp2.origin.y);
        // 滚动条列 = 右缘内 16px 轨道（WIDTH=4*2+8），thumb 中心 ≈ 右缘−6；
        // 顶部内容时 thumb 起于 +inset4 → +6 在 thumb 上（扫掠实测定点）。
        drag(cx, right - 6.0, vtop + 6.0, 200.0);
        let (_, t2b) = top(cx);
        assert!(t2b.item_ix > 0, "超视口拖动改变视口: {:?}", t2b.item_ix);

        // 3) 浮层遮罩下：同拖无效，浮层保持打开。
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state.app.overlay = Some(crate::app::overlay::Overlay::Info {
                    title: "S".into(),
                    lines: vec!["x".into()],
                    loading: false,
                    seq: 1,
                });
            });
        });
        render(cx);
        let (_, t3) = top(cx);
        drag(
            cx,
            f32::from(vp2.origin.x + vp2.size.width) - 6.0,
            f32::from(vp2.origin.y) + 6.0,
            200.0,
        );
        let (_, t3b) = top(cx);
        assert_eq!(t3, t3b, "浮层下滚动条拖动无效");
        assert!(
            cx.update(|cx| term.read(cx).state.app.overlay.is_some()),
            "浮层保持打开"
        );
    }

    /// mode 切换后渲染帧立即用新调色板（task2.3 spike）。
    ///
    /// 功能: 固定 Light 出帧断言白底 → 切 Dark 应用 → 出帧断言深底；
    ///       （kit `Theme::edit` 自带 `cx.refresh_windows()`——真实窗口由
    ///       其驱动自动出帧，测试宿主显式出帧做一帧断言，无需
    ///       observe_global 兜底）；
    /// 输入: 开窗测试会话；
    /// 输出: 断言通过（两帧 `chat_bg` 探针分别为浅/深表值）。
    #[gpui_kit::test]
    fn mode_switch_repaints_frame_with_new_palette(cx: &mut TestAppContext) {
        use crate::gpui_agent::theme::{ThemeSetting, ThemeState, apply};
        let (handle, _term) = open_focused(cx, "mode-switch-repaint");
        // 固定 Light：一帧断言浅底。
        cx.update(|cx| {
            ThemeState::set(cx, ThemeSetting::Light);
            apply(cx);
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render light");
        assert_eq!(super::probe_chat_bg(), 0xff_ffff, "浅帧白底");
        // 切 Dark：应用后一帧断言深底。
        cx.update(|cx| {
            ThemeState::set(cx, ThemeSetting::Dark);
            apply(cx);
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render dark");
        assert_eq!(super::probe_chat_bg(), 0x1e_1e_1e, "深帧深底");
    }

    /// /settings Theme 行三态循环（task3.1 e2e）：键盘可达、循环生效、不落盘。
    ///
    /// 功能: 真实 slash 通路开 /settings（surface=Gpui → theme_row）→
    ///       Down×3 到 Theme 行（游标 3）→ Enter ×3 循环
    ///       system→light→dark→system（泵应用，帧色探针随动）→
    ///       settings.json 字节全程不变（A3）；
    /// 输入: 开窗测试会话 + 临时 settings_path（防写真实文件）；
    /// 输出: 断言通过（三态顺序、mode/帧色、文件字节、浮层保持）。
    #[gpui_kit::test]
    fn settings_theme_row_cycles_three_modes_without_persistence(
        cx: &mut TestAppContext,
    ) {
        use crate::app::overlay::Overlay;
        use crate::gpui_agent::theme::{ThemeSetting, ThemeState};
        use gpui_kit::component::theme::{Theme, ThemeMode};
        let (handle, term) = open_focused(cx, "settings-theme-row");
        // 临时 settings 文件：字节比对防落盘 + 防写真实文件。
        let dir = std::env::temp_dir().join(format!("gpui-theme-row-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, r#"{"theme":"system"}"#).unwrap();
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state.app.override_settings_path(path.clone());
            })
        });
        let before = std::fs::read(&path).unwrap();

        // 真实 slash 通路开 /settings（完成菜单 Enter 提交）。
        cx.update_window(handle, |_, window, cx| {
            window.input("/settings", cx);
            window.press("enter", cx);
        })
        .expect("submit /settings");
        cx.run_until_parked();
        // 泵 get_state 回填（退 loading）：RPC 线程真实时序，轮询拉取。
        let mut loaded = false;
        for _ in 0..100 {
            cx.update(|cx| term.update(cx, |term, cx| term.pump_ui_events(cx)));
            let loading = cx.update(|cx| match &term.read(cx).state.app.overlay {
                Some(Overlay::Settings { loading, .. }) => *loading,
                _ => true,
            });
            if !loading {
                loaded = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(loaded, "get_state 回填、退 loading");

        // Down×3 → Theme 行（surface 门控可达性）。
        for _ in 0..3 {
            cx.update_window(handle, |_, window, cx| window.press("down", cx))
                .expect("down");
        }
        let cursor = cx.update(|cx| match &term.read(cx).state.app.overlay {
            Some(Overlay::Settings { cursor, loading, .. }) => {
                assert!(!*loading, "get_state 已回填、退 loading");
                *cursor
            }
            other => panic!("应为 Settings 浮层: {other:?}"),
        });
        assert_eq!(cursor, 3, "GPUI 表面可达 Theme 行");

        // Enter ×3：system→light→dark→system（逐轮断言 setting/mode/帧色）。
        let rounds: [(ThemeSetting, Option<ThemeMode>, Option<u32>); 3] = [
            (ThemeSetting::Light, Some(ThemeMode::Light), Some(0xff_ffff)),
            (ThemeSetting::Dark, Some(ThemeMode::Dark), Some(0x1e_1e_1e)),
            (ThemeSetting::System, None, None), // mode 随 OS 外观，不断言
        ];
        for (want_setting, want_mode, want_bg) in rounds {
            cx.update_window(handle, |_, window, cx| window.press("enter", cx))
                .expect("enter");
            cx.run_until_parked();
            cx.update(|cx| term.update(cx, |term, cx| term.pump_ui_events(cx)));
            cx.update_window(handle, |_, window, cx| window.render_frame(cx))
                .expect("render");
            let got = cx.update(|cx| (ThemeState::get(cx).setting, Theme::global(cx).mode));
            assert_eq!(got.0, want_setting, "循环到 {want_setting:?}");
            if let Some(m) = want_mode {
                assert_eq!(got.1, m, "kit mode 随设置");
            }
            if let Some(bg) = want_bg {
                assert_eq!(super::probe_chat_bg(), bg, "帧色探针随表");
            }
        }

        // A3：文件字节全程不变；浮层保持（Toggle 语义）。
        let after = std::fs::read(&path).unwrap();
        assert_eq!(before, after, "不写 settings.json");
        assert!(
            cx.update(|cx| term.read(cx).state.app.overlay.is_some()),
            "浮层保持打开"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 选择热生效：切 dark 后消息区底/输入边框/工具卡三面取深色表（task3.2）。
    ///
    /// 功能: tool_call fixture 开窗 → 固定 Light 渲染断言三面浅色 → 切
    ///       Dark 应用 → 渲染断言三面深色（cycle→Theme::change→refresh
    ///       →render 同帧链路的色彩面）；
    /// 输入: 开窗测试会话 + 三支帧色探针；
    /// 输出: 断言通过（chat/边框/卡底 三面浅深两套值）。
    #[gpui_kit::test]
    fn switching_to_dark_repaints_panels_border_and_card(cx: &mut TestAppContext) {
        use crate::gpui_agent::theme::{ThemeSetting, ThemeState, apply};
        let (handle, term) = open_focused(cx, "tool_call");
        cx.update(|cx| term.update(cx, |term, _| feed_fixture(term, "tool_call")));
        let probes = || {
            (
                super::probe_chat_bg(),
                super::probe_input_border(),
                super::probe_card_bg(),
            )
        };
        // 固定 Light：白底 / 纯黑边 / 浅成功卡（pi light 同源）。
        cx.update(|cx| {
            ThemeState::set(cx, ThemeSetting::Light);
            apply(cx);
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render light");
        assert_eq!(
            probes(),
            (0xff_ffff, 0x00_00_00, 0xde_e9_e1),
            "浅色三面"
        );
        // 切 Dark：深底 / pi dark 边框 / 深成功卡（pi dark 同源）。
        cx.update(|cx| {
            ThemeState::set(cx, ThemeSetting::Dark);
            apply(cx);
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render dark");
        assert_eq!(
            probes(),
            (0x1e_1e_1e, 0x5f_a8_cc, 0x25_41_31),
            "深色三面"
        );
    }

    /// 聚焦加粗规则不变（task4.1）：常态 1px、聚焦 2px，两模式同规。
    ///
    /// 功能: 真值表断言 + render 消费同一真源（`input_border_width`）；
    /// 输入: 无（纯函数）；
    /// 输出: 断言通过。
    #[test]
    fn input_border_width_rule_unchanged() {
        assert_eq!(super::input_border_width(false), 1, "常态 1px");
        assert_eq!(super::input_border_width(true), 2, "聚焦加粗 2px");
    }

    /// GPUI Up/Down 历史分派（task4.1；spec: 输入历史浏览）：空输入直入、
    /// 翻过最新回空草稿、首按行中回行首次按进历史、多行非首行原生、
    /// 末行非浏览到行尾。
    ///
    /// 功能: 注入历史 → 分相位按 Up/Down，断言 Textarea 值/光标与浏览态；
    /// 输入: 开窗测试会话（输入区已聚焦）；
    /// 输出: 断言通过（分派条件内共享、条件外原生）。
    #[gpui_kit::test]
    fn history_keys_route_to_shared_and_back(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "history-route");
        cx.update(|cx| {
            term.update(cx, |t, _| {
                t.state.app.editor.push_history("hist msg");
            })
        });
        let value = |cx: &mut TestAppContext| {
            cx.update(|cx| term.read(cx).textarea.read(cx).value().to_string())
        };
        let cursor = |cx: &mut TestAppContext| {
            cx.update(|cx| {
                let p = term.read(cx).textarea.read(cx).cursor_position();
                (p.line as usize, p.character as usize)
            })
        };
        let browsing = |cx: &mut TestAppContext| {
            cx.update(|cx| term.read(cx).state.app.editor.history_browsing())
        };
        // 重置编辑器内容并回写 Textarea（复用语义回写桥）。
        let reset = |cx: &mut TestAppContext, text: &str| {
            cx.update(|cx| {
                term.update(cx, |t, cx| {
                    t.state.app.editor.replace(text);
                    t.state.app.editor.exit_history_browsing();
                    t.push_editor_via_active_window(cx);
                })
            });
            cx.run_until_parked();
        };
        let press = |cx: &mut TestAppContext, key: &str| {
            cx.update_window(handle, |_, window, cx| window.press(key, cx))
                .unwrap_or_else(|e| panic!("press {key}: {e}"));
            cx.run_until_parked();
        };

        // 相位 1：空输入直入最新 → 翻过最新回空草稿。
        reset(cx, "");
        press(cx, "up");
        assert_eq!(value(cx), "hist msg", "空输入直入历史");
        assert_eq!(cursor(cx), (0, 0), "载入落行首");
        assert!(browsing(cx), "进入浏览态");
        press(cx, "down");
        assert_eq!(value(cx), "", "翻过最新回空草稿");
        assert!(!browsing(cx), "回草稿态");

        // 相位 2：行中首按回行首（不进历史）、次按进历史。
        reset(cx, "draft");
        assert_eq!(cursor(cx), (0, 5), "重置后光标行尾");
        press(cx, "up");
        assert_eq!(cursor(cx), (0, 0), "首按回行首");
        assert_eq!(value(cx), "draft", "首按不载入历史");
        assert!(!browsing(cx), "首按不进浏览");
        press(cx, "up");
        assert_eq!(value(cx), "hist msg", "次按进历史");
        assert!(browsing(cx));

        // 相位 3：多行——非首行 Up 原生、非末行/末行按下表分派。
        reset(cx, "l1\nl2"); // 光标在末行尾 (1,2)
        press(cx, "up");
        assert_eq!(value(cx), "l1\nl2", "非首行 Up 不动历史");
        assert_eq!(cursor(cx).0, 0, "非首行 Up 原生上移");
        assert!(!browsing(cx), "原生移动不进浏览");
        press(cx, "down"); // 回 (1,2)（原生下移回末行）
        assert_eq!(cursor(cx), (1, 2), "原生下移回末行");
        press(cx, "down"); // 末行非浏览 → 共享行尾（pi 同源，值不变）
        assert_eq!(value(cx), "l1\nl2", "末行 Down 不改文本");
        assert_eq!(cursor(cx), (1, 2), "末行非浏览 Down 落行尾");
        assert!(!browsing(cx));
    }

    /// GPUI 内容变更退出浏览、程序化回写不退（task4.2；spec: 内容变更退出）。
    ///
    /// 功能: 空输入 Up 进浏览 → esc（ToApp 回写，文本相等）不断 →
    ///       键入一字符（文本偏离条目）退出浏览且文本留驻；
    /// 输入: 开窗测试会话；
    /// 输出: 断言通过。
    #[gpui_kit::test]
    fn typing_exits_browsing_but_programmatic_push_does_not(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "history-change-exit");
        cx.update(|cx| {
            term.update(cx, |t, _| {
                t.state.app.editor.push_history("hist msg");
            })
        });
        let value = |cx: &mut TestAppContext| {
            cx.update(|cx| term.read(cx).textarea.read(cx).value().to_string())
        };
        let browsing = |cx: &mut TestAppContext| {
            cx.update(|cx| term.read(cx).state.app.editor.history_browsing())
        };
        // 空输入 Up → 浏览
        cx.update_window(handle, |_, window, cx| window.press("up", cx)).expect("up");
        cx.run_until_parked();
        assert!(browsing(cx), "进入浏览");
        assert_eq!(value(cx), "hist msg");

        // 程序化回写（esc → ToApp → sync+push，文本与条目相等）→ 不退出
        cx.update_window(handle, |_, window, cx| window.press("escape", cx)).expect("esc");
        cx.run_until_parked();
        assert!(browsing(cx), "程序化回写不退出浏览");
        assert_eq!(value(cx), "hist msg");

        // 用户键入（文本偏离条目）→ 退出浏览、文本就地留驻
        cx.update_window(handle, |_, window, cx| window.input("x", cx)).expect("input");
        cx.run_until_parked();
        assert!(!browsing(cx), "键入退出浏览");
        assert_eq!(value(cx), "xhist msg", "浏览文本就地留驻（光标行首插字）");
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
                    theme_row: true,
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
                    theme_row: true,
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
                vec![
                    "You:",
                    "Use the read tool",
                    "● read",
                    // 头行参数（head_arg）+ 计时（timer_text）+ 折叠提示行
                    "Cargo.toml",
                    "Took",
                    "ctrl+o to expand",
                    "[package]",
                ],
            ),
            (
                // 错误/重试提示面：失败工具卡（· error）+ markdown 错误说明
                //（含代码块，顺带覆盖高亮路径）。
                "failed_tool",
                vec![
                    "You:",
                    "· error",
                    "definitely-does-not-exist-xyz.txt",
                    "failed with an ENOENT",
                ],
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

    /// 收集消息区文本快照拼串（path 含 gpui-messages 的 test_text 记录）。
    ///
    /// 输入: cx — 测试上下文；handle — 窗口句柄；
    /// 输出: 最近一帧消息区各行文本按绘制序拼接的字符串（task 2.2 断言用）。
    fn msg_texts(cx: &mut TestAppContext, handle: gpui_kit::AnyWindowHandle) -> String {
        let texts = cx
            .update_window(handle, |_, window, _| {
                gpui_kit::base::test_support::texts(window)
            })
            .expect("texts");
        texts
            .iter()
            .filter(|t| t.path().contains(&ElementId::Name("gpui-messages".into())))
            .map(|t| t.text().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 单卡点击切换（task 2.2）：完成态卡点击翻转共享覆盖表、再点翻回；
    /// 展开面经内建 Collapsible 挂载（提示行/全文行随面互换）。
    ///
    /// 功能: 灌 tool_call fixture → 折叠面断言 → 点击 → 展开面断言 →
    ///       再点回折叠面断言（生效值读共享覆盖表）；
    /// 输入: cx — 测试上下文；
    /// 输出: 断言通过（覆盖表翻转 + 卡片面随帧重渲染）。
    #[gpui_kit::test]
    fn tool_card_click_toggles_single_card(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "tool_call");
        cx.update(|cx| {
            term.update(cx, |term, _| feed_fixture(term, "tool_call"));
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();

        let tool_id = "call_x96eunvxvt479n9ys4r29ozo";
        let card_id = format!("tool-card-{tool_id}");
        let hint = "more lines, ctrl+o to expand";
        let tail = "path = \"src/bin/probe/main.rs\"";
        let effective = |cx: &mut TestAppContext| {
            cx.update(|cx| {
                term.update(cx, |term, _| term.state.app.tool_expanded_effective(tool_id))
            })
        };

        // 折叠面：提示行在场、被折叠的行不在场，生效值 = 全局折叠。
        let msg = msg_texts(cx, handle);
        assert!(msg.contains(hint), "折叠态应含提示行: {msg:?}");
        assert!(!msg.contains(tail), "折叠态不应含被折叠行: {msg:?}");
        assert!(!effective(cx), "初始为全局折叠");

        // 点击 → 展开面（提示行去、全文行在），覆盖表翻转为展开。
        cx.update_window(handle, |_, window, cx| window.click(card_id.clone(), cx))
            .expect("click");
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        let msg = msg_texts(cx, handle);
        assert!(msg.contains(tail), "展开态应含全文行: {msg:?}");
        assert!(!msg.contains(hint), "展开态不应含提示行: {msg:?}");
        assert!(effective(cx), "点击后单卡展开");

        // 再点 → 回到折叠面，覆盖表翻回折叠。
        cx.update_window(handle, |_, window, cx| window.click(card_id.clone(), cx))
            .expect("click");
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        let msg = msg_texts(cx, handle);
        assert!(msg.contains(hint), "再点回折叠面: {msg:?}");
        assert!(!msg.contains(tail), "再点回折叠面: {msg:?}");
        assert!(!effective(cx), "再点回全局折叠");
    }

    /// 运行中卡点击无动作（task 2.2：仅 `result.is_some()` 卡响应点击，
    /// 与 TUI 命中门控同源，design D2/D4）。
    ///
    /// 功能: 注入 toolcall + tool_execution_start（无 end，卡留在流式尾块）
    ///       → 点击卡 → 断言共享覆盖表未翻转；
    /// 输入: cx — 测试上下文；
    /// 输出: 断言通过（点击被门控拦截，不写覆盖表）。
    #[gpui_kit::test]
    fn running_tool_card_click_is_noop(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "running-card");
        cx.update(|cx| {
            term.update(cx, |term, _| {
                use serde_json::json;
                let steps = [
                    json!({"type":"message_start","message":{"role":"assistant","content":[]}}),
                    json!({"type":"message_update","assistantMessageEvent":{"type":"toolcall_start","contentIndex":0,"id":"run1","toolName":"bash"}}),
                    json!({"type":"message_update","assistantMessageEvent":{"type":"toolcall_delta","contentIndex":0,"delta":"{\"command\":\"sleep 999\"}"}}),
                    json!({"type":"tool_execution_start","toolCallId":"run1","toolName":"bash","args":{"command":"sleep 999"}}),
                ];
                for raw in steps {
                    term.state.reduce(&ClientEvent::Event(raw), &term.commander, &term.ui_tx);
                }
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();

        // 无最终结果的卡（流式尾块）：点击应被门控拦截。
        cx.update_window(handle, |_, window, cx| window.click("tool-card-run1", cx))
            .expect("click");
        cx.run_until_parked();

        let effective = cx.update(|cx| {
            term.update(cx, |term, _| term.state.app.tool_expanded_effective("run1"))
        });
        assert!(!effective, "运行中卡点击不应翻转覆盖表");
    }

    /// 单卡隔离与全局开关（task 2.2 验收）：双卡 fixture 点 A 只翻 A；
    /// 全局 ctrl+o 翻转后覆盖清空、无覆盖卡跟随新全局值。
    ///
    /// 功能: 灌 tool_edit fixture（write + edit 两卡）→ 断言双卡随全局折叠
    ///       → 点卡 A → 断言仅 A 展开 → `handle_key(ctrl+o)` 全局展开 →
    ///       断言覆盖清空且双卡跟随 → 再 ctrl+o → 双卡回折叠；
    /// 输入: cx — 测试上下文；
    /// 输出: 断言通过（覆盖表隔离 + 全局开关消费，渲染面由单卡测试盖）。
    #[gpui_kit::test]
    fn tool_card_global_toggle_and_per_card_isolation(cx: &mut TestAppContext) {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (handle, term) = open_focused(cx, "tool_edit");
        cx.update(|cx| {
            term.update(cx, |term, _| feed_fixture(term, "tool_edit"));
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();

        let card_a = "call_a508d15ac76543b8b42e6cd7"; // write 卡
        let card_b = "call_8faa3ab5630c49d5be3e1cdc"; // edit 卡
        let effective = |cx: &mut TestAppContext, id: &str| {
            cx.update(|cx| term.update(cx, |term, _| term.state.app.tool_expanded_effective(id)))
        };
        let ctrl_o = || KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL);

        // 初始：双卡均随全局折叠。
        assert!(!effective(cx, card_a), "A 初始折叠");
        assert!(!effective(cx, card_b), "B 初始折叠");

        // 点 A：只翻 A，B 不受影响（单卡隔离，design D1）。
        cx.update_window(handle, |_, window, cx| {
            window.click(format!("tool-card-{card_a}"), cx)
        })
        .expect("click A");
        cx.run_until_parked();
        assert!(effective(cx, card_a), "点 A 后 A 展开");
        assert!(!effective(cx, card_b), "点 A 后 B 不受影响");

        // 全局 ctrl+o：覆盖清空、双卡跟随新全局值（展开）。
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state
                    .app
                    .handle_key(ctrl_o(), &term.commander, &term.ui_tx)
            })
        });
        assert!(effective(cx, card_a), "全局展开后 A 跟随");
        assert!(effective(cx, card_b), "全局展开后 B 跟随");

        // 再 ctrl+o：全局回折叠，双卡跟随。
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state
                    .app
                    .handle_key(ctrl_o(), &term.commander, &term.ui_tx)
            })
        });
        assert!(!effective(cx, card_a), "全局回折叠后 A 跟随");
        assert!(!effective(cx, card_b), "全局回折叠后 B 跟随");
    }

    /// thinking 块语义对齐（task 2.3）：空块不占行、非空块折叠为占位、
    /// 单块点击仅翻该块（不碰全局与它块；seq 用共享 thinking_seqs 规则）。
    ///
    /// 功能: 注入 assistant 流式尾块（空 thinking + 两非空 thinking 夹
    ///       text）→ 折叠面断言（双占位、无空块占位）→ 点 `thinking-1`
    ///       → 断言仅块 0 展开、块 1 仍占位、全局未翻 → 再点回折叠；
    /// 输入: cx — 测试上下文；
    /// 输出: 断言通过（共享 seq/占位/覆盖语义）。
    #[gpui_kit::test]
    fn thinking_block_placeholder_and_isolated_click(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "thinking-blocks");
        cx.update(|cx| {
            term.update(cx, |term, _| {
                use serde_json::json;
                let steps = [
                    json!({"type":"message_start","message":{"role":"assistant","content":[]}}),
                    // 空 thinking（start 无 delta）→ 不占行，但占 seq 0。
                    json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_start","contentIndex":0}}),
                    // 块 0：非空（seq 1）。
                    json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_start","contentIndex":1}}),
                    json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","contentIndex":1,"delta":"first-thought"}}),
                    // 中间 text（不占 seq）。
                    json!({"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":2}}),
                    json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":2,"delta":"answer"}}),
                    // 块 1：非空（seq 2）。
                    json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_start","contentIndex":3}}),
                    json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","contentIndex":3,"delta":"second-thought"}}),
                ];
                for raw in steps {
                    term.state.reduce(&ClientEvent::Event(raw), &term.commander, &term.ui_tx);
                }
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();

        // 折叠面：两块占位；空块不占行（无 <thinking 0 chars>）、正文不显。
        let msg = msg_texts(cx, handle);
        assert!(msg.contains("<thinking 13 chars>"), "块0 占位: {msg:?}");
        assert!(msg.contains("<thinking 14 chars>"), "块1 占位: {msg:?}");
        assert!(!msg.contains("<thinking 0 chars>"), "空块不占行: {msg:?}");
        assert!(!msg.contains("first-thought"), "折叠态不显正文: {msg:?}");

        // 点块 0（seq=1）→ 仅块 0 展开；块 1 占位仍在即证全局未翻（OR 规则）。
        cx.update_window(handle, |_, window, cx| window.click(String::from("thinking-1"), cx))
            .expect("click");
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        let msg = msg_texts(cx, handle);
        assert!(msg.contains("first-thought"), "点后块0 展开: {msg:?}");
        assert!(!msg.contains("<thinking 13 chars>"), "点后块0 占位消失: {msg:?}");
        assert!(msg.contains("<thinking 14 chars>"), "块1 不受影响: {msg:?}");
        // 无覆盖序号的生效值 = 全局 show_thinking（直读全局未被翻转）。
        let global = cx.update(|cx| {
            term.update(cx, |term, _| term.state.app.thinking_expanded_effective(999))
        });
        assert!(!global, "单块点击不应翻转全局 show_thinking");

        // 再点 → 块 0 回占位。
        cx.update_window(handle, |_, window, cx| window.click(String::from("thinking-1"), cx))
            .expect("click");
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        let msg = msg_texts(cx, handle);
        assert!(msg.contains("<thinking 13 chars>"), "再点回占位: {msg:?}");
        assert!(!msg.contains("first-thought"), "再点回占位: {msg:?}");
    }

    /// edit 卡 diff 文本 runs（task 2.4）：details.diff 字段与 args old/new
    /// LCS 降级两路径正文都入消息区（颜色映射由 seg_visual 单测断言）。
    ///
    /// 功能: 灌 tool_edit fixture（diff 字段 -/+ 行）→ 同窗追加 ed1
    ///       （diff 含 @@ 行）与 ed2（无 details → old/new 降级）→
    ///       逐 needle 断言；
    /// 输入: cx — 测试上下文；
    /// 输出: 断言通过（两路径正文均渲染）。
    #[gpui_kit::test]
    fn edit_card_diff_field_and_fallback_paths(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "tool_edit");
        cx.update(|cx| {
            term.update(cx, |term, _| feed_fixture(term, "tool_edit"));
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();

        // 路径1：fixture 的 details.diff（- 红 / + 绿行）。
        let msg = msg_texts(cx, handle);
        assert!(msg.contains("-1 FIXTURE-MARKER"), "diff 字段 - 行: {msg:?}");
        assert!(msg.contains("+1 EDITED-MARKER"), "diff 字段 + 行: {msg:?}");

        // 追加：@@ 青行卡（diff 字段）+ 无 details 的 old/new 降级卡。
        cx.update(|cx| {
            term.update(cx, |term, _| {
                use serde_json::json;
                let steps = [
                    json!({"type":"message_start","message":{"role":"assistant","content":[]}}),
                    json!({"type":"message_update","assistantMessageEvent":{"type":"toolcall_start","contentIndex":0,"id":"ed1","toolName":"edit"}}),
                    json!({"type":"message_update","assistantMessageEvent":{"type":"toolcall_delta","contentIndex":0,"delta":"{\"path\":\"x.txt\",\"oldString\":\"a\",\"newString\":\"b\"}"}}),
                    json!({"type":"tool_execution_start","toolCallId":"ed1","toolName":"edit","args":{"path":"x.txt"}}),
                    json!({"type":"tool_execution_end","toolCallId":"ed1","toolName":"edit","isError":false,"durationMs":4,
                        "result":{"content":[{"type":"text","text":"ok"}],"details":{"diff":"@@ -1,2 +1,2 @@\n-removed-line\n+added-line"}}}),
                    json!({"type":"message_update","assistantMessageEvent":{"type":"toolcall_start","contentIndex":1,"id":"ed2","toolName":"edit"}}),
                    json!({"type":"message_update","assistantMessageEvent":{"type":"toolcall_delta","contentIndex":1,"delta":"{\"path\":\"y.txt\",\"oldString\":\"old-fallback\",\"newString\":\"new-fallback\"}"}}),
                    json!({"type":"tool_execution_start","toolCallId":"ed2","toolName":"edit","args":{"path":"y.txt"}}),
                    json!({"type":"tool_execution_end","toolCallId":"ed2","toolName":"edit","isError":false,"durationMs":3,
                        "result":{"content":[{"type":"text","text":"ok"}]}}),
                ];
                for raw in steps {
                    term.state.reduce(&ClientEvent::Event(raw), &term.commander, &term.ui_tx);
                }
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();

        let msg = msg_texts(cx, handle);
        // @@ 青行（文本在场；颜色映射由 seg_visual 单测断言）。
        assert!(msg.contains("@@ -1,2 +1,2 @@"), "@@ 行: {msg:?}");
        assert!(msg.contains("-removed-line"), "diff - 行: {msg:?}");
        assert!(msg.contains("+added-line"), "diff + 行: {msg:?}");
        // 降级路径：无 details.diff → args old/new LCS 行。
        assert!(msg.contains("-old-fallback"), "降级 - 行: {msg:?}");
        assert!(msg.contains("+new-fallback"), "降级 + 行: {msg:?}");
    }

    /// Fg 段 → 卡片提亮色（task 2.4 + 1.2 双表）：diff 红/绿/青按 mode 取表。
    ///
    /// 输入: [`SegRole::Fg`] 携带的三种 diff 前景色 + 浅色表；
    /// 输出: 断言通过（`seg_visual` 首项 = 浅色表 pi 同源提亮色）。
    #[test]
    fn seg_visual_fg_diff_colors_map_on_card() {
        use super::{Hsla, SegRole, palette, seg_visual};
        let pal = palette::for_dark(false);
        let (red, _, _) = seg_visual(SegRole::Fg(Color::Red), pal);
        let (green, _, _) = seg_visual(SegRole::Fg(Color::Green), pal);
        let (cyan, _, _) = seg_visual(SegRole::Fg(Color::Cyan), pal);
        let rgb = |h: Hsla| {
            let c: gpui_kit::Rgba = h.into();
            (
                (c.r * 255.0).round() as u8,
                (c.g * 255.0).round() as u8,
                (c.b * 255.0).round() as u8,
            )
        };
        assert_eq!(rgb(red), (0xc8, 0x25, 0x3d), "diff - 红");
        assert_eq!(rgb(green), (0x33, 0x7e, 0x58), "diff + 绿");
        assert_eq!(rgb(cyan), (0x00, 0x97, 0xa7), "@@ 青");
    }

    /// 工具卡面板底随 mode 三态取表（task1.3 接线验证，无 TTY）。
    ///
    /// 输入: Running/Ended(成功)/Ended(失败) × 浅深两表；
    /// 输出: 断言通过（三态分别命中各表对应值）。
    #[test]
    fn card_panel_bg_follows_mode_three_states() {
        use super::{ToolStatus, card_panel_bg, palette};
        let light = palette::for_dark(false);
        let dark = palette::for_dark(true);
        let rgb = |c: gpui_kit::Rgba| {
            (
                (c.r * 255.0).round() as u8,
                (c.g * 255.0).round() as u8,
                (c.b * 255.0).round() as u8,
            )
        };
        assert_eq!(
            rgb(card_panel_bg(&ToolStatus::Running, false, light)),
            (0xe4, 0xe5, 0xe6),
            "浅 Running"
        );
        assert_eq!(
            rgb(card_panel_bg(&ToolStatus::Ended, false, light)),
            (0xde, 0xe9, 0xe1),
            "浅 成功"
        );
        assert_eq!(
            rgb(card_panel_bg(&ToolStatus::Ended, true, light)),
            (0xee, 0xe2, 0xe1),
            "浅 失败"
        );
        assert_eq!(
            rgb(card_panel_bg(&ToolStatus::Running, false, dark)),
            (0x34, 0x38, 0x3a),
            "深 Running"
        );
        assert_eq!(
            rgb(card_panel_bg(&ToolStatus::Ended, false, dark)),
            (0x25, 0x41, 0x31),
            "深 成功"
        );
        assert_eq!(
            rgb(card_panel_bg(&ToolStatus::Ended, true, dark)),
            (0x5b, 0x28, 0x2a),
            "深 失败"
        );
    }

    /// 通知行消费共享 `notice_model`（task 2.5）：`[kind]` 前缀并入文本，
    /// 色/粗/斜由段角色映射（error 红粗、retry 黄、其它暗灰斜体）。
    ///
    /// 功能: 注入 error/retry 两条通知条目 → 断言 `[error]`/`[retry]`
    ///       前缀 needle 均在场；
    /// 输入: cx — 测试上下文；
    /// 输出: 断言通过（两 kind 均以共享模型前缀渲染）。
    #[gpui_kit::test]
    fn notice_row_uses_shared_model_prefixes(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "notice-rows");
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state
                    .app
                    .state
                    .push_notice("error", "boom happened".to_string());
                term.state
                    .app
                    .state
                    .push_notice("retry", "retrying now".to_string());
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();

        let msg = msg_texts(cx, handle);
        assert!(msg.contains("[error] boom happened"), "error 前缀: {msg:?}");
        assert!(msg.contains("[retry] retrying now"), "retry 前缀: {msg:?}");
    }

    /// bash 条目消费共享 `bash_entry_model`（task 3.1）：尾部 500 行截断
    /// + 截断标记 + exit 状态行 + 命令行（与 TUI append_bash 同源）。
    ///
    /// 功能: start_bash + finish_bash（600 行输出、exit 0）→ 断言命令行、
    ///       `…（已截断 101 行）`、尾行 row599 在场且 row100 已丢、
    ///       `exit 0` 状态行在场；
    /// 输入: cx — 测试上下文；
    /// 输出: 断言通过（模型语义经 segs_row 呈现）。
    #[gpui_kit::test]
    fn bash_row_uses_shared_model_tail_cap_and_status(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "bash-entry");
        cx.update(|cx| {
            term.update(cx, |term, _| {
                use serde_json::json;
                let big: String = (0..600).map(|i| format!("row{i}\n")).collect();
                let state = &mut term.state.app.state;
                state.start_bash("big-command", false);
                state.finish_bash(&json!({"output": big, "exitCode": 0}));
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();

        let msg = msg_texts(cx, handle);
        assert!(msg.contains("$ big-command"), "命令行: {msg:?}");
        assert!(msg.contains("…（已截断 101 行）"), "截断标记: {msg:?}");
        assert!(msg.contains("row101"), "首个保留行: {msg:?}");
        assert!(msg.contains("row599"), "末行保留: {msg:?}");
        assert!(!msg.contains("row100"), "头部行已丢: {msg:?}");
        assert!(msg.contains("exit 0"), "exit 状态行: {msg:?}");
    }

    /// 运行中 bash 尾槽（5.3 冒烟 B2 发现的缺口；spec: bash 条目语义
    /// 对齐——运行中 `⟳ 运行中…（esc 中止）` MUST 可见）：start_bash 后
    /// 帧含命令行与运行状态行、行数对账含尾槽；输出分片后新行入帧；
    /// finish_bash 落条目 → 运行状态行消失、`exit 0` 在场、行数收敛。
    ///
    /// 功能: 注入 start_bash + bash_execution_update → 渲染断言运行态；
    ///       finish_bash → 断言收敛为完成条目；
    /// 输入: cx — 测试上下文；
    /// 输出: 断言通过（尾槽渲染、增长与收敛）。
    #[gpui_kit::test]
    fn live_bash_slot_renders_and_lands(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "live-bash");
        // 运行开始 + 首个输出分片。
        cx.update(|cx| {
            term.update(cx, |term, _| {
                use serde_json::json;
                let state = &mut term.state.app.state;
                state.start_bash("seq 1 3", false);
                state.handle(&json!({"type":"bash_execution_update","delta":"line-one\n"}));
            });
        });
        assert_eq!(
            cx.update(|cx| term.read(cx).scroller_rows()),
            1,
            "空会话 + 尾槽 = 1 项"
        );
        let render = |cx: &mut TestAppContext| {
            cx.update_window(handle, |_, window, cx| window.render_frame(cx))
                .expect("render");
            cx.run_until_parked();
            msg_texts(cx, handle)
        };
        let msg = render(cx);
        assert!(msg.contains("$ seq 1 3"), "命令行在场: {msg:?}");
        assert!(msg.contains("⟳ 运行中"), "运行状态行在场: {msg:?}");
        assert!(msg.contains("line-one"), "首行输出在场: {msg:?}");

        // 输出分片增长 → 帧内出现新行（TUI 实时增长同语义）。
        cx.update(|cx| {
            term.update(cx, |term, _| {
                use serde_json::json;
                term.state
                    .app
                    .state
                    .handle(&json!({"type":"bash_execution_update","delta":"line-two\n"}));
            });
        });
        let msg = render(cx);
        assert!(msg.contains("line-two"), "增长输出在场: {msg:?}");

        // 完成 → 尾槽收敛为落盘条目：运行状态行消失、exit 0 在场。
        cx.update(|cx| {
            term.update(cx, |term, _| {
                use serde_json::json;
                term.state.app.state.finish_bash(
                    &json!({"output":"line-one\nline-two\n","exitCode":0}),
                );
            });
        });
        let msg = render(cx);
        assert!(!msg.contains("运行中"), "运行状态行应消失: {msg:?}");
        assert!(msg.contains("exit 0"), "exit 状态行在场: {msg:?}");
        assert_eq!(cx.update(|cx| term.read(cx).scroller_rows()), 1, "落 1 条目、尾槽消失");
        assert_eq!(
            cx.update(|cx| term.read(cx).state.session().entries().len()),
            1,
            "落盘条目数"
        );
    }

    /// 生成带唯一行标记的 user 条目（spike 测量用；行 = `{tag}L{jj}`）。
    ///
    /// 功能: 经 reduce 投递 message_start 落一条 user 条目；
    /// 输入: term — 组件；tag — 条目标识前缀；lines — 正文行数；
    /// 输出: 无返回值；副作用为 transcript 追加条目。
    fn feed_tagged(term: &mut GPUIAgentTerminal, tag: &str, lines: usize) {
        use serde_json::json;
        let body = (0..lines)
            .map(|i| format!("{tag}L{i:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        term.state.reduce(
            &ClientEvent::Event(json!({
                "type":"message_start",
                "message":{"role":"user","content":[{"type":"text","text":body}]}
            })),
            &term.commander,
            &term.ui_tx,
        );
    }

    /// 可见标记文本的窗口 y 坐标（未绘制/不可见 → None）。
    ///
    /// 功能: 从上一帧文本快照中按前缀定位标记；多行条目优先取含 `\n` 的
    ///       逻辑块快照（test_text 容器，跨帧一致），单行回退首个匹配；
    /// 输入: cx/handle — 测试上下文与窗口；prefix — 标记前缀；
    /// 输出: Some(文本原点 y px) / None。
    fn marker_y(
        cx: &mut TestAppContext,
        handle: gpui_kit::AnyWindowHandle,
        prefix: &str,
    ) -> Option<f32> {
        cx.update_window(handle, |_, window, _| {
            let all: Vec<_> = gpui_kit::base::test_support::texts(window)
                .iter()
                .filter(|t| t.visible() && t.text().starts_with(prefix))
                .cloned()
                .collect();
            all.iter()
                .find(|t| t.text().contains('\n'))
                .or_else(|| all.first())
                .map(|t| f32::from(t.bounds().origin.y))
        })
        .ok()
        .flatten()
    }

    /// 条目首行标记（user 条目：首行原文；取前缀避免长行快照差异）。
    ///
    /// 功能: 取条目正文首行作为 marker 前缀；
    /// 输入: term — 组件（只读）；ix — 条目下标；
    /// 输出: 首行前 24 字符（不足则全部）。
    fn entry_marker(term: &GPUIAgentTerminal, ix: usize) -> String {
        match &term.state.session().entries()[ix] {
            Entry::User { text } => text
                .split('\n')
                .next()
                .unwrap_or_default()
                .chars()
                .take(24)
                .collect(),
            _ => String::new(),
        }
    }

    /// spike 1.2：`scroll_to_item(ix)` 后视口顶对齐条目顶
    /// （`logical_scroll_top == {ix, 0}`——跳转 spec「视口顶移到消息起始行」
    /// 的能力前提；非顶对齐需停下回看）。
    ///
    /// 功能: 灌内容 → 滚到条目 3 → 断言 ListOffset 顶对齐；
    /// 输入: cx — 测试上下文；
    /// 输出: 断言通过（顶对齐）。
    #[gpui_kit::test]
    fn scroll_to_item_aligns_to_item_top(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "align-top");
        cx.update(|cx| {
            term.update(cx, |term, _| {
                for i in 0..10 {
                    feed_tagged(term, &format!("T{i:02}"), 5);
                }
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();

        cx.update(|cx| {
            term.update(cx, |term, cx| {
                let _ = term.scroller.update(cx, |s, cx| s.scroll_to_item(3, cx));
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();

        let top = cx.update(|cx| {
            let s = term.read(cx).scroller.read(cx);
            s.logical_scroll_top()
        });
        assert_eq!(top.item_ix, 3, "视口顶应落在条目 3");
        assert!(
            f32::from(top.offset_in_item).abs() < 0.5,
            "应顶对齐（offset=0），实测 {:?}",
            top.offset_in_item
        );
    }

    /// spike 1.1：S1 内容估算步进的落点容差（design D2 门槛）。
    ///
    /// 功能: 三段式——①帧@0 标定（文本行高/字符宽/折叠高度/文本宽）；
    ///       ②折行模型校验（WRAP 长行实际行数 vs 估算，±1 行）；
    ///       ③落点门槛：代表内容跨度落点位移距真一页 ≤ 1/3 视口，
    ///       跨 WRAP 跨度测量位移误差（eprintln 供 S1/S2 结论）；
    /// 输入: cx — 测试上下文；
    /// 输出: 断言通过（标定合理 + 模型校验 + 代表内容门槛 + 结构断言）。
    #[gpui_kit::test]
    fn page_step_lands_within_tolerance(cx: &mut TestAppContext) {
        use crate::gpui_agent::scroll::{ScrollMetrics, page_down_target};
        let (handle, term) = open_focused(cx, "page-spike");
        // 内容：A(8×5行) + WRAP(单行400字符) + C(10×10行，标定第二点)。
        cx.update(|cx| {
            term.update(cx, |term, _| {
                for i in 0..8 {
                    feed_tagged(term, &format!("A{i:02}"), 5);
                }
                term.state.reduce(
                    &ClientEvent::Event(serde_json::json!({
                        "type":"message_start",
                        "message":{"role":"user","content":[{"type":"text","text":format!("{}{}", "WRAP-MARKER-", "abcdefghij1234567890".repeat(20))}]}
                    })),
                    &term.commander,
                    &term.ui_tx,
                );
                for i in 0..10 {
                    feed_tagged(term, &format!("C{i:02}"), 10);
                }
                // 容量探针：单 token 70/140/280 字符 + 带空格 140 对照 + 锚点。
                for (tag, n) in [("D1", 70), ("D2", 140), ("D3", 280)] {
                    term.state.reduce(
                        &ClientEvent::Event(serde_json::json!({
                            "type":"message_start",
                            "message":{"role":"user","content":[{"type":"text","text":format!("{}{}{}", tag, "-MARKER-", "x".repeat(n))}]}
                        })),
                        &term.commander,
                        &term.ui_tx,
                    );
                }
                term.state.reduce(
                    &ClientEvent::Event(serde_json::json!({
                        "type":"message_start",
                        "message":{"role":"user","content":[{"type":"text","text":"E1-MARKER- word1 word2 word3 word4 word5 word6 word7 word8 word9 word10 word11 word12 word13 word14 word15 word16 word17 word18 word19 word20"}]}
                    })),
                    &term.commander,
                    &term.ui_tx,
                );
                feed_tagged(term, "ANCHOR", 3);
            });
        });
        let render = |cx: &mut TestAppContext| {
            cx.update_window(handle, |_, window, cx| window.render_frame(cx))
                .expect("render");
            cx.run_until_parked();
        };
        render(cx);

        // 视口宽高与页长（TUI 页 = 视口行数−4 行）。
        let (view_h, vp_w, page_px) = cx
            .update_window(handle, |_, window, _| {
                let snap = gpui_kit::base::test_support::find(
                    window,
                    &[],
                    &ElementId::from("gpui-messages"),
                )
                .expect("消息区观测在场");
                let (_, line_h) = grid_metrics(window);
                let vh = f32::from(snap.bounds().size.height);
                let vw = f32::from(snap.bounds().size.width);
                let rows = (vh / f32::from(line_h)).floor();
                (vh, vw, (rows - 4.0).max(1.0) * f32::from(line_h))
            })
            .expect("viewport");

        let scroll_to = |cx: &mut TestAppContext, ix: usize| {
            cx.update(|cx| {
                term.update(cx, |term, cx| {
                    let _ = term.scroller.update(cx, |s, cx| s.scroll_to_item(ix, cx));
                });
            });
            render(cx);
        };

        // —— ① 帧@0 标定（常数回填 scroll.rs 生产值的依据）——
        scroll_to(cx, 0);
        let ya0 = marker_y(cx, handle, "A00L00").expect("A00 可见");
        let ya1 = marker_y(cx, handle, "A01L00").expect("A01 可见");
        let h_entry = ya1 - ya0;
        let calib = cx
            .update_window(handle, |_, window, _| {
                let vp = gpui_kit::base::test_support::find(
                    window,
                    &[],
                    &ElementId::from("gpui-messages"),
                )
                .expect("视口");
                let vpb = vp.bounds();
                let mut label = (0.0f32, 0.0f32); // (y, h)
                let mut body = (0.0f32, 0.0f32, 0.0f32, 0usize); // (y, h, w, chars)
                let mut painted = (0.0f32, 0usize); // (w, chars) 单行渲染版
                for t in gpui_kit::base::test_support::texts(window) {
                    let s = t.text();
                    if s == "You:" && label.1 == 0.0 {
                        label = (f32::from(t.bounds().origin.y), f32::from(t.bounds().size.height));
                    } else if s.starts_with("A00L00") && s.contains('\n') {
                        body = (
                            f32::from(t.bounds().origin.y),
                            f32::from(t.bounds().size.height),
                            f32::from(t.bounds().size.width),
                            s.chars().count(),
                        );
                    } else if s.starts_with("A00L00") && !s.contains('\n') {
                        painted = (f32::from(t.bounds().size.width), s.chars().count());
                    }
                }
                (label, body, painted, f32::from(vpb.origin.x))
            })
            .expect("calib@0");
        let (label, body, _painted, _vp_x) = calib;
        let text_line = body.1;
        // 推进宽用经验常数（spike 容量定律：760px 下 64 字符，见 CHAR_ADVANCE_PX）。
        let char_w = crate::gpui_agent::scroll::CHAR_ADVANCE_PX;
        let text_w = body.2; // 正文容器宽（w_full = 文本折行宽度）
        let user_folded = h_entry - body.1;
        eprintln!(
            "[spike] ①标定 view=({view_h}x{vp_w}) page={page_px} h_entry={h_entry} label={label:?} body(y,h,w,chars)={body:?} → text_line={text_line} char_w={char_w} text_w={text_w} folded={user_folded}"
        );
        assert!(
            text_line > 8.0 && (6.0..13.0).contains(&char_w) && text_w > 300.0
                && user_folded > 10.0 && label.1 > 5.0,
            "标定合理: text_line={text_line} char_w={char_w} text_w={text_w} folded={user_folded} label={label:?}"
        );
        let metrics = ScrollMetrics {
            grid_line: f32::from(cx
                .update_window(handle, |_, window, _| grid_metrics(window).1)
                .expect("grid")),
            text_line,
            user_folded,
            base_pad: 0.0, // 本测试内容全 user，非 user 路径不参与
            text_w,
            char_w,
        };

        // —— ②b 容量定律校验（帧@19：D1/D2/D3/E1 正文行数 vs 模型，±1 行）——
        scroll_to(cx, 19);
        for (prefix, chars) in [
            ("D1-MARKER", 80.0),
            ("D2-MARKER", 150.0),
            ("D3-MARKER", 290.0),
            ("E1-MARKER", 141.0),
        ] {
            let h = cx
                .update_window(handle, |_, window, _| {
                    gpui_kit::base::test_support::texts(window)
                        .iter()
                        .find(|t| t.visible() && t.text().starts_with(prefix))
                        .map(|t| f32::from(t.bounds().size.height))
                        .expect("探针正文可见")
                })
                .expect("probe19");
            let rows_actual = h / text_line;
            let rows_est = (chars * metrics.char_w / metrics.text_w).ceil().max(1.0);
            eprintln!("[spike] ②b {prefix} h={h} rows_actual={rows_actual} rows_est={rows_est}");
            assert!(
                (rows_est - rows_actual).abs() <= 1.0,
                "{prefix} 容量定律估算 {rows_est} 应贴近实际 {rows_actual}（±1 行）"
            );
        }

        // —— ② 折行模型校验（帧@8：WRAP 实际行数 vs 估算，±1 行）——
        scroll_to(cx, 8);
        let y_wrap = marker_y(cx, handle, "WRAP-MARKER-").expect("WRAP 可见");
        let y_c0 = marker_y(cx, handle, "C00L00").expect("C00 可见");
        let h_wrap = y_c0 - y_wrap;
        let wrap_text = cx.update(|cx| match &term.read(cx).state.session().entries()[8] {
            Entry::User { text } => text.clone(),
            _ => String::new(),
        });
        let rows_actual = (h_wrap - user_folded) / text_line;
        let rows_est = crate::gpui_agent::scroll::wrapped_rows(&wrap_text, &metrics);
        eprintln!(
            "[spike] ②折行 h_wrap={h_wrap} rows_actual={rows_actual} rows_est={rows_est} (chars={})",
            wrap_text.chars().count()
        );
        assert!(
            (rows_est - rows_actual).abs() <= 1.0,
            "折行估算 {rows_est} 应贴近实际 {rows_actual}（±1 行）"
        );

        // —— ③ 落点门槛 ——

        // 跨度①（代表内容，A 区内）：同帧内测 from→落点标记间距 = 真位移。
        scroll_to(cx, 2);
        let y_from = marker_y(cx, handle, "A02L00").expect("A02 可见");
        let t1 = cx.update(|cx| {
            let t = term.read(cx);
            let entries = t.state.session().entries().to_vec();
            page_down_target(&entries, &t.state.app, 2, 0.0, page_px, &metrics)
        });
        let land1 = cx.update(|cx| entry_marker(term.read(cx), t1));
        let y_t1 = marker_y(cx, handle, &land1).expect("落点标记在 from 帧内可见");
        let err1 = ((y_t1 - y_from) - page_px).abs();
        eprintln!("[spike] 跨度① landing={t1} disp={} err={err1}", y_t1 - y_from);
        assert!(
            err1 <= view_h / 3.0,
            "代表内容落点误差 {err1} 应 ≤ 1/3 视口 {}",
            view_h / 3.0
        );

        // 跨度②（跨 WRAP 长行条目）：同帧测量位移误差供 S1/S2 结论。
        scroll_to(cx, 7);
        let y_from2 = marker_y(cx, handle, "A07L00").expect("A07 可见");
        let t2 = cx.update(|cx| {
            let t = term.read(cx);
            let entries = t.state.session().entries().to_vec();
            page_down_target(&entries, &t.state.app, 7, 0.0, page_px, &metrics)
        });
        let land2 = cx.update(|cx| entry_marker(term.read(cx), t2));
        let y_t2 = marker_y(cx, handle, &land2).expect("落点标记在 from 帧内可见");
        let err2 = ((y_t2 - y_from2) - page_px).abs();
        eprintln!(
            "[spike] 跨度② landing={t2} disp={} err={err2} (真一页={page_px})",
            y_t2 - y_from2
        );
        // 结构性断言：向后落点、未飞到末条；误差数据见 eprintln 供任务结论。
        assert!(t2 > 7, "应向后落");
        assert!(t2 < cx.update(|cx| term.read(cx).state.session().entries().len()), "不应停在末条");
    }

    /// 队列条（task 3.2）：消息区与输入区之间，`[steer]`/`[queue]` 前缀
    /// 行随流式排队出现；队列清空后整条消失（空队列不占位）。
    ///
    /// 功能: 注入 queue_update（steering 1 + followUp 2）→ 断言前缀行与
    ///       `… 1 more`（最多 2 行）在 gpui-queue 路径下；再注入空队列
    ///       → 断言 gpui-queue 路径无任何文本；
    /// 输入: cx — 测试上下文；
    /// 输出: 断言通过（复用 view::queue_lines 同源输出）。
    #[gpui_kit::test]
    fn queue_strip_appears_and_clears(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "queue-strip");
        // 流式排队：steering 1 + followUp 2 → 3 项 → 取 2 行 + `… 1 more`。
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state.reduce(
                    &ClientEvent::Event(serde_json::json!({
                        "type": "queue_update",
                        "steering": ["s1"],
                        "followUp": ["q1", "q2"]
                    })),
                    &term.commander,
                    &term.ui_tx,
                );
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();

        let queue_text = |cx: &mut TestAppContext| {
            let texts = cx
                .update_window(handle, |_, window, _| {
                    gpui_kit::base::test_support::texts(window)
                })
                .expect("texts");
            texts
                .iter()
                .filter(|t| t.path().contains(&ElementId::Name("gpui-queue".into())))
                .map(|t| t.text().to_string())
                .collect::<Vec<_>>()
                .join("\n")
        };

        let q = queue_text(cx);
        assert!(q.contains("[steer] s1"), "steer 行: {q:?}");
        assert!(q.contains("[queue] q1"), "queue 行: {q:?}");
        assert!(q.contains("… 1 more"), "超 2 行计数: {q:?}");

        // 清空队列 → 整条消失（空队列不占位）。
        cx.update(|cx| {
            term.update(cx, |term, _| {
                term.state.reduce(
                    &ClientEvent::Event(serde_json::json!({
                        "type": "queue_update",
                        "steering": [],
                        "followUp": []
                    })),
                    &term.commander,
                    &term.ui_tx,
                );
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();
        assert!(queue_text(cx).is_empty(), "空队列不应渲染任何行");
    }

    /// extension widget 面板（task 3.3）：setWidget 数据 → ` widget ` 标题
    /// 边框面板可见（青色文字）；widgetLines null → 清除；未设置/已清除
    /// 不占位（消息区照常可见，不被面板挤压覆盖）。
    ///
    /// 功能: 无数据先渲染断言无 gpui-widget 文本 → 注入 setWidget（两行）
    ///       → 断言标题与行在场且消息区仍可见 → 注入 null → 断言清除；
    /// 输入: cx — 测试上下文；
    /// 输出: 断言通过（RPC → handle_extension_request → 面板渲染链路）。
    #[gpui_kit::test]
    fn widget_panel_set_and_clear(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "widget-panel");
        // 消息区底料（断言面板不遮挡消息内容）。
        cx.update(|cx| {
            term.update(cx, |term, _| feed_fixture(term, "text_only"));
        });
        let widget_text = |cx: &mut TestAppContext| {
            let texts = cx
                .update_window(handle, |_, window, _| {
                    gpui_kit::base::test_support::texts(window)
                })
                .expect("texts");
            texts
                .iter()
                .filter(|t| t.path().contains(&ElementId::Name("gpui-widget".into())))
                .map(|t| t.text().to_string())
                .collect::<Vec<_>>()
                .join("\n")
        };
        let inject = |cx: &mut TestAppContext, widget_lines: serde_json::Value| {
            cx.update(|cx| {
                term.update(cx, |term, _| {
                    term.state.reduce(
                        &ClientEvent::Event(serde_json::json!({
                            "type": "extension_ui_request",
                            "id": "w1",
                            "method": "setWidget",
                            "widgetLines": widget_lines
                        })),
                        &term.commander,
                        &term.ui_tx,
                    );
                });
            });
        };
        let render = |cx: &mut TestAppContext| {
            cx.update_window(handle, |_, window, cx| window.render_frame(cx))
                .expect("render");
            cx.run_until_parked();
        };

        // 未设置：无面板文本（不占位）。
        render(cx);
        assert!(widget_text(cx).is_empty(), "未设置不应有面板");

        // setWidget 带数据 → 面板可见（标题 + 行），消息区不被遮挡。
        inject(cx, serde_json::json!(["line-a", "line-b"]));
        render(cx);
        let w = widget_text(cx);
        assert!(w.contains(" widget "), "标题在场: {w:?}");
        assert!(w.contains("line-a"), "widget 行 1: {w:?}");
        assert!(w.contains("line-b"), "widget 行 2: {w:?}");
        let msg = msg_texts(cx, handle);
        assert!(!msg.is_empty(), "面板不应挤压/遮挡消息区");

        // widgetLines null → 清除（主规格：null 清空而非报错）。
        inject(cx, serde_json::Value::Null);
        render(cx);
        assert!(widget_text(cx).is_empty(), "null 应清除面板");
        let msg = msg_texts(cx, handle);
        assert!(!msg.is_empty(), "清除后消息区照常");
    }

    /// ctrl+t / ctrl+o 端到端（task 4.1）：按键经拦截层 → `Route::ToApp` →
    /// `App::handle_key` 翻转全局开关，消息区即时重渲染，按键不落入
    /// Textarea（spec: 键位与全局折叠开关）。
    ///
    /// 功能: 注入 thinking 块 + 20 行 read 卡（均折叠）→ 断言占位/提示
    ///       在场 → press ctrl+t → thinking 正文展开且输入框仍空 →
    ///       press ctrl+o → 卡全量展开、提示消失且输入框仍空；
    /// 输入: cx — 测试上下文；
    /// 输出: 断言通过（两全局开关翻转 + 重渲染 + 不漏入编辑器）。
    #[gpui_kit::test]
    fn ctrl_t_ctrl_o_toggle_globals_end_to_end(cx: &mut TestAppContext) {
        let (handle, term) = open_focused(cx, "ctrl-to");
        cx.update(|cx| {
            term.update(cx, |term, _| {
                use serde_json::json;
                let rows: String = (1..=20).map(|i| format!("row{i}\n")).collect();
                let steps = [
                    json!({"type":"message_start","message":{"role":"assistant","content":[]}}),
                    // 非空 thinking 块（默认折叠为占位）。
                    json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_start","contentIndex":0}}),
                    json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","contentIndex":0,"delta":"secret-thoughts"}}),
                    // read 卡（20 行结果 → 默认折叠带提示行）。
                    json!({"type":"message_update","assistantMessageEvent":{"type":"toolcall_start","contentIndex":1,"id":"rc1","toolName":"read"}}),
                    json!({"type":"message_update","assistantMessageEvent":{"type":"toolcall_delta","contentIndex":1,"delta":"{\"path\":\"src/main.rs\",\"offset\":1,\"limit\":20}"}}),
                    json!({"type":"tool_execution_start","toolCallId":"rc1","toolName":"read","args":{"path":"src/main.rs","offset":1,"limit":20}}),
                    json!({"type":"tool_execution_end","toolCallId":"rc1","toolName":"read","isError":false,"durationMs":3,
                        "result":{"content":[{"type":"text","text":rows}],"structuredContent":rows}}),
                ];
                for raw in steps {
                    term.state.reduce(&ClientEvent::Event(raw), &term.commander, &term.ui_tx);
                }
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("render");
        cx.run_until_parked();

        // 初始：两处均折叠（占位 + 提示行），输入框为空。
        let msg = msg_texts(cx, handle);
        assert!(msg.contains("<thinking 15 chars>"), "thinking 占位: {msg:?}");
        assert!(msg.contains("more lines, ctrl+o to expand"), "卡提示行: {msg:?}");
        assert!(!msg.contains("secret-thoughts"), "折叠态不显 thinking 正文: {msg:?}");
        assert!(!msg.contains("row20"), "折叠态不显全文: {msg:?}");

        // ctrl+t → thinking 全局展开，不入输入框。
        cx.update_window(handle, |_, window, cx| window.press("ctrl-t", cx))
            .expect("press ctrl+t");
        cx.run_until_parked();
        let global = cx.update(|cx| {
            term.update(cx, |term, _| term.state.app.thinking_expanded_effective(999))
        });
        let editor = cx.update(|cx| term.update(cx, |term, _| term.state.app.editor.text().to_string()));
        let focused = cx.update_window(handle, |_, window, cx| {
            term.read(cx).focus.is_focused(window)
        }).unwrap_or(false);
        assert!(global, "ctrl+t 后 show_thinking 应翻转 (editor={editor:?} focused={focused})");
        let msg = msg_texts(cx, handle);
        assert!(msg.contains("secret-thoughts"), "ctrl+t 后 thinking 展开: {msg:?}");
        assert!(!msg.contains("<thinking 15 chars>"), "占位应消失: {msg:?}");
        let value = cx.update(|cx| term.read(cx).textarea.read(cx).value().to_string());
        assert_eq!(value, "", "ctrl+t 不应落入输入框");

        // ctrl+o → 工具卡全局展开（提示行去、全文在），不入输入框。
        cx.update_window(handle, |_, window, cx| window.press("ctrl-o", cx))
            .expect("press ctrl+o");
        cx.run_until_parked();
        let msg = msg_texts(cx, handle);
        assert!(msg.contains("row20"), "ctrl+o 后卡展开: {msg:?}");
        assert!(!msg.contains("more lines, ctrl+o to expand"), "提示行应消失: {msg:?}");
        let value = cx.update(|cx| term.read(cx).textarea.read(cx).value().to_string());
        assert_eq!(value, "", "ctrl+o 不应落入输入框");
    }

    /// 计时走字任务启停（task 4.2）：Running 期间起 1s interval 且每秒
    /// notify（观察者计数递增）；工具结束 → 渲染点对账取消，后续时钟
    /// 推进不再重绘（镜像 ratatui `timer_redraw_due` 门控）。
    ///
    /// 功能: 注入 tool_execution_start → 渲染（任务在场）→
    ///       advance_clock(1s)+pump → 断言 notify 计数 +1 →
    ///       注入 tool_execution_end → 渲染（任务已取消）→
    ///       再推进时钟 → 计数不再增长；
    /// 输入: cx — 测试上下文；
    /// 输出: 断言通过（任务启停与周期重绘门控）。
    #[gpui_kit::test]
    fn timer_task_starts_while_running_and_stops_when_ended(cx: &mut TestAppContext) {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;
        let (handle, term) = open_focused(cx, "timer-tick");
        // notify 计数（观察者挂在组件上，区分周期重绘与手动渲染）。
        let ticks = Arc::new(AtomicUsize::new(0));
        let _sub = cx.update(|cx| {
            let ticks = ticks.clone();
            cx.observe(&term, move |_, _| {
                ticks.fetch_add(1, Ordering::SeqCst);
            })
        });
        let running = |cx: &mut TestAppContext, done: bool| {
            cx.update(|cx| {
                term.update(cx, |term, _| {
                    use serde_json::json;
                    if !done {
                        term.state.reduce(
                            &ClientEvent::Event(json!({
                                "type":"message_start","message":{"role":"assistant","content":[]}
                            })),
                            &term.commander,
                            &term.ui_tx,
                        );
                        term.state.reduce(
                            &ClientEvent::Event(json!({
                                "type":"message_update",
                                "assistantMessageEvent":{"type":"toolcall_start","contentIndex":0,"id":"t1","toolName":"bash"}
                            })),
                            &term.commander,
                            &term.ui_tx,
                        );
                        term.state.reduce(
                            &ClientEvent::Event(json!({
                                "type":"tool_execution_start","toolCallId":"t1","toolName":"bash","args":{"command":"sleep 9"}
                            })),
                            &term.commander,
                            &term.ui_tx,
                        );
                    } else {
                        term.state.reduce(
                            &ClientEvent::Event(json!({
                                "type":"tool_execution_end","toolCallId":"t1","toolName":"bash","isError":false,"durationMs":5,
                                "result":{"content":[{"type":"text","text":"done"}]}
                            })),
                            &term.commander,
                            &term.ui_tx,
                        );
                    }
                });
            });
        };
        let render = |cx: &mut TestAppContext| {
            cx.update_window(handle, |_, window, cx| window.render_frame(cx))
                .expect("render");
            cx.run_until_parked();
        };
        let tick_count = |cx: &mut TestAppContext| {
            cx.update(|_| ticks.load(Ordering::SeqCst))
        };
        let task_alive = |cx: &mut TestAppContext| {
            cx.update(|cx| term.read(cx).timer_task.is_some())
        };

        // Running：渲染点对账起任务；时钟未推进 → 尚未走字。
        running(cx, false);
        render(cx);
        assert!(task_alive(cx), "Running 期间应有计时任务");
        assert_eq!(tick_count(cx), 0, "未到 1s 不应重绘");

        // 推进 1s → interval 触发 → notify（观察者计数 +1），任务仍在。
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        assert_eq!(tick_count(cx), 1, "Running 期间每秒 notify 走字");
        assert!(task_alive(cx), "走字后任务应继续");

        // 工具结束 → 渲染点对账取消任务。
        running(cx, true);
        render(cx);
        assert!(!task_alive(cx), "Ended 后任务应取消");

        // 再推进时钟 → 无任务可触发，计数不增（无周期重绘）。
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        assert_eq!(tick_count(cx), 1, "Ended 后不应有周期重绘");
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
