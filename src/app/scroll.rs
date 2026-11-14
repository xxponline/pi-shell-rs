//! 消息区滚动状态（纯逻辑，无终端依赖）。
//!
//! 背景见 `openspec/changes/add-message-scrollback/design.md` D1：采用
//! "换行后绝对行号 + 贴底跟随标志"，与 pi `ScrollView` 的
//! `scrollTop + followingEnd` 同构——绝对行号在内容增长时不动即钉住，
//! 正是规格"上滚后新内容不抢位"所需；"距底偏移量"方案会随增长滑动，已否决。

/// 整页滚动的上下文重叠行数（对齐 pi `PAGE_SCROLL_OVERLAP`：翻页后保留
/// 上下文，避免整屏换血）。
pub const PAGE_SCROLL_OVERLAP: usize = 4;

/// 消息区滚动状态：视口首行（换行后绝对行号）+ 贴底跟随标志。
///
/// 字段：
/// - `top` — 当前视口首行在**换行后**行序列中的绝对行号；`following` 时
///   渲染端每帧重算为 `len - height`，此值仅在钉住时生效（每帧被 absorb
///   同步为实际显示首行）。
/// - `following` — 贴底跟随标志；true 时新内容保持视口贴底，false 时视口
///   钉在原内容上不被新内容抢占。
/// - `viewport` — 上次渲染的消息区高度（页长计算用；渲染端每帧回写），
///   resize 后下一帧自纠，首帧前为 0（页长退化为 1 行）。
/// - `content_len` — 内容总行数（换行后；渲染端每帧回写，供滚动条几何，
///   design D1）；首帧前为 0。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatScroll {
    /// 视口首行（换行后绝对行号）。
    pub top: usize,
    /// 贴底跟随标志。
    pub following: bool,
    /// 上次渲染的消息区高度（页长计算用）。
    pub viewport: u16,
    /// 内容总行数（渲染端每帧回写，滚动条几何用，design D1）。
    pub content_len: usize,
}

impl Default for ChatScroll {
    /// 新会话默认贴底跟随（top=0、viewport 尚未渲染为 0）；
    /// `ViewUi` 的 derive(Default) 依赖此语义。
    fn default() -> Self {
        Self {
            top: 0,
            following: true,
            viewport: 0,
            content_len: 0,
        }
    }
}

impl ChatScroll {
    /// 整页滚动的行数：视口高减去上下文重叠行，至少 1 行。
    ///
    /// 输入: 无（读 self.viewport）
    /// 输出: 页长（行数）；viewport 未渲染（0）或不足重叠行数时退化为 1
    pub fn page(&self) -> usize {
        (self.viewport as usize)
            .saturating_sub(PAGE_SCROLL_OVERLAP)
            .max(1)
    }

    /// 向上翻一页（看更早内容）：打断贴底跟随并饱和上移。
    ///
    /// 输入: 无
    /// 输出: 副作用为 following=false、top 减一页（不低于 0）；
    ///       已在顶部时保持 top=0 但跟随仍被打断
    pub fn page_up(&mut self) {
        self.following = false;
        self.top = self.top.saturating_sub(self.page());
    }

    /// 向下翻一页（看更新内容）：饱和下移。
    ///
    /// 输入: 无
    /// 输出: 副作用为 top 加一页（饱和防溢出）；不改 following——
    ///       是否已到底、是否恢复跟随由渲染端 absorb 判定
    pub fn page_down(&mut self) {
        self.top = self.top.saturating_add(self.page());
    }

    /// 滚轮上滚 n 行（看更早内容）：打断贴底跟随并饱和上移。
    ///
    /// 输入: n — 移动行数（正数；加速器产出）；0 为无操作（不改状态）
    /// 输出: 副作用为 following=false、top 减 n（不低于 0）；
    ///       已在顶部时保持 top=0 但跟随仍被打断
    pub fn scroll_up_lines(&mut self, n: usize) {
        if n == 0 {
            return;
        }
        self.following = false;
        self.top = self.top.saturating_sub(n);
    }

    /// 滚轮下滚 n 行（看更新内容）：饱和下移。
    ///
    /// 输入: n — 移动行数（正数）；0 为无操作（不改状态）
    /// 输出: 副作用为 top 加 n（饱和防溢出）；不改 following——
    ///       是否已到底、是否恢复跟随由渲染端 absorb 判定
    pub fn scroll_down_lines(&mut self, n: usize) {
        if n == 0 {
            return;
        }
        self.top = self.top.saturating_add(n);
    }

    /// 吸收渲染端反馈：回写视口首行并据"是否贴底"恢复/维持跟随。
    ///
    /// 输入:
    ///   effective_top — 渲染端 clamp 后实际使用的视口首行
    ///   at_bottom     — 该首行是否已在内容底部（含内容不足一屏）
    /// 输出: 副作用为 top=effective_top、following=at_bottom；
    ///       贴底即恢复跟随——覆盖规格"滚回底部恢复跟随"与
    ///       "内容骤变 clamp 到底恢复跟随"两条路径
    pub fn absorb(&mut self, effective_top: usize, at_bottom: bool) {
        self.top = effective_top;
        self.following = at_bottom;
    }

    /// 绝对定位视口首行（滚动条跳转/拖拽 scrub 用，复刻 pi `scrollTo`）。
    ///
    /// 输入: top — 目标视口首行（换行后绝对行号，可越界）
    /// 输出: 副作用为 clamp 到 [0, max_top] 并按是否到底恢复/清除跟随
    ///       （max_top = content_len − viewport；内容不足一屏时为 0
    ///       → 恒贴底）；与 page/scroll 系不同，此函数直接写绝对位置
    pub fn set_top_abs(&mut self, top: usize) {
        let max_top = self.content_len.saturating_sub(self.viewport as usize);
        let t = top.min(max_top);
        self.top = t;
        self.following = t >= max_top;
    }
}

/// 计算 prompt 跳转目标视口首行（add-prompt-jump design D2）。
///
/// 输入: starts — 各消息起始绝对行（升序——hits 按行序产出；可空）；
///       top — 当前视口首行；max_top — 最大可滚距离（content − viewport）；
///       dir — -1 上一条 / +1 下一条
/// 输出: 目标 top——prev = 严格小于 top 的最后一个 start，无则 0；
///       next = 严格大于 top 的第一个 start，无则 max_top（贴底）；
///       不在此处 clamp（set_top_abs 统一收敛，端点由它恢复跟随）
pub fn prompt_jump_target(starts: &[usize], top: usize, max_top: usize, dir: i8) -> usize {
    if dir < 0 {
        starts
            .iter()
            .rev()
            .find(|&&s| s < top)
            .copied()
            .unwrap_or(0)
    } else {
        starts
            .iter()
            .find(|&&s| s > top)
            .copied()
            .unwrap_or(max_top)
    }
}

/// 滚动条瞬态隐藏延迟（对齐 pi `scrollbarHideDelayMs` 默认 1000ms；判定经
/// 事件循环 200ms tick，实际隐延迟 1.0–1.2s，design D3 记录接受）。
pub const SCROLLBAR_HIDE_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

/// 滚动条拖拽状态（add-message-scrollbar design D5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollbarDrag {
    /// 按下点距 thumb 顶的行偏移（抓 thumb）；点轨道时为 thumb 高的一半。
    pub grab_offset: usize,
}

/// 滚动条瞬态判隐（纯函数，design D3）。
///
/// 输入: visible — 当前是否显示；hovered — 悬停中（压制计时）；
///       activity_age — 距最近一次滚动移动的时长
/// 输出: true = 应隐藏（可见且非悬停且 ≥ SCROLLBAR_HIDE_DELAY）
pub fn scrollbar_should_hide(
    visible: bool,
    hovered: bool,
    activity_age: std::time::Duration,
) -> bool {
    visible && !hovered && activity_age >= SCROLLBAR_HIDE_DELAY
}

// ─────────────────────────── 滚轮加速器 ───────────────────────────

/// 间隔 < 此值的事件属同一物理 notch（Ghostty 约 4ms 连发）：不加速也不更新均值。
const BURST_GAP_MS: u64 = 5;
/// 间隔 > 此值视为手势结束（重新从 1 行起步）。
const GESTURE_GAP_MS: u64 = 200;
/// 平均间隔映射基准：100ms 间隔 = 每事件 1 行，更快按比例放大。
const REFERENCE_GAP_MS: f64 = 100.0;
/// 自动加速的每事件行数上限（对齐 pi MAX_AUTO_LINES）。
const MAX_AUTO_LINES: f64 = 6.0;
/// Alt+滚轮行数倍率（对齐 pi ALT_WHEEL_SCROLL_MULTIPLIER）。
pub const ALT_WHEEL_MULTIPLIER: u16 = 5;

/// 滚轮事件 → 行数的变速加速器（仅 auto 模式，移植 pi `WheelScrollAccelerator`）。
///
/// 背景：本地 darwin 终端已由 OS 逐行加速（`accelerate=false`，防双倍）；
/// 其余平台/SSH 按事件间隔加速——慢速单发 1 行，快速连滚最多 6 行/事件。
/// 纯状态机、无 TTY：`next(dir, now_ms)` 由事件源线程在读取点打点调用。
pub struct WheelAccel {
    /// 起点时刻（把单调时钟折算成毫秒；测试直接喂 now_ms）。
    t0: std::time::Instant,
    /// 上一事件时刻（None = 尚无事件，首事件必然重置手势）。
    last_ms: Option<u64>,
    /// 上一事件方向（0 = 无；换向重置加速）。
    last_dir: i8,
    /// 指数移动平均的事件间隔（ms；None = 手势未建立）。
    avg_gap: Option<f64>,
    /// 分数行进位（如 1.667 行 → 本次 1 行、进位 0.667）。
    carry: f64,
    /// 是否按速度加速（false = 恒 1 行/事件）。
    accelerate: bool,
}

impl WheelAccel {
    /// 按本机判定构造（本地 darwin 非 SSH = 不加速，其余加速）。
    ///
    /// 输入: 无
    /// 输出: 可直接用于事件翻译的加速器
    pub fn new() -> Self {
        Self::with_accelerate(!terminal_accelerates_wheel())
    }

    /// 以指定加速开关构造（测试与平台判定分层用）。
    ///
    /// 输入: accelerate — true=按速度加速；false=恒 1 行/事件
    /// 输出: 新加速器实例（手势状态未建立）
    pub fn with_accelerate(accelerate: bool) -> Self {
        Self {
            t0: std::time::Instant::now(),
            last_ms: None,
            last_dir: 0,
            avg_gap: None,
            carry: 0.0,
            accelerate,
        }
    }

    /// 自构造以来的单调毫秒（事件源线程读取点打点用）。
    ///
    /// 输入: 无
    /// 输出: 毫秒数（饱和为 u64）
    pub fn now_ms(&self) -> u64 {
        self.t0.elapsed().as_millis() as u64
    }

    /// 把一个滚轮事件换算为移动行数（移植 pi `next`）。
    ///
    /// 输入: dir — 方向（-1 上滚 / +1 下滚）；now_ms — 事件时刻（单调毫秒）
    /// 输出: 行数（≥1）；不加速模式恒 1；加速模式按事件间隔缩放——
    ///       <5ms 连发 = 同一 notch 恒 1；换向/停顿 >200ms 重置为 1；
    ///       分数进位累积（确定性：同输入序列 → 同输出）
    pub fn next(&mut self, dir: i8, now_ms: u64) -> u16 {
        if !self.accelerate {
            return 1;
        }
        let gap = match self.last_ms {
            Some(last) => now_ms.saturating_sub(last),
            // 首事件：无上一时刻，必然走“手势未建立”分支。
            None => u64::MAX,
        };
        let same_gesture =
            self.last_ms.is_some() && dir == self.last_dir && gap <= GESTURE_GAP_MS;
        self.last_ms = Some(now_ms);
        self.last_dir = dir;
        if !same_gesture {
            self.avg_gap = None;
            self.carry = 0.0;
            return 1;
        }
        if gap < BURST_GAP_MS {
            // 同一 notch 的高速连发：不加速，也不更新均值/进位（pi 同）。
            return 1;
        }
        let avg = match self.avg_gap {
            Some(a) => (a + gap as f64) / 2.0,
            None => gap as f64,
        };
        self.avg_gap = Some(avg);
        let lines = (REFERENCE_GAP_MS / avg).clamp(1.0, MAX_AUTO_LINES) + self.carry;
        let whole = lines.floor();
        self.carry = lines - whole;
        whole as u16
    }
}

impl Default for WheelAccel {
    /// 默认 = 按本机判定构造（与 `new()` 一致）。
    fn default() -> Self {
        Self::new()
    }
}

/// 终端是否已对滚轮做过加速（移植 pi `terminalAcceleratesWheel`）。
///
/// 输入: 无（读平台与 SSH_* 环境变量）
/// 输出: true = 本地 macOS(darwin) 且非 SSH（OS 已逐行上报，我们不再加速）
pub fn terminal_accelerates_wheel() -> bool {
    terminal_accelerates_wheel_if(
        cfg!(target_os = "macos"),
        ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"]
            .iter()
            .any(|k| std::env::var(k).is_ok()),
    )
}

/// 纯判定核心（可测，不读环境）。
///
/// 输入: is_macos — 平台是否 macOS(darwin)；ssh — 是否 SSH 会话
/// 输出: true = 终端已加速（本地 macOS 且非 SSH）
fn terminal_accelerates_wheel_if(is_macos: bool, ssh: bool) -> bool {
    is_macos && !ssh
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 任务 1.1：default 即贴底跟随（新会话/测试构造的默认语义）。
    #[test]
    fn default_follows_bottom() {
        let s = ChatScroll::default();
        assert!(s.following, "默认应贴底跟随");
        assert_eq!(s.top, 0);
        assert_eq!(s.viewport, 0);
        assert_eq!(s.content_len, 0, "首帧前内容总行数为 0");
    }

    /// 任务 1.1：set_top_abs 绝对定位——中段钉住、越界 clamp、贴底恢复
    /// 跟随、内容不足一屏恒贴底（复刻 pi scrollTo，滚动条跳转/拖拽用）。
    #[test]
    fn set_top_abs_clamps_and_restores_follow() {
        let mut s = ChatScroll {
            top: 0,
            following: true,
            viewport: 20,
            content_len: 100,
        };

        // 中段：钉住
        s.set_top_abs(40);
        assert_eq!(s.top, 40, "中段定位");
        assert!(!s.following, "中段不跟随");

        // 越界：clamp 到 max_top=80 并恢复跟随
        s.set_top_abs(999);
        assert_eq!(s.top, 80, "越界 clamp 到底");
        assert!(s.following, "到底恢复跟随");

        // 贴底定位本身即跟随态
        s.set_top_abs(80);
        assert!(s.following);

        // 内容不足一屏：max_top=0 → 恒贴底
        let mut fit = ChatScroll {
            top: 0,
            following: false,
            viewport: 50,
            content_len: 10,
        };
        fit.set_top_abs(5);
        assert_eq!(fit.top, 0, "不足一屏 clamp 到 0");
        assert!(fit.following, "不足一屏恒贴底");
    }

    /// 任务 1.2：页长 = viewport − 重叠行，viewport 不足时退化为 1。
    #[test]
    fn page_subtracts_overlap_and_floors_at_one() {
        let mut s = ChatScroll {
            viewport: 20,
            ..Default::default()
        };
        assert_eq!(s.page(), 20 - PAGE_SCROLL_OVERLAP, "页长应减去 4 行重叠");

        s.viewport = 3;
        assert_eq!(s.page(), 1, "视口不足重叠行数时页长至少 1");

        s.viewport = 0;
        assert_eq!(s.page(), 1, "首帧前（viewport=0）页长退化为 1");
    }

    /// 任务 1.2：page_up 打断跟随并饱和上移；已在顶部不越界。
    #[test]
    fn page_up_breaks_follow_and_saturates_at_top() {
        let mut s = ChatScroll {
            content_len: 0,
            top: 10,
            following: true,
            viewport: 20,
        };
        s.page_up();
        assert!(!s.following, "上滚应打断贴底跟随");
        assert_eq!(s.top, 0, "10 < 页长 16，饱和到顶部");

        s.top = 3;
        s.page_up();
        assert_eq!(s.top, 0, "顶部饱和不越界");
        assert!(!s.following);
    }

    /// 任务 1.2：page_down 饱和下移、不改 following（恢复交给 absorb）。
    #[test]
    fn page_down_saturates_and_keeps_following_flag() {
        let mut s = ChatScroll {
            content_len: 0,
            top: 10,
            following: false,
            viewport: 20,
        };
        s.page_down();
        assert_eq!(s.top, 10 + 16, "top 加一页");
        assert!(!s.following, "page_down 本身不改 following");

        s.top = usize::MAX;
        s.page_down();
        assert_eq!(s.top, usize::MAX, "饱和防溢出");
    }

    /// 任务 1.2：absorb 贴底恢复跟随、中段维持钉住。
    #[test]
    fn absorb_restores_follow_at_bottom_and_keeps_pin_midway() {
        let mut s = ChatScroll {
            content_len: 0,
            top: 500,
            following: false,
            viewport: 20,
        };
        // 中段：维持钉住
        s.absorb(120, false);
        assert_eq!(s.top, 120, "回写实际显示首行");
        assert!(!s.following, "中段不恢复跟随");

        // 滚回/骤变 clamp 到底：恢复跟随
        s.absorb(180, true);
        assert_eq!(s.top, 180);
        assert!(s.following, "贴底即恢复跟随");
    }

    /// 任务 1.1：滚轮上滚打断跟随并按行数上移；饱和不越界。
    #[test]
    fn wheel_scroll_up_breaks_follow_and_moves_by_lines() {
        let mut s = ChatScroll {
            content_len: 0,
            top: 50,
            following: true,
            viewport: 20,
        };
        s.scroll_up_lines(16);
        assert!(!s.following, "上滚应打断贴底跟随");
        assert_eq!(s.top, 34, "按行数上移（50−16）");

        s.scroll_up_lines(100);
        assert_eq!(s.top, 0, "顶部饱和不越界");
        assert!(!s.following);
    }

    /// 任务 1.1：滚轮下滚不改 following（回底恢复交给渲染端 absorb）。
    #[test]
    fn wheel_scroll_down_keeps_following_flag() {
        let mut s = ChatScroll {
            content_len: 0,
            top: 10,
            following: false,
            viewport: 20,
        };
        s.scroll_down_lines(5);
        assert_eq!(s.top, 15, "按行数下移");
        assert!(!s.following, "下滚不直接改 following");

        // following 态下下滚也不直接改标志（渲染端每帧重算 top 并 absorb）
        let mut f = ChatScroll {
            content_len: 0,
            top: 100,
            following: true,
            viewport: 20,
        };
        f.scroll_down_lines(3);
        assert!(f.following, "following 态下滚保持标志");

        // 饱和防溢出
        f.top = usize::MAX;
        f.scroll_down_lines(1);
        assert_eq!(f.top, usize::MAX, "饱和防溢出");
    }

    /// 任务 1.1：n=0 为无操作（不打断跟随）。
    #[test]
    fn wheel_scroll_zero_is_noop() {
        let mut s = ChatScroll {
            content_len: 0,
            top: 10,
            following: true,
            viewport: 20,
        };
        s.scroll_up_lines(0);
        assert!(s.following, "0 行不打断跟随");
        assert_eq!(s.top, 10);
        s.scroll_down_lines(0);
        assert_eq!(s.top, 10);
    }

    // ────────── 任务 2.1：WheelAccel 对拍（pi wheel-scroll.test.ts auto 用例） ──────────

    /// 以固定时刻序列跑一次加速（方向恒 +1），返回每事件行数。
    fn wheel_auto(times: &[u64]) -> Vec<u16> {
        let mut a = WheelAccel::with_accelerate(true);
        times.iter().map(|&t| a.next(1, t)).collect()
    }

    /// 慢速（≥150ms 间隔）恒 1 行；50ms→2、20ms→5、10ms→6（上限）。
    #[test]
    fn wheel_accel_velocity_table_matches_pi() {
        assert_eq!(wheel_auto(&[0, 150, 300, 450]), [1, 1, 1, 1], "150ms→1");
        assert_eq!(wheel_auto(&[1000, 1050, 1100, 1150]), [1, 2, 2, 2], "50ms→2");
        assert_eq!(wheel_auto(&[2000, 2020, 2040, 2060]), [1, 5, 5, 5], "20ms→5");
        assert_eq!(wheel_auto(&[3000, 3010, 3020, 3030]), [1, 6, 6, 6], "10ms→6（上限）");
    }

    /// <5ms 连发 = 同一 notch，不加速也不更新均值（burst 用例）。
    #[test]
    fn wheel_accel_burst_same_notch_not_accelerated() {
        assert_eq!(wheel_auto(&[0, 3, 6, 9]), [1, 1, 1, 1]);
    }

    /// 换向与停顿（>200ms）重置加速为 1。
    #[test]
    fn wheel_accel_resets_on_direction_change_and_pause() {
        let mut a = WheelAccel::with_accelerate(true);
        assert_eq!(a.next(1, 0), 1);
        assert_eq!(a.next(1, 20), 5);
        assert_eq!(a.next(1, 40), 5);
        assert_eq!(a.next(-1, 60), 1, "换向重置");
        assert_eq!(a.next(1, 500), 1, "停顿 >200ms 重置");
        assert_eq!(a.next(1, 520), 5, "新手势重新加速");
    }

    /// 分数行进位：60ms 间隔 1.667 行 → [1,1,2]（carry 累积）。
    #[test]
    fn wheel_accel_fractional_carry_accumulates() {
        assert_eq!(wheel_auto(&[0, 60, 120]), [1, 1, 2]);
    }

    /// 不加速模式恒 1 行（本机 darwin 本地路径）。
    #[test]
    fn wheel_accel_disabled_always_one_line() {
        let mut a = WheelAccel::with_accelerate(false);
        assert_eq!(a.next(1, 0), 1);
        assert_eq!(a.next(1, 10), 1);
        assert_eq!(a.next(-1, 20), 1);
    }

    /// 确定性：同输入序列两个实例输出一致。
    #[test]
    fn wheel_accel_deterministic_for_same_inputs() {
        let times = [0u64, 30, 60, 90, 150, 170];
        let mut a = WheelAccel::with_accelerate(true);
        let mut b = WheelAccel::with_accelerate(true);
        for &t in &times {
            assert_eq!(a.next(1, t), b.next(1, t), "t={t} 应一致");
        }
    }

    /// 平台判定纯函数：仅本地 darwin 非 SSH 视为“终端已加速”。
    #[test]
    fn wheel_accel_platform_judgment_matches_pi() {
        assert!(terminal_accelerates_wheel_if(true, false), "darwin 本地 → 已加速");
        assert!(!terminal_accelerates_wheel_if(true, true), "darwin + SSH → 需自行加速");
        assert!(!terminal_accelerates_wheel_if(false, false), "非 darwin → 需自行加速");
        assert!(!terminal_accelerates_wheel_if(false, true), "非 darwin + SSH → 需自行加速");
    }
}
