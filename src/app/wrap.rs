//! 视口换行与滚动窗口（纯函数，无终端依赖）。
//!
//! 背景：ratatui 的 `Paragraph` 在开启 `Wrap` 时，`scroll.y` 按**换行后的
//! 屏幕行**计数，而我们无法从公开 API 拿到精确的换行行数，导致偏移不足、
//! 最新内容被挤出视口。这里改为：自己贪心换行到视口宽度，再按滚动状态
//! 截取视口窗口（following=贴底跟随，即原底部锚定行为；钉住=按换行后
//! 绝对行号开窗），渲染时不再使用 `Wrap`/`scroll`。

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::scroll::ChatScroll;
use crate::app::theme::TuiTheme;

/// 点击命中的对象类型（design D1：一表强类型分发，拒字符串键约定）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HitKind {
    /// 工具调用卡片（toolCallId）。
    Tool(String),
    /// thinking 块（全局渲染序号，空块也计数，design D2）。
    Thinking(usize),
    /// 语义消息起点（用户/assistant 条目首逻辑行的全局序号，add-
    /// prompt-jump-scroll-keys design D1——无覆盖表，点击 no-op、
    /// 跳转取区间起点）。
    MessageStart(usize),
}

/// 可点击内容在**绝对屏行**空间的命中区间（design D1/D2）。
///
/// 字段：`start` 含 / `end` 不含（半开区间）；`kind` 区分工具卡/thinking。
/// 绝对号 = 窗口化（drain）之前的全量屏行序号——流式只在底部追加，
/// 既有屏行不移位，上一帧 hits + 上一帧 top 在两次绘制间点击仍精确。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// 区间起始屏行（含）。
    pub start: usize,
    /// 区间结束屏行（不含）。
    pub end: usize,
    /// 命中对象（工具卡 id / thinking 序号）。
    pub kind: HitKind,
}

/// 滚动条轨道前景（pi theme `scrollbarTrack` = dark.json
/// `okhsl(237 7% 33%)`，经 pi 自带 `oklab.ts::okhslToRgb` 实算 → rgb(72,78,82)）。
pub const SCROLLBAR_TRACK_COLOR: ratatui::style::Color =
    ratatui::style::Color::Rgb(72, 78, 82);

/// 滚动条拖动条前景（pi theme `scrollbarThumb` = dark.json
/// `okhsl(232 7% 65%)`，经 pi 自带 `oklab.ts::okhslToRgb` 实算 →
/// rgb(151,160,165)）。
pub const SCROLLBAR_THUMB_COLOR: ratatui::style::Color =
    ratatui::style::Color::Rgb(151, 160, 165);

/// 滚动条几何（渲染与悬停/拖拽命中共用，同帧同参不错位，design D2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollbarGeometry {
    /// 轨道高（行，= 消息区高）。
    pub track_height: usize,
    /// 拖动条高（行，含最小高下限）。
    pub thumb_height: usize,
    /// 拖动条顶（相对轨道顶，行）。
    pub thumb_top: usize,
    /// 最大可滚距离（content − viewport，scrub 映射用）。
    pub max_scroll_top: usize,
}

/// 按 pi `getScrollbarGeometry` 公式计算滚动条几何（design D2）。
///
/// 输入: content_len — 内容总行数（换行后）；height — 消息区高（轨道高）；
///       top — 当前视口首行（越界自动 clamp）
/// 输出: None = 内容不足一屏或区域高为 0（不画、不可命中）；
///       Some(几何)——thumb_h = max(min(2,h), round(h*h/content))、
///       thumbTop = round(top/maxTop * (h−thumb_h))，直接移植 pi 公式
pub fn scrollbar_geometry(
    content_len: usize,
    height: usize,
    top: usize,
) -> Option<ScrollbarGeometry> {
    if height == 0 || content_len <= height {
        return None;
    }
    let min_thumb = height.min(2);
    let proportional = ((height * height) as f64 / content_len as f64).round() as usize;
    let thumb_height = min_thumb.max(proportional).min(height);
    let max_scroll_top = content_len - height;
    let max_thumb_top = height - thumb_height;
    let clamped_top = top.min(max_scroll_top);
    let thumb_top = if max_scroll_top == 0 {
        0
    } else {
        ((clamped_top as f64 / max_scroll_top as f64) * max_thumb_top as f64).round() as usize
    };
    Some(ScrollbarGeometry {
        track_height: height,
        thumb_height,
        thumb_top,
        max_scroll_top,
    })
}

/// 滚动条绘制覆盖输入（design D2）：None=本帧不画；hovered=悬停加粗。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollbarOverlay {
    /// 指针是否悬停在滚动条列（thumb 加粗为 █）。
    pub hovered: bool,
}

/// 把行的显示宽截断到 max_w（从尾部逐字删；CJK 双格整字删，不留半字）。
///
/// 输入: line — 待截断行（原地）；max_w — 目标最大显示宽
/// 输出: 无返回值；副作用为修剪 line 末尾 span（可能整 span 移除）
fn truncate_line_width(line: &mut Line<'static>, max_w: usize) {
    let mut w: usize = line.spans.iter().map(|s| s.content.as_ref().width()).sum();
    while w > max_w {
        let Some(last) = line.spans.last_mut() else {
            break;
        };
        match last.content.as_ref().chars().next_back() {
            Some(ch) => {
                w -= ch.width().unwrap_or(0);
                let mut s = last.content.as_ref().to_string();
                s.pop();
                last.content = std::borrow::Cow::Owned(s);
                if last.content.is_empty() {
                    line.spans.pop();
                }
            }
            None => {
                line.spans.pop();
            }
        }
    }
}

/// 把分块逻辑行按视口宽度换行，并按滚动状态截取视口窗口。
///
/// 输入:
///   chunks — `(逻辑行片, 逐行命中 meta 片)` 序列（条目分块借出，避免
///            全文拼装；与单片输入逐行等价）。meta 片内为命中种属：已有
///            结果的工具卡含 Tool(id)、非空 thinking 块含 Thinking(seq)、
///            消息首行含 MessageStart(seq)——每行 Vec 多标记共存，
///            design D1/D2
///   width  — 视口宽度（单元格数，CJK 按 2 计）
///   height — 视口高度（窗口行数上限，至少 1）
///   scroll — 滚动状态：following 取末尾（贴底跟随，即原底部锚定行为）；
///            钉住取 `[top, top+height)`，top 越界时 clamp 到底
///   scrollbar — 滚动条覆盖（None=本帧不画，design D2）
/// 输出:
///   `(窗口行序列, 吸收后的 ChatScroll, Vec<Hit>)`——窗口总行数
///   ≤ height；返回状态已回写 `viewport=height`、`content_len=全量` 并
///   `absorb(...)`；hits 为**窗口化之前**的全量绝对屏行区间（供点击命中）。
///   空逻辑行保留为一个空行，保证段落间距不塌缩。
pub fn wrap_for_viewport_chunks(
    chunks: &[(&[Line<'static>], &[Vec<HitKind>])],
    width: u16,
    height: u16,
    scroll: &ChatScroll,
    scrollbar: Option<ScrollbarOverlay>,
    theme: &TuiTheme,
) -> (Vec<Line<'static>>, ChatScroll, Vec<Hit>) {
    let width = width.max(1) as usize;
    let height = height.max(1) as usize;

    let mut wrapped: Vec<Line<'static>> = Vec::new();
    // 逐屏行的命中标记组：一条逻辑行的全部子行继承同一组（logical→
    // screen 传播）。
    let mut screen_meta: Vec<Vec<HitKind>> = Vec::new();
    for (chunk_lines, chunk_meta) in chunks {
        for (i, line) in chunk_lines.iter().enumerate() {
            // 父行 style（如工具卡片 bg 面板）传播到拆分出的子行（design D5）：
            // Paragraph 只给有文本的单元格上色，行尾还得补 pad。
            let style = line.style;
            let mark = chunk_meta.get(i).cloned().unwrap_or_default();
            let start = wrapped.len();
            for sub in split_on_newlines(line) {
                wrap_one(&sub, width, &mut wrapped);
            }
            for l in &mut wrapped[start..] {
                l.style = style;
            }
            // 新增屏行全部继承该逻辑行的标识。
            screen_meta.resize(wrapped.len(), mark.clone());
        }
    }

    // 全量屏行空间扫出点击命中区间（drain 窗口之前，绝对号，design D2）。
    let hits = collect_hits(&screen_meta);

    // 带 bg 的行补齐到视口宽（整行面板着色，否则行尾露底色）。
    for l in &mut wrapped {
        if l.style.bg.is_some() {
            let cur: usize = l.spans.iter().map(|s| s.content.as_ref().width()).sum();
            if cur < width {
                l.spans
                    .push(Span::styled(" ".repeat(width - cur), l.style));
            }
        }
    }

    // 窗口化（design D3 单源 `window_bounds`）：following 取末尾（现底部
    // 锚定行为），钉住取 [top, top+height)；top 越界（内容骤减）时 clamp。
    let len = wrapped.len();
    let (effective_top, at_bottom) = window_bounds(len, height, scroll);

    if effective_top > 0 {
        wrapped.drain(..effective_top);
    }
    wrapped.truncate(height);

    let next = finish_frame(
        &mut wrapped, width, len, height, effective_top, at_bottom, scroll, scrollbar, theme,
    );
    (wrapped, next, hits)
}

/// 窗口边界（design D3 单源，全文/窗口化两路径共用）。
///
/// 输入: len — 内容总屏行数；height — 视口高（≥1）；scroll — 滚动状态
/// 输出: `(effective_top, at_bottom)`——following 取末尾、钉住取
///       `[top, top+height)`（越界 clamp 到底）；贴底 = 窗口覆盖到末行
///       （含内容不足一屏、空内容）
fn window_bounds(len: usize, height: usize, scroll: &ChatScroll) -> (usize, bool) {
    let max_top = len.saturating_sub(height);
    let effective_top = if scroll.following {
        max_top
    } else {
        scroll.top.min(max_top)
    };
    let at_bottom = effective_top + height >= len;
    (effective_top, at_bottom)
}

/// 滚动条覆盖 + ChatScroll 回写（design D2 单源，全文/窗口化两路径共用）。
///
/// 输入: wrapped — 已开窗的行（原地画滚动条）；width — 视口宽（≥1）；
///       len — 内容总屏行；height — 视口高；effective_top/at_bottom —
///       窗口边界；scroll — 滚动状态；scrollbar — 覆盖（None=本帧不画）；
///       theme — 滚动条配色
/// 输出: 回写后的 `ChatScroll`（viewport/content_len/absorb）
#[allow(clippy::too_many_arguments)] // 逐参语义直白优于打包（与 render_chat 同取舍）
fn finish_frame(
    wrapped: &mut [Line<'static>],
    width: usize,
    len: usize,
    height: usize,
    effective_top: usize,
    at_bottom: bool,
    scroll: &ChatScroll,
    scrollbar: Option<ScrollbarOverlay>,
    theme: &TuiTheme,
) -> ChatScroll {
    // 滚动条绘制（design D2）：截断显示宽至 width-1 再补末列字符——CJK
    // 双格尾字整字丢弃；pad+字符同 span（fg=token 色、无 bg，对齐 pi
    // 整格替换：工具卡面板底色在该格让位）。仅窗口内行（内容超视口时
    // 窗口必满，轨道覆盖全高）。
    if let Some(overlay) = scrollbar
        && let Some(geo) = scrollbar_geometry(len, height, effective_top)
    {
        for (i, line) in wrapped.iter_mut().enumerate() {
            let thumb = i >= geo.thumb_top && i < geo.thumb_top + geo.thumb_height;
            let glyph = match (thumb, overlay.hovered) {
                (true, true) => "█",
                (true, false) => "┃",
                (false, _) => "│",
            };
            let color = if thumb {
                theme.scrollbar_thumb
            } else {
                theme.scrollbar_track
            };
            let target = width - 1;
            truncate_line_width(line, target);
            let cur: usize = line.spans.iter().map(|s| s.content.as_ref().width()).sum();
            let pad = target.saturating_sub(cur);
            line.spans.push(Span::styled(
                format!("{}{glyph}", " ".repeat(pad)),
                Style::default().fg(color),
            ));
        }
    }

    // 回写（design D2）：viewport 供页长计算；absorb 同步 top 并按
    // at_bottom 恢复/维持跟随。
    let mut next = *scroll;
    next.viewport = height as u16;
    // 内容总行数回写（窗口化前全量，滚动条几何用，design D1）。
    next.content_len = len;
    next.absorb(effective_top, at_bottom);
    next
}

/// 分块布局借出单元（design D3：窗口化 wrap 与命中扫描的共用输入）。
pub struct Chunk<'a> {
    /// 该块逻辑行（未 wrap；条目/易失尾槽各一块）。
    pub lines: &'a [Line<'static>],
    /// 逐逻辑行命中 meta（与 lines 等长）。
    pub meta: &'a [Vec<HitKind>],
    /// 逐逻辑行 wrap 计数（与 lines 等长）。
    pub counts: &'a [u32],
    /// 块屏行总数（= counts 和；O(块数) 起点定位与 len 汇总用）。
    pub total: usize,
    /// 标记跨度 `(首个非空行, 末个非空行, 首行屏内偏移)`；None = 全块
    /// 无标记（命中扫描 O(1) 跳过）。
    pub span: Option<(u32, u32, u32)>,
}

/// 计算块的标记跨度（design D3：命中扫描只走 [first..=last] 区间）。
///
/// 输入: meta — 逐逻辑行命中组；counts — 逐行 wrap 计数（与 meta 等长）
/// 输出: `Some((first, last, first_offset))` 或全空时 `None`
pub(crate) fn mark_span(meta: &[Vec<HitKind>], counts: &[u32]) -> Option<(u32, u32, u32)> {
    let first = meta.iter().position(|r| !r.is_empty())?;
    let last = meta.iter().rposition(|r| !r.is_empty())?;
    let first_offset: u32 = counts.iter().take(first).sum();
    Some((first as u32, last as u32, first_offset))
}

/// 按逻辑行计数扫描命中区间（design D3：无需排版，屏行号由计数前缀和推；
/// 每块经 mark_span 跳过无标记区间，成本 O(块数 + 标记跨度)）。
///
/// 输入: chunks — 分块借出单元序列（含 meta/计数/总数/标记跨度）
/// 输出: 与 [`collect_hits`]（屏行空间）逐项等价的绝对屏行区间序列——
///       同一逻辑行的全部子行继承同一组标记且屏行连续，两遍序（先内容类、
///       后 MessageStart）保持 hit_at 首匹配语义
fn collect_hits_counted(chunks: &[Chunk<'_>]) -> Vec<Hit> {
    let mut hits = Vec::new();
    collect_runs_counted(chunks, &mut hits, |k| !matches!(k, HitKind::MessageStart(_)));
    collect_runs_counted(chunks, &mut hits, |k| matches!(k, HitKind::MessageStart(_)));
    hits
}

/// 单遍：按 want 谓词选 kind，逻辑行序扫描连续同 kind 段（屏行连续性由
/// 相邻逻辑行首尾相接保证——design D1 两遍序在计数空间的等价实现）。
///
/// 输入: chunks — 分块借出单元；hits — 结果追加器；want — 目标 kind 判定
/// 输出: 无返回值；副作用为向 hits 追加半开区间 `[start, end)`
fn collect_runs_counted(
    chunks: &[Chunk<'_>],
    hits: &mut Vec<Hit>,
    want: impl Fn(&HitKind) -> bool,
) {
    let mut base = 0usize;
    let mut run: Option<(HitKind, usize, usize)> = None;
    for chunk in chunks {
        let entry_base = base;
        base = entry_base + chunk.total;
        let Some((first, last, first_offset)) = chunk.span else {
            // 全块无标记：等价逐行 kind=None → 关闭未完 run（O(1)）。
            if let Some((rk, start, end)) = run.take() {
                hits.push(Hit { start, end, kind: rk });
            }
            continue;
        };
        let first_row_base = entry_base + first_offset as usize;
        // 首标记行前的空隙：同样等价逐行 kind=None → 关闭（若 run 尚未
        // 接到该行）。
        if let Some((_, _, end)) = &run
            && *end < first_row_base
        {
            let (rk, start, end) = run.take().expect("run 在场");
            hits.push(Hit { start, end, kind: rk });
        }
        // 只走标记跨度 [first..=last]；跨内逐行运行状态机（同屏行版）。
        let mut row_base = first_row_base;
        for li in first..=last {
            let row = &chunk.meta[li as usize];
            let count = chunk.counts.get(li as usize).copied().unwrap_or(1) as usize;
            let line_base = row_base;
            row_base += count;
            let kind = row.iter().find(|k| want(k)).cloned();
            match (&mut run, kind) {
                (Some((rk, start, end)), Some(k)) if *rk == k && *end == line_base => {
                    *end = row_base; // 同 kind 且屏行相接 → 延伸区间
                }
                (slot, kind) => {
                    if let Some((rk, start, end)) = slot.take() {
                        hits.push(Hit { start, end, kind: rk });
                    }
                    if let Some(k) = kind {
                        *slot = Some((k, line_base, row_base));
                    }
                }
            }
        }
        // 跨尾之后的空隙留待下一块的首行检查关闭（run.end ≤ base 不变式）。
    }
    if let Some((kind, start, end)) = run.take() {
        hits.push(Hit { start, end, kind });
    }
}

/// 分块视口换行（design D3：只排版落在窗口内的逻辑行，成本 O(窗口行)）。
///
/// 输入:
///   chunks — 分块借出单元序列（逻辑行/meta/计数/块总数/标记跨度；
///            布局缓存产出）
///   width/height/scroll/scrollbar/theme — 同 [`wrap_for_viewport`]
/// 输出: 同 [`wrap_for_viewport`]——窗口行、ChatScroll 回写与 hits 与全文
///       路径逐字节相等（5.2 回归保证）；空逻辑行保留一个空行
pub fn wrap_for_viewport_windowed(
    chunks: &[Chunk<'_>],
    width: u16,
    height: u16,
    scroll: &ChatScroll,
    scrollbar: Option<ScrollbarOverlay>,
    theme: &TuiTheme,
) -> (Vec<Line<'static>>, ChatScroll, Vec<Hit>) {
    let width = width.max(1) as usize;
    let height = height.max(1) as usize;

    // 内容总行数 = 块级总数和（design D3：无需排版即得 len/content_len）。
    let len: usize = chunks.iter().map(|c| c.total).sum();
    let (effective_top, at_bottom) = window_bounds(len, height, scroll);

    // 定位窗口起点：先按块屏行总数 O(块数) 找块，再块内精定位行 + 偏移
    // （块内行数小，线性可接受；避免每帧扫全部逻辑行）。
    let mut start: Option<(usize, usize, usize)> = None;
    let mut base = 0usize;
    for (ci, chunk) in chunks.iter().enumerate() {
        if base + chunk.total > effective_top {
            let mut inner = base;
            for (li, c) in chunk.counts.iter().enumerate() {
                let count = *c as usize;
                if inner + count > effective_top {
                    start = Some((ci, li, effective_top - inner));
                    break;
                }
                inner += count;
            }
            break;
        }
        base += chunk.total;
    }

    // 从起点逐行排版，首行跳过行内偏移，凑满 height 截断（等价全文路径
    // 的 drain+truncate，design D3）。
    let mut wrapped: Vec<Line<'static>> = Vec::with_capacity(height.min(256));
    if let Some((ci0, li0, offset)) = start {
        'collect: for (ci, chunk) in chunks.iter().enumerate().skip(ci0) {
            let li_start = if ci == ci0 { li0 } else { 0 };
            for (i, line) in chunk.lines.iter().enumerate().skip(li_start) {
                if wrapped.len() >= height {
                    break 'collect;
                }
                let mut rows: Vec<Line<'static>> = Vec::new();
                for sub in split_on_newlines(line) {
                    wrap_one(&sub, width, &mut rows);
                }
                // 父行 style（面板 bg）传播到子行（design D5），同全文路径。
                for r in &mut rows {
                    r.style = line.style;
                }
                let skip = if ci == ci0 && i == li0 { offset } else { 0 };
                for r in rows.into_iter().skip(skip) {
                    if wrapped.len() >= height {
                        break;
                    }
                    wrapped.push(r);
                }
            }
        }
    }

    // 带 bg 的行补齐到视口宽（同全文路径，只作用于窗口内行）。
    for l in &mut wrapped {
        if l.style.bg.is_some() {
            let cur: usize = l.spans.iter().map(|s| s.content.as_ref().width()).sum();
            if cur < width {
                l.spans.push(Span::styled(" ".repeat(width - cur), l.style));
            }
        }
    }

    // 命中区间：逻辑层计数扫描（design D3，无需排版）。
    let hits = collect_hits_counted(chunks);
    let next = finish_frame(
        &mut wrapped, width, len, height, effective_top, at_bottom, scroll, scrollbar, theme,
    );
    (wrapped, next, hits)
}

/// 单片输入的视口换行（全量路径：存量测试与 5.2 回归基准的 oracle；
/// 渲染/选区主路径走 [`wrap_for_viewport_windowed`]，二者逐字节一致）。
///
/// 输入: lines/meta — 全量逻辑行与逐行命中 meta（等长，语义同 chunks
///       的单块）；width/height/scroll/scrollbar/theme — 同
///       [`wrap_for_viewport_windowed`]
/// 输出: 同 [`wrap_for_viewport_windowed`]（单片 = 单块）
#[allow(dead_code)]
pub fn wrap_for_viewport(
    lines: &[Line<'static>],
    width: u16,
    height: u16,
    scroll: &ChatScroll,
    meta: &[Vec<HitKind>],
    scrollbar: Option<ScrollbarOverlay>,
    theme: &TuiTheme,
) -> (Vec<Line<'static>>, ChatScroll, Vec<Hit>) {
    wrap_for_viewport_chunks(&[(lines, meta)], width, height, scroll, scrollbar, theme)
}

/// 把逐行多标记扫描为连续命中区间（纯函数，design D1 两遍序）。
///
/// 输入: meta — 逐逻辑行标记组（每行可含多个 HitKind）
/// 输出: 半开区间 `[start, end)` 序列（绝对屏行号）——**先内容类
///       （Tool/Thinking）、后 MessageStart**：hit_at 首匹配天然内容
///       动作优先，MessageStart 仅在无内容标记的行被命中（点击 no-op）；
///       同 kind 连续成段、相邻不同段分开（跳转按 kind 过滤与序无关）
fn collect_hits(meta: &[Vec<HitKind>]) -> Vec<Hit> {
    let mut hits = Vec::new();
    collect_runs(meta, &mut hits, |k| !matches!(k, HitKind::MessageStart(_)));
    collect_runs(meta, &mut hits, |k| matches!(k, HitKind::MessageStart(_)));
    hits
}

/// 单遍：按 want 谓词选 kind，行序扫描连续同 kind 段（design D1）。
///
/// 输入: meta — 逐行标记组；hits — 结果追加器；want — 本次遍历的目标
///       kind 判定
/// 输出: 无返回值；副作用为向 hits 追加区间
fn collect_runs(
    meta: &[Vec<HitKind>],
    hits: &mut Vec<Hit>,
    want: impl Fn(&HitKind) -> bool,
) {
    let mut i = 0;
    while i < meta.len() {
        let Some(kind) = meta[i].iter().find(|k| want(k)).cloned() else {
            i += 1;
            continue;
        };
        let start = i;
        while i < meta.len() && meta[i].contains(&kind) {
            i += 1;
        }
        hits.push(Hit { start, end: i, kind });
    }
}

/// 计算单个逻辑行按视口宽度换行后的屏行数（design D3 前缀和原料）。
///
/// 输入: line — 逻辑行（span 内可含 `\n`）；width — 视口宽度（单元格）
/// 输出: 屏行数（≥1；空行/仅换行均保留至少一行）；与
///       `wrap_for_viewport*` 对该行的产出行数一致（回归由 5.2 保证）
pub(crate) fn count_wrapped_rows(line: &Line<'static>, width: usize) -> u32 {
    let width = width.max(1);
    let mut out = Vec::new();
    for sub in split_on_newlines(line) {
        wrap_one(&sub, width, &mut out);
    }
    out.len() as u32
}

/// 把一个逻辑行按 span 内的 `\n` 拆成多个子行（保留每个 span 的样式）。
///
/// 输入: 一个逻辑行
/// 输出: 非空的子行序列；无 `\n` 时原样返回一个元素
fn split_on_newlines(line: &Line<'static>) -> Vec<Vec<Span<'static>>> {
    let mut subs: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    for span in &line.spans {
        let parts: Vec<&str> = span.content.as_ref().split('\n').collect();
        for (i, part) in parts.into_iter().enumerate() {
            if i > 0 {
                subs.push(Vec::new());
            }
            if !part.is_empty() {
                subs.last_mut()
                    .expect("sub lines non-empty")
                    .push(Span::styled(part.to_string(), span.style));
            }
        }
    }
    subs
}

/// 把一个子行贪心换行追加到 `out`。
///
/// 输入:
///   sub   — 不含 `\n` 的 span 序列
///   width — 目标宽度
///   out   — 结果收集器（换行后的行追加到尾部）
/// 输出: 无返回值；副作用为向 out 追加 ≥1 行
fn wrap_one(sub: &[Span<'static>], width: usize, out: &mut Vec<Line<'static>>) {
    // 空子行 → 保留一个空行。
    if sub.iter().all(|s| s.content.as_ref().is_empty()) {
        out.push(Line::default());
        return;
    }

    let mut cur: Vec<Span<'static>> = Vec::new();
    let mut cur_w: usize = 0;

    for span in sub {
        for (token, style) in tokenize(span) {
            let tw = token.width();

            if tw > width {
                // 单个 token 比整行还宽：先冲刷当前行，再按宽度硬切。
                flush(&mut cur, &mut cur_w, out);
                let chunks = hard_split(&token, width);
                let last = chunks.len().saturating_sub(1);
                for (i, chunk) in chunks.into_iter().enumerate() {
                    if i < last {
                        out.push(Line::from(Span::styled(chunk, style)));
                    } else {
                        cur.push(Span::styled(chunk, style));
                        cur_w = token_width_last(&cur);
                    }
                }
                continue;
            }

            if cur_w + tw > width {
                // 放不下：冲刷当前行；行首的空白 token 直接丢弃。
                flush(&mut cur, &mut cur_w, out);
                if token.chars().all(|c| c.is_whitespace()) {
                    continue;
                }
            }
            cur_w += tw;
            cur.push(Span::styled(token, style));
        }
    }
    flush(&mut cur, &mut cur_w, out);
}

/// 计算当前行宽（用于硬切后的最后一块）。
///
/// 输入: 当前 span 列表
/// 输出: 显示宽度总和
fn token_width_last(cur: &[Span<'static>]) -> usize {
    cur.iter().map(|s| s.content.as_ref().width()).sum()
}

/// 冲刷当前行为一行（空行也产出，避免丢失结构）。
///
/// 输入/输出: 通过 &mut 修改 cur/cur_w/out
fn flush(cur: &mut Vec<Span<'static>>, cur_w: &mut usize, out: &mut Vec<Line<'static>>) {
    if !cur.is_empty() {
        out.push(Line::from(std::mem::take(cur)));
    }
    *cur_w = 0;
}

/// 按显示宽度把超宽 token 切成多块，每块 ≤ width（按字符边界，CJK 安全）。
///
/// 输入: token 文本与目标宽度
/// 输出: 至少一个块的 Vec；总宽度保持不变（只做位置切分）
fn hard_split(token: &str, width: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for ch in token.chars() {
        let cw = ch.width().unwrap_or(0).max(1);
        if cur_w + cw > width && !cur.is_empty() {
            chunks.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        cur.push(ch);
        cur_w += cw;
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }
    if chunks.is_empty() {
        chunks.push(String::new());
    }
    chunks
}

/// 把 span 文本切成 token 序列（保留空白）。
///
/// 输入: 一个 span
/// 输出: (token 文本, 样式) 序列；非空白词与空白 run 分开返回
fn tokenize(span: &Span<'static>) -> Vec<(String, ratatui::style::Style)> {
    let text = span.content.as_ref();
    let mut tokens = Vec::new();
    let mut buf = String::new();
    let mut is_ws: Option<bool> = None;
    for ch in text.chars() {
        let ws = ch.is_whitespace() && ch != '\n';
        if is_ws.is_some() && Some(ws) != is_ws {
            tokens.push((std::mem::take(&mut buf), span.style));
        }
        is_ws = Some(ws);
        buf.push(ch);
    }
    if !buf.is_empty() {
        tokens.push((buf, span.style));
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;

    fn plain(text: &str) -> Line<'static> {
        Line::from(Span::raw(text.to_string()))
    }

    fn render(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<Vec<_>>()
                    .join("")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn content_fitting_viewport_is_unchanged() {
        let lines = vec![plain("hello"), plain("world")];
        let out = wrap_for_viewport(&lines, 40, 10, &ChatScroll::default(), &[],
        None, &TuiTheme::default()).0;
        assert_eq!(render(&out), "hello\nworld");
    }

    #[test]
    fn long_line_wraps_at_word_boundary() {
        let lines = vec![plain(&"word ".repeat(50))];
        let out = wrap_for_viewport(&lines, 20, 100, &ChatScroll::default(), &[],
        None, &TuiTheme::default()).0;
        assert!(out.len() > 1, "should wrap");
        for l in &out {
            let w: usize = l.spans.iter().map(|s| s.content.as_ref().width()).sum();
            assert!(w <= 20, "line too wide: {w}");
        }
    }

    #[test]
    fn over_height_keeps_tail_not_head() {
        let lines = vec![plain(&format!("{}TAIL", "word ".repeat(500)))];
        let out = wrap_for_viewport(&lines, 20, 10, &ChatScroll::default(), &[],
        None, &TuiTheme::default()).0;
        assert_eq!(out.len(), 10);
        let text = render(&out);
        assert!(text.trim_end().ends_with("TAIL"), "tail should be newest content: {text:?}");
        // 头部（最早的行）被丢弃
        assert!(!text.starts_with("word word word word word word word word word word word "));
    }

    #[test]
    fn embedded_newline_creates_lines() {
        let lines = vec![plain("line1\nline2\nline3")];
        let out = wrap_for_viewport(&lines, 40, 10, &ChatScroll::default(), &[],
        None, &TuiTheme::default()).0;
        assert_eq!(render(&out), "line1\nline2\nline3");
    }

    #[test]
    fn empty_logical_line_preserved() {
        let lines = vec![plain("a"), plain(""), plain("b")];
        let out = wrap_for_viewport(&lines, 40, 10, &ChatScroll::default(), &[],
        None, &TuiTheme::default()).0;
        assert_eq!(render(&out), "a\n\nb");
    }

    #[test]
    fn cjk_width_counts_double() {
        // 10 个中文字 = 20 宽，宽度 10 → 每行 5 个
        let lines = vec![plain(&"中".repeat(10))];
        let out = wrap_for_viewport(&lines, 10, 100, &ChatScroll::default(), &[],
        None, &TuiTheme::default()).0;
        assert_eq!(out.len(), 2);
        assert_eq!(render(&out), "中中中中中\n中中中中中");
    }

    #[test]
    fn unbroken_long_token_hard_splits() {
        let lines = vec![plain(&"x".repeat(45))];
        let out = wrap_for_viewport(&lines, 10, 100, &ChatScroll::default(), &[],
        None, &TuiTheme::default()).0;
        assert_eq!(out.len(), 5); // 45 chars / 10 wide
        assert_eq!(render(&out).replace('\n', ""), "x".repeat(45));
    }

    #[test]
    fn styles_survive_wrapping() {
        let style = Style::default().fg(ratatui::style::Color::Red);
        let lines = vec![Line::from(vec![
            Span::styled("aa ", style),
            Span::raw("bb cc dd ee ff gg hh ii jj"),
        ])];
        let out = wrap_for_viewport(&lines, 10, 100, &ChatScroll::default(), &[],
        None, &TuiTheme::default()).0;
        assert!(out.len() >= 2);
        // 首个 span 的样式保留
        let first = out[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<Vec<_>>()
            .join("");
        assert!(first.starts_with("aa"));
    }

    /// design D5：带 bg 的父行拆行后子行仍带 bg，且行尾补 pad 到视口宽
    /// （整行面板着色）。
    #[test]
    fn bg_style_propagates_and_pads_to_width() {
        let bg = Style::default().bg(ratatui::style::Color::Rgb(40, 44, 52));
        // 贪心拆行路径：多词行
        let lines = vec![Line::from(vec![
            Span::styled("word ".repeat(10), bg),
        ])
        .style(bg)];
        let out = wrap_for_viewport(&lines, 20, 100, &ChatScroll::default(), &[],
        None, &TuiTheme::default()).0;
        assert!(out.len() > 1, "应拆行: {}", out.len());
        for l in &out {
            assert_eq!(l.style.bg, Some(ratatui::style::Color::Rgb(40, 44, 52)), "子行保留 bg");
            let w: usize = l.spans.iter().map(|s| s.content.as_ref().width()).sum();
            assert_eq!(w, 20, "行尾补 pad 到视口宽");
            assert!(l.spans.last().unwrap().content.as_ref().ends_with(' ') || w == 0);
        }

        // 硬切路径：超宽单 token
        let lines = vec![Line::from(Span::styled("x".repeat(45), bg)).style(bg)];
        let out = wrap_for_viewport(&lines, 10, 100, &ChatScroll::default(), &[],
        None, &TuiTheme::default()).0;
        assert_eq!(out.len(), 5);
        for l in &out {
            assert_eq!(l.style.bg, Some(ratatui::style::Color::Rgb(40, 44, 52)), "硬切子行保留 bg");
            let w: usize = l.spans.iter().map(|s| s.content.as_ref().width()).sum();
            assert_eq!(w, 10, "硬切行补 pad 到视口宽");
        }

        // 无 bg 的行不补 pad（不改变既有行为）
        let lines = vec![plain("hi")];
        let out = wrap_for_viewport(&lines, 20, 10, &ChatScroll::default(), &[],
        None, &TuiTheme::default()).0;
        assert_eq!(render(&out), "hi");
    }

    /// 任务 2.1：钉住态按 top 开窗口（中段内容），不被新内容/底部挤动。
    #[test]
    fn pinned_window_starts_at_top_and_keeps_mid_content() {
        let lines: Vec<Line> = (0..50).map(|i| plain(&format!("L{i:02}"))).collect();
        let scroll = ChatScroll {
            content_len: 0,
            top: 10,
            following: false,
            viewport: 0,
        };
        let (out, next, _) = wrap_for_viewport(&lines, 20, 5, &scroll, &[],
        None, &TuiTheme::default());
        assert_eq!(render(&out), "L10\nL11\nL12\nL13\nL14", "窗口应从 top 起");
        assert_eq!(next.top, 10, "中段 top 原样回写");
        assert!(!next.following, "中段维持钉住");
        assert_eq!(next.viewport, 5, "回写视口高供页长计算");
    }

    /// 任务 2.1：following 态 = 原底部锚定（回归网）；top 越界 clamp 到底
    /// 并恢复跟随（规格场景 3 兜底路径）。
    #[test]
    fn following_keeps_tail_and_out_of_range_top_clamps_to_bottom() {
        let lines: Vec<Line> = (0..50).map(|i| plain(&format!("L{i:02}"))).collect();

        // following：取末尾，与原行为逐字节一致
        let (out, next, _) = wrap_for_viewport(&lines, 20, 5, &ChatScroll::default(), &[],
        None, &TuiTheme::default());
        assert_eq!(render(&out), "L45\nL46\nL47\nL48\nL49");
        assert!(next.following);
        assert_eq!(next.top, 45);

        // 钉住但 top 越界（内容骤减后残留 stale top）：clamp 到底 + 恢复跟随
        let stale = ChatScroll {
            content_len: 0,
            top: 60,
            following: false,
            viewport: 0,
        };
        let (out, next, _) = wrap_for_viewport(&lines, 20, 5, &stale, &[],
        None, &TuiTheme::default());
        assert_eq!(render(&out), "L45\nL46\nL47\nL48\nL49", "越界应 clamp 到底");
        assert!(next.following, "clamp 到底应恢复跟随");
        assert_eq!(next.top, 45);
    }

    /// 任务 1.1：每帧回写 content_len（drain 前全量总行数，滚动条几何用，
    /// design D1）——窗口化不改变 total，following/钉住两态一致。
    #[test]
    fn content_len_updated_each_wrap() {
        let lines: Vec<Line> = (0..50).map(|i| plain(&format!("L{i:02}"))).collect();
        let (_out, next, _) = wrap_for_viewport(&lines, 20, 10, &ChatScroll::default(), &[],
        None, &TuiTheme::default());
        assert_eq!(next.content_len, 50, "总行数 = drain 前全量");
        assert_eq!(next.top, 40, "following 取末尾");

        let pinned = ChatScroll {
            top: 10,
            following: false,
            viewport: 0,
            content_len: 0,
        };
        let (_out2, next2, _) = wrap_for_viewport(&lines, 20, 5, &pinned, &[],
        None, &TuiTheme::default());
        assert_eq!(next2.content_len, 50, "钉住态也回写总行数");
        assert_eq!(next2.top, 10, "钉住 top 不变");
    }

    /// 任务 2.1：滚动条绘制——末列字形（轨道│/常态thumb┃/悬停thumb█）、
    /// CJK 双格尾字整字丢弃、工具卡 bg 行末格让位、内容不足不画（design D2）。
    #[test]
    fn scrollbar_paints_last_column_with_glyphs_and_clears_bg() {
        // 常态：50 行、视口10、贴底（top=40）→ geometry thumb=2、thumbTop=8
        let lines: Vec<Line> = (0..50).map(|i| plain(&format!("L{i:02}"))).collect();
        let (out, _next, _) = wrap_for_viewport(
            &lines,
            10,
            10,
            &ChatScroll::default(),
            &[],
            Some(ScrollbarOverlay { hovered: false }), &TuiTheme::default());
        assert_eq!(out.len(), 10, "窗口满高");
        for (i, l) in out.iter().enumerate() {
            let last = l.spans.last().unwrap().content.as_ref();
            let expect = if i >= 8 { "┃" } else { "│" };
            assert!(last.ends_with(expect), "行 {i} 末字形 {last:?} 应为 {expect}");
            let w: usize = l.spans.iter().map(|s| s.content.as_ref().width()).sum();
            assert_eq!(w, 10, "行 {i} 补齐到视口宽");
        }
        assert!(
            out[0].spans[0].content.as_ref().starts_with("L40"),
            "内容保留（窗口从 top 起）"
        );

        // 悬停：thumb 加粗为 █，轨道不变
        let (hover, _, _) = wrap_for_viewport(
            &lines,
            10,
            10,
            &ChatScroll::default(),
            &[],
            Some(ScrollbarOverlay { hovered: true }), &TuiTheme::default());
        assert!(
            hover[9].spans.last().unwrap().content.as_ref().ends_with('█'),
            "悬停 thumb 加粗"
        );
        assert!(
            hover[0].spans.last().unwrap().content.as_ref().ends_with('│'),
            "轨道字形不变"
        );

        // CJK 双格尾字整字丢弃（逻辑行14 宽被换行成 10 宽行 → 截到 9 →
        // 补 1 空格 + 字形）；内容需超视口才画（不足一屏不画已在下方断言）
        let mut cjk: Vec<Line> = (0..15).map(|i| plain(&format!("S{i:02}"))).collect();
        cjk.push(plain("中中中中中中中"));
        let (cjk_out, _, _) = wrap_for_viewport(
            &cjk,
            10,
            10,
            &ChatScroll::default(),
            &[],
            Some(ScrollbarOverlay { hovered: false }), &TuiTheme::default());
        let cjk_row = cjk_out
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.as_ref().contains('中')))
            .expect("换行后 CJK 行在窗口内");
        let text: String = cjk_row.spans.iter().map(|s| s.content.as_ref()).collect();
        let w: usize = cjk_row.spans.iter().map(|s| s.content.as_ref().width()).sum();
        assert_eq!(w, 10, "行宽 = 视口宽: {text:?}");
        assert!(text.starts_with("中中中中"), "整字保留 4 个: {text:?}");
        assert!(!text.contains("中中中中中"), "不留第 5 个中: {text:?}");
        assert!(
            text.ends_with('│') || text.ends_with('┃'),
            "末列为滚动条字形: {text:?}"
        );

        // bg 行：末格让位（字形 span 无 bg，内容段保留 bg）
        let bg = Style::default().bg(ratatui::style::Color::Rgb(1, 2, 3));
        let bg_lines: Vec<Line> = (0..50)
            .map(|i| {
                Line::from(vec![Span::styled(format!("L{i:02}"), bg)])
                    .style(bg)
            })
            .collect();
        let (bg_out, _, _) = wrap_for_viewport(
            &bg_lines,
            10,
            10,
            &ChatScroll::default(),
            &[],
            Some(ScrollbarOverlay { hovered: false }), &TuiTheme::default());
        let l0 = &bg_out[0];
        assert!(
            l0.spans.first().unwrap().style.bg.is_some(),
            "内容段保留 bg"
        );
        assert_eq!(
            l0.spans.last().unwrap().style.bg,
            None,
            "字形格无 bg（整格替换）"
        );

        // 内容不足一屏：geometry=None → 不画
        let short = vec![plain("only")];
        let (s_out, _, _) = wrap_for_viewport(
            &short,
            10,
            10,
            &ChatScroll::default(),
            &[],
            Some(ScrollbarOverlay { hovered: false }), &TuiTheme::default());
        let st: String = s_out[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(st, "only", "内容不足不画");
    }

    /// 宿主配色注入（design D1/D2）：非默认 `TuiTheme` 下滚动条 track/thumb
    /// 格子的 fg 等于注入值，默认 theme 等于现有 const 现值。
    ///
    /// 输入: 无（50 行内容 + 贴底滚动状态 + 悬停/常态 overlay）
    /// 输出: 断言通过；末列字形 fg 与注入/默认值比对
    #[test]
    fn scrollbar_colors_follow_injected_theme() {
        let lines: Vec<Line> = (0..50).map(|i| plain(&format!("L{i:02}"))).collect();
        let injected = TuiTheme {
            scrollbar_track: ratatui::style::Color::Rgb(10, 20, 30),
            scrollbar_thumb: ratatui::style::Color::Rgb(40, 50, 60),
            ..TuiTheme::default()
        };
        let (out, _next, _) = wrap_for_viewport(
            &lines,
            10,
            10,
            &ChatScroll::default(),
            &[],
            Some(ScrollbarOverlay { hovered: false }),
            &injected,
        );
        // 贴底（top=40）→ thumb 占末两行（geo thumbTop=8）；其余为轨道
        for (i, l) in out.iter().enumerate() {
            let last = l.spans.last().unwrap();
            let expect = if i >= 8 {
                injected.scrollbar_thumb
            } else {
                injected.scrollbar_track
            };
            assert_eq!(
                last.style.fg,
                Some(expect),
                "行 {i} 应取注入滚动条色"
            );
        }

        // 默认 theme = 现有 const（与 view.rs 断言同源）
        let (d_out, _next, _) = wrap_for_viewport(
            &lines,
            10,
            10,
            &ChatScroll::default(),
            &[],
            Some(ScrollbarOverlay { hovered: false }),
            &TuiTheme::default(),
        );
        for (i, l) in d_out.iter().enumerate() {
            let last = l.spans.last().unwrap();
            let expect = if i >= 8 {
                SCROLLBAR_THUMB_COLOR
            } else {
                SCROLLBAR_TRACK_COLOR
            };
            assert_eq!(
                last.style.fg,
                Some(expect),
                "默认 theme 应等于现有 const（行 {i}）"
            );
        }
    }

    /// 任务 1.2：MessageStart 分组（add-prompt-jump design D1）——首行
    /// 超宽换行成多屏行 → 同 seq 连续成一段、start=首屏行；相邻不同
    /// seq 分段；pinned 态绝对号不变。
    #[test]
    fn message_start_spans_wrapped_rows_and_stays_absolute() {
        let lines = vec![
            plain(&"word ".repeat(30)),
            plain("mid"),
            plain("next msg"),
            plain("tail"),
        ];
        let meta = vec![
            vec![HitKind::MessageStart(0)],
            vec![],
            vec![HitKind::MessageStart(1)],
            vec![],
        ];
        let (out, _next, hits) = wrap_for_viewport(
            &lines,
            20,
            100,
            &ChatScroll::default(),
            &meta,
            None, &TuiTheme::default());
        assert!(out.len() > 4, "首行应换行成多屏行: {}", out.len());
        assert_eq!(hits.len(), 2, "两个起点各成一段: {hits:?}");
        assert_eq!(hits[0].kind, HitKind::MessageStart(0));
        assert_eq!(hits[0].start, 0, "起点 = 首屏行绝对号");
        assert!(hits[0].end > hits[0].start, "超宽行跨多屏行连续");
        assert_eq!(hits[1].kind, HitKind::MessageStart(1));
        assert!(hits[1].start > hits[0].end, "不同 seq 分段");

        // pinned：绝对号不含窗口偏移
        let scroll = ChatScroll {
            top: 3,
            following: false,
            viewport: 0,
            content_len: 0,
        };
        let (_o2, _n2, hits2) = wrap_for_viewport(&lines, 20, 5, &scroll, &meta, None, &TuiTheme::default());
        assert_eq!(
            (hits2[0].start, hits2[0].end),
            (hits[0].start, hits[0].end),
            "pinned 态绝对号不变"
        );
    }

    /// 任务 1.2：滚动条几何公式向量（移植 pi getScrollbarGeometry）。
    #[test]
    fn scrollbar_geometry_vectors_match_pi() {
        // 内容不足一屏 / 区域高 0 → None（不画、不可命中）
        assert_eq!(scrollbar_geometry(10, 20, 0), None, "内容不足一屏");
        assert_eq!(scrollbar_geometry(20, 20, 0), None, "恰好一屏也不画");
        assert_eq!(scrollbar_geometry(100, 0, 0), None, "区域高 0");

        // 比例 thumb：content=100,h=20 → round(400/100)=4
        let g = scrollbar_geometry(100, 20, 0).unwrap();
        assert_eq!(g.thumb_height, 4, "比例高度");
        assert_eq!(g.max_scroll_top, 80, "最大可滚距离");
        assert_eq!(g.thumb_top, 0, "顶部在轨道顶");
        let bottom = scrollbar_geometry(100, 20, 80).unwrap();
        assert_eq!(bottom.thumb_top, 20 - 4, "贴底在轨道底端（h−thumb）");
        let mid = scrollbar_geometry(100, 20, 40).unwrap();
        assert_eq!(mid.thumb_top, 8, "中段按比例（40/80 × 16）");

        // 越界 top clamp
        assert_eq!(
            scrollbar_geometry(100, 20, 999).unwrap().thumb_top,
            16,
            "越界 top 按贴底算"
        );

        // 最小 thumb 高 = 2（内容远高于视口）
        let tiny = scrollbar_geometry(1000, 20, 0).unwrap();
        assert_eq!(tiny.thumb_height, 2, "最小拖动条高 2");

        // 勉强一屏多：content=21,h=20 → round(400/21)=19
        let barely = scrollbar_geometry(21, 20, 0).unwrap();
        assert_eq!(barely.thumb_height, 19, "thumb 不超过轨道高");
        assert_eq!(barely.max_scroll_top, 1);
    }

    /// 任务 2.1：内容不足一屏时窗口全量、视为贴底（恢复跟随）。
    #[test]
    fn short_content_pinned_restores_follow() {
        let lines = vec![plain("a"), plain("b")];
        let scroll = ChatScroll {
            content_len: 0,
            top: 7,
            following: false,
            viewport: 0,
        };
        let (out, next, _) = wrap_for_viewport(&lines, 20, 10, &scroll, &[],
        None, &TuiTheme::default());
        assert_eq!(render(&out), "a\nb");
        assert!(next.following, "内容不足一屏即视为贴底");
        assert_eq!(next.top, 0);
    }

    /// 任务 2.1：meta 逻辑→屏行传播 + 连续命中区间（跨多屏行、
    /// 窗口化之前产出、与窗口化输出对齐）。
    #[test]
    fn hits_span_wrapped_lines_and_align_with_window() {
        // 3 逻辑行：None / Some(Tool(c1)，超宽换行成多屏行) / None
        let lines = vec![
            plain("header"),
            plain(&"word ".repeat(30)),
            plain("tail"),
        ];
        let meta = vec![vec![], vec![HitKind::Tool("c1".into())], vec![]];
        let (out, _next, hits) =
            wrap_for_viewport(&lines, 20, 100, &ChatScroll::default(), &meta,
        None, &TuiTheme::default());
        assert_eq!(hits.len(), 1, "应恰一个命中区间: {hits:?}");
        let h = &hits[0];
        assert_eq!(h.kind, HitKind::Tool("c1".into()), "强类型 kind 断言");
        assert_eq!(h.start, 1, "区间起点在 header 之后");
        assert!(h.end - h.start > 1, "展开卡跨多屏行连续: {h:?}");
        // 未滚动（following、内容不足一屏）时窗口 = 全量，绝对号即窗口内号
        assert!(h.end <= out.len(), "区间落在窗口内");
        let first = &out[h.start];
        assert!(
            first.spans.iter().any(|s| s.content.as_ref().starts_with("word")),
            "区间首行与窗口对齐"
        );
        assert!(out[h.end].spans.iter().any(|s| s.content.as_ref() == "tail"),
            "区间末行之后是 None 行");
    }

    /// 任务 2.1：pinned 态下 hits 仍为绝对屏行号（不含窗口偏移）；
    /// 相邻不同 kind 各成一段（强类型分组，design D1）。
    #[test]
    fn hits_are_absolute_when_pinned_and_kind_typed() {
        let lines: Vec<Line> = (0..200).map(|i| plain(&format!("L{i:03}"))).collect();
        let mut meta = vec![Vec::new(); 200];
        meta[80].push(HitKind::Tool("c1".into()));
        meta[81].push(HitKind::Thinking(7));
        let scroll = ChatScroll {
            content_len: 0,
            top: 50,
            following: false,
            viewport: 0,
        };
        let (out, _next, hits) = wrap_for_viewport(&lines, 20, 10, &scroll, &meta,
        None, &TuiTheme::default());
        assert_eq!(out.len(), 10);
        assert_eq!(out[0].spans[0].content.as_ref(), "L050", "窗口从 top 起");
        assert_eq!(hits.len(), 2, "不同 kind 各成一段: {hits:?}");
        assert_eq!(
            (hits[0].start, hits[0].end, hits[0].kind.clone()),
            (80, 81, HitKind::Tool("c1".into())),
            "绝对屏行号，不含窗口偏移（窗口在 [50,60)）"
        );
        assert_eq!(
            (hits[1].start, hits[1].end, hits[1].kind.clone()),
            (81, 82, HitKind::Thinking(7)),
        );
    }
}
