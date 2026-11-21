//! 渲染分段计时与门控诊断输出（常驻基准归因 + `PI_RENDER_PROBE` 门控）。
//!
//! 把 `view::render` 一帧内部按布局/高亮/工具卡/wrap 计数/wrap/paint 分段
//! 计时，并记录布局缓存重排范围：计数为常驻基准（`bench_warm_frame_cells1`
//! 等）的分段归因基础设施；`[draw-probe]`/`[draw-cost]`/`[draw-seg]` 诊断
//! 打印由 [`enabled`] 门控（环境变量 `PI_RENDER_PROBE=1` 开启，默认关闭
//! 零日志噪音）。线程局部累加、[`take`] 一次性取走清零；不影响渲染输出。

use std::cell::RefCell;
use std::sync::OnceLock;

/// 分段计时与重排范围快照（[`take`] 产物）。
#[derive(Debug, Default, Clone, Copy)]
pub struct SegProbe {
    /// 布局段（`LayoutCache::refresh` 合计，内含高亮段）纳秒。
    pub layout_ns: u64,
    /// 高亮段（markdown 解析/语法高亮，落在布局段内）纳秒。
    pub highlight_ns: u64,
    /// wrap 段（分块借出 + 视口窗口化 + 命中扫描）纳秒。
    pub wrap_ns: u64,
    /// paint 段（组件绘制进 frame buffer）纳秒。
    pub paint_ns: u64,
    /// 工具卡渲染（`tools::tool_lines_themed`，落在布局段内）纳秒。
    pub toolcard_ns: u64,
    /// wrap 行数计数（`wrap::count_wrapped_rows` 批量，落在布局段内）纳秒。
    pub wrapcount_ns: u64,
    /// 本帧布局缓存重排的最小条目下标（`usize::MAX` = 无重排）。
    pub relayout_from: usize,
    /// 本帧布局缓存重排的条目数（多次 refresh 累计）。
    pub relayout_entries: usize,
    /// 本帧布局缓存重排的逻辑行数（多次 refresh 累计）。
    pub relayout_lines: usize,
}

thread_local! {
    /// 线程局部累加器（渲染单线程；测试亦在各自线程独立计量）。
    static SEG: RefCell<SegProbe> = RefCell::new(SegProbe {
        relayout_from: usize::MAX,
        ..SegProbe::default()
    });
}

/// 累加布局段耗时。
///
/// 输入: ns — 本次 `LayoutCache::refresh` 调用的纳秒数
/// 输出: 无返回值；副作用为线程局部计数器累加
pub fn record_layout(ns: u64) {
    SEG.with(|s| s.borrow_mut().layout_ns += ns);
}

/// 累加高亮段耗时（markdown 解析/语法高亮）。
///
/// 输入: ns — 本次解析调用的纳秒数
/// 输出: 无返回值；副作用为线程局部计数器累加
pub fn record_highlight(ns: u64) {
    SEG.with(|s| s.borrow_mut().highlight_ns += ns);
}

/// 累加 wrap 段耗时（分块借出 + 视口窗口化 + 命中扫描）。
///
/// 输入: ns — 本次 wrap 调用的纳秒数
/// 输出: 无返回值；副作用为线程局部计数器累加
pub fn record_wrap(ns: u64) {
    SEG.with(|s| s.borrow_mut().wrap_ns += ns);
}

/// 累加 paint 段耗时（组件绘制进 frame buffer）。
///
/// 输入: ns — 本次绘制的纳秒数
/// 输出: 无返回值；副作用为线程局部计数器累加
pub fn record_paint(ns: u64) {
    SEG.with(|s| s.borrow_mut().paint_ns += ns);
}

/// 累加工具卡渲染耗时（落在布局段内）。
///
/// 输入: ns — 本次 `tools::tool_lines_themed` 的纳秒数
/// 输出: 无返回值；副作用为线程局部计数器累加
pub fn record_toolcard(ns: u64) {
    SEG.with(|s| s.borrow_mut().toolcard_ns += ns);
}

/// 累加 wrap 行数计数耗时（落在布局段内）。
///
/// 输入: ns — 本次 `count_wrapped_rows` 批量的纳秒数
/// 输出: 无返回值；副作用为线程局部计数器累加
pub fn record_wrapcount(ns: u64) {
    SEG.with(|s| s.borrow_mut().wrapcount_ns += ns);
}

/// 记录一次布局缓存重排范围。
///
/// 输入: from — 本次重排的最小条目下标（usize::MAX = 无重排则不记）；
///       entries — 本次重排的条目数；lines — 本次重排的逻辑行数
/// 输出: 无返回值；副作用为重排下标取 min、条目/行数累加
pub fn record_relayout(from: usize, entries: usize, lines: usize) {
    if from == usize::MAX {
        return;
    }
    SEG.with(|s| {
        let mut seg = s.borrow_mut();
        seg.relayout_from = seg.relayout_from.min(from);
        seg.relayout_entries += entries;
        seg.relayout_lines += lines;
    });
}

/// 取走并清零当前线程的分段快照。
///
/// 输入: 无
/// 输出: 自上次 `take` 以来的累计 [`SegProbe`]（无记录时各段为 0、
///       `relayout_from` 为 `usize::MAX`）
pub fn take() -> SegProbe {
    SEG.with(|s| {
        std::mem::replace(
            &mut *s.borrow_mut(),
            SegProbe {
                relayout_from: usize::MAX,
                ..SegProbe::default()
            },
        )
    })
}

/// 诊断输出门控：环境变量 `PI_RENDER_PROBE` 存在即开启（读一次缓存）。
///
/// 输入: 无
/// 输出: true = 打印 `[draw-probe]`/`[draw-cost]`/`[draw-seg]` 诊断并累计
///       backend 分解统计；false = 全部旁路（默认）
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("PI_RENDER_PROBE").is_some())
}
