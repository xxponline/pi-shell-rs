//! 鼠标选区纯逻辑（add-select-to-copy；无 TTY、可单测）。
//!
//! 坐标 =（绝对屏行, 显示列），与命中表同空间（design D1）。词分段、
//! 连击与粒度延展语义对齐 pi `tui-alt-screen`（`getWordSelection` /
//! `getClickCount` / `updateSelectionFocus`）。

use std::time::{Duration, Instant};

use ratatui::text::Line;
use unicode_width::UnicodeWidthChar;

/// 选区粒度（design D5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionGranularity {
    /// 字符（默认，逐点扩展）。
    Character,
    /// 词（双击；含连接符贯通）。
    Word,
    /// 行（三击）。
    Line,
}

/// 一次进行中的选区（design D1）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionState {
    /// 锚点（绝对屏行, 显示列）——按下时定。
    pub anchor: (usize, u16),
    /// 焦点（拖动跟随）。
    pub focus: (usize, u16),
    /// 粒度（连击确定后锁定，design D5）。
    pub granularity: SelectionGranularity,
    /// 词/行初选范围（拖拽延展的枢轴，pi `initialRange` 同构）。
    pub initial_range: Option<((usize, u16), (usize, u16))>,
    /// 是否发生过拖动（拖动打断连击链）。
    pub dragged: bool,
    /// 边缘自动滚动方向（-1 上 / 0 无 / 1 下；design D5）。
    pub auto_scroll: i8,
    /// 最近一次拖拽指针（原始屏幕坐标；边缘自动滚每拍重算用，pi
    /// `selectionDragPointer` 同构）。
    pub pointer: (u16, u16),
}

impl SelectionState {
    /// 以锚点构造字符粒度选区（按下即用）。
    ///
    /// 输入: anchor —（绝对屏行, 显示列）；pointer — 原始指针屏幕坐标
    /// 输出: 粒度 Character、无初选、未拖动、无自动滚的选区状态
    pub fn new(anchor: (usize, u16), pointer: (u16, u16)) -> Self {
        Self {
            anchor,
            focus: anchor,
            granularity: SelectionGranularity::Character,
            initial_range: None,
            dragged: false,
            auto_scroll: 0,
            pointer,
        }
    }
}

/// 连击记录（App 级、比选区寿命长——纯点击清选区后仍需计数，design D5）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastClick {
    /// 上一击时刻。
    pub at: Instant,
    /// 所在绝对屏行。
    pub row: usize,
    /// 命中的词界（显示列区间）。
    pub word: (u16, u16),
    /// 连击序号（1 起）。
    pub count: u32,
}

/// 连击窗口（抄 pi `DOUBLE_CLICK_INTERVAL_MS = 500`）。
pub const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(500);

/// 词连接符集合（抄 pi `TERMINAL_WORD_SELECTION_JOINERS`——路径整体可选）。
const WORD_JOINERS: [char; 7] = ['/', '-', '+', '\\', '~', '_', '.'];

/// 边界排序（行优先字典序）；同点 = None（无选区，pi
/// `getSelectionBounds` 同义）。
///
/// 输入: anchor/focus —（绝对屏行, 显示列）
/// 输出: Some((较小端, 较大端))；两点相同 → None
pub fn selection_bounds(
    anchor: (usize, u16),
    focus: (usize, u16),
) -> Option<((usize, u16), (usize, u16))> {
    if anchor == focus {
        return None;
    }
    Some(if anchor <= focus { (anchor, focus) } else { (focus, anchor) })
}

/// 排序并把**终点列转为 exclusive**（渲染/提取共用，design D2/D3）。
///
/// 语义差：Character 的焦点 = 指针所在格（**含**该格 → +1）；Word/Line 的
/// 初选终点本就是段末边界（exclusive → 不加）。
///
/// 输入: sel — 进行中的选区
/// 输出: Some((起, 止 exclusive))；空选区 → None
pub fn selection_exclusive_bounds(sel: &SelectionState) -> Option<((usize, u16), (usize, u16))> {
    let ((sr, sc), (er, ec)) = selection_bounds(sel.anchor, sel.focus)?;
    let ec = match sel.granularity {
        SelectionGranularity::Character => ec.saturating_add(1),
        _ => ec,
    };
    Some(((sr, sc), (er, ec)))
}

/// 按显示列切出行文本（`[start, end)`；边界吸附到字符——起点回退、终点
/// 前进，宽字符整字不劈；design D2）。
///
/// 输入: line — 源行；start/end — 显示列区间（end 专属）
/// 输出: 区间内字符拼接的纯文本（不含样式）
pub fn slice_line_cols(line: &Line<'_>, start: u16, end: u16) -> String {
    if end <= start {
        return String::new();
    }
    // 逐字符收集（起列, 宽度, 字符）
    let mut chars: Vec<(u16, u16, char)> = Vec::new();
    let mut col = 0u16;
    for span in &line.spans {
        for ch in span.content.as_ref().chars() {
            let w = ch.width().unwrap_or(0) as u16;
            chars.push((col, w, ch));
            col = col.saturating_add(w);
        }
    }
    // 吸附：起点落在宽字符中间 → 回退到该字起点；终点落在中间 → 前进到
    // 该字终点（pi `getGraphemeCellRange` 同义——不劈字）。
    let mut s = start;
    let mut e = end;
    for &(cs, w, _) in &chars {
        if w > 0 && cs < s && s < cs + w {
            s = cs;
            break;
        }
    }
    for &(cs, w, _) in chars.iter().rev() {
        if w > 0 && cs < e && e < cs + w {
            e = cs + w;
            break;
        }
    }
    let mut out = String::new();
    let mut prev_taken = false;
    for &(cs, w, ch) in &chars {
        let taken = if w == 0 {
            prev_taken // 零宽（组合符）跟随前一字
        } else {
            cs >= s && cs + w <= e
        };
        if taken {
            out.push(ch);
        }
        if w > 0 {
            prev_taken = taken;
        }
    }
    out
}

/// 行显示宽度（列上限用，Line granularity 的 end 列）。
///
/// 输入: line — 源行
/// 输出: 显示列总数
pub fn line_display_width(line: &Line<'_>) -> u16 {
    line.spans
        .iter()
        .map(|s| s.content.as_ref().chars().map(|c| c.width().unwrap_or(0) as u16).sum::<u16>())
        .sum()
}

/// 行文本（拼接 span，剥样式；词分段与提取用）。
///
/// 输入: line — 源行
/// 输出: 纯文本
pub fn line_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect::<Vec<_>>().join("")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SegKind {
    /// 词段（字母/数字/CJK…可选）。
    Word,
    /// 连接符段（WORD_JOINERS，可选且可贯通）。
    Joiner,
    /// 分隔（空白/其它标点，不可选）。
    Sep,
}

impl SegKind {
    fn selectable(self) -> bool {
        !matches!(self, SegKind::Sep)
    }
    fn joiner(self) -> bool {
        matches!(self, SegKind::Joiner)
    }
}

fn seg_kind(ch: char) -> SegKind {
    if ch.is_whitespace() {
        SegKind::Sep
    } else if WORD_JOINERS.contains(&ch) {
        SegKind::Joiner
    } else if ch.is_alphanumeric() {
        SegKind::Word
    } else {
        SegKind::Sep // 其它标点不可选（pi Intl 分词中非 word-like）
    }
}

/// 找出显示列 `col` 所在的词界（含连接符贯通，pi `getWordSelection` 同式：
/// 贯通条件 = 两侧可选且至少一侧为连接符）。
///
/// 输入: text — 行纯文本；col — 显示列
/// 输出: Some((起列, 末列 exclusive))；空白/不可选段或空行 → None
pub fn word_range_at(text: &str, col: u16) -> Option<(u16, u16)> {
    // 分段：连续同类合并（词 run 成段、连接符成段、分隔成段）
    let mut segs: Vec<(u16, u16, SegKind)> = Vec::new();
    let mut cur: Option<(u16, u16, SegKind)> = None;
    let mut pos = 0u16;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0) as u16;
        let kind = seg_kind(ch);
        match &mut cur {
            Some((s, e, k)) if *k == kind => *e = pos + w,
            Some(slot) => {
                segs.push(*slot);
                *slot = (pos, pos + w, kind);
            }
            None => cur = Some((pos, pos + w, kind)),
        }
        pos = pos.saturating_add(w);
    }
    if let Some(slot) = cur {
        segs.push(slot);
    }

    let idx = segs.iter().position(|(s, e, _)| col >= *s && col < *e)?;
    let (_s, _e, kind) = segs[idx];
    if !kind.selectable() {
        return None;
    }
    // pi canJoin：两侧可选且至少一侧为连接符
    let can_join = |l: SegKind, r: SegKind| l.selectable() && r.selectable() && (l.joiner() || r.joiner());
    let mut start = segs[idx].0;
    let mut end = segs[idx].1;
    let mut i = idx;
    while i > 0 && can_join(segs[i - 1].2, segs[i].2) {
        i -= 1;
        start = segs[i].0;
    }
    let mut j = idx;
    while j + 1 < segs.len() && can_join(segs[j].2, segs[j + 1].2) {
        j += 1;
        end = segs[j].1;
    }
    Some((start, end))
}

/// 连击 → 粒度与新连击记录（抄 pi `getClickCount`：500ms 窗、同行同词界
/// 才连击，count%3+1 循环；词未命中则重置）。
///
/// 输入: prev — 上一击记录（可空）；row — 当前行；word — 当前命中词界
///       （可空）；now — 当前时刻
/// 输出: (粒度, 新的连击记录——word 为 None 时记录清空)
pub fn click_granularity(
    prev: Option<&LastClick>,
    row: usize,
    word: Option<(u16, u16)>,
    now: Instant,
) -> (SelectionGranularity, Option<LastClick>) {
    let count = match (prev, word) {
        (Some(p), Some(w))
            if now.duration_since(p.at) <= DOUBLE_CLICK_INTERVAL
                && p.row == row
                && p.word == w =>
        {
            (p.count % 3) + 1
        }
        _ => 1,
    };
    let last = word.map(|w| LastClick { at: now, row, word: w, count });
    let granularity = match count {
        1 => SelectionGranularity::Character,
        2 => SelectionGranularity::Word,
        _ => SelectionGranularity::Line,
    };
    (granularity, last)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str) -> Line<'static> {
        Line::from(ratatui::text::Span::raw(text.to_string()))
    }

    /// 任务 1.1：边界排序与列切分（design D1/D2）。
    #[test]
    fn selection_bounds_orders_and_empty_is_none() {
        // 排序：行优先字典序
        assert_eq!(
            selection_bounds((3, 5), (1, 9)),
            Some(((1, 9), (3, 5))),
            "反序翻转"
        );
        assert_eq!(selection_bounds((2, 1), (2, 8)), Some(((2, 1), (2, 8))));
        assert_eq!(
            selection_bounds((5, 0), (1, 99)),
            Some(((1, 99), (5, 0))),
            "跨行跨列排序"
        );
        // 同点 = 无选区
        assert_eq!(selection_bounds((4, 7), (4, 7)), None, "同点 None");

        // 列切分：ASCII 区间
        let l = line("hello world");
        assert_eq!(slice_line_cols(&l, 0, 5), "hello");
        assert_eq!(slice_line_cols(&l, 6, 11), "world");
        assert_eq!(slice_line_cols(&l, 3, 3), "", "空区间");
        assert_eq!(slice_line_cols(&l, 9, 4), "", "倒置区间");

        // 宽字符不劈：起点落在字中间 → 回退整字纳入；终点落在中间 → 前进
        // （拖拽触到字内即整字选中——吸附语义）
        let cjk = line("中文字abc"); // 中(0-2) 中(2-4) 字(4-6) a(6-7)...
        assert_eq!(slice_line_cols(&cjk, 1, 5), "中文字", "两端吸附（触字即整字）");
        assert_eq!(slice_line_cols(&cjk, 3, 7), "文字a", "起点吸附至文首、终点含 a（触字即整字）");
        assert_eq!(slice_line_cols(&cjk, 0, 6), "中文字", "整字边界");
        assert_eq!(slice_line_cols(&cjk, 6, 9), "abc", "ASCII 段");

        // 多 span 行（样式分段不影响列）
        let multi = Line::from(vec![
            ratatui::text::Span::raw("ab"),
            ratatui::text::Span::raw("cd"),
        ]);
        assert_eq!(slice_line_cols(&multi, 1, 3), "bc", "跨 span 切分");

        // 宽度函数
        assert_eq!(line_display_width(&cjk), 9, "中文字abc = 3×2+3");
        assert_eq!(line_display_width(&line("hi")), 2);
    }

    /// 任务 1.2：词分段与连击粒度（pi getWordSelection/getClickCount 同式，
    /// design D5）。
    #[test]
    fn word_line_granularity_matches_pi() {
        // 连接符贯通：点 bar → 整个 foo-bar；点 foo 同
        assert_eq!(word_range_at("foo-bar baz", 5), Some((0, 7)), "点 bar 含连接符左扩");
        assert_eq!(word_range_at("foo-bar baz", 1), Some((0, 7)), "点 foo 右扩贯通");
        // 非连接符标点断开：foo,bar → 点 foo 只到逗号前
        assert_eq!(word_range_at("foo,bar", 1), Some((0, 3)), "逗号不可选断开");
        // 路径整体可选（pi joiner 集合）
        assert_eq!(word_range_at("/a/b.txt", 5), Some((0, 8)), "路径整体");
        assert_eq!(word_range_at("src/app/main.rs", 4), Some((0, 15)), "长路径整体");
        // 空白不可选
        assert_eq!(word_range_at("foo bar", 3), None, "空白 None");
        assert_eq!(word_range_at("", 0), None, "空行 None");
        // CJK 词
        assert_eq!(word_range_at("中文测试", 2), Some((0, 8)), "CJK 整词");
        // 显示列语义：中文后的 ASCII（中=2 列）——连续字母数字（含 CJK）
        // 合并为一词：与 pi 的 ICU 字典分词在 CJK/拉丁混合边界上允许差异
        // （设计记录的简化——规格未钉边界细分）
        assert_eq!(word_range_at("中ab", 2), Some((0, 4)), "连续字母数字整段为词");

        // 连击：同词连击 1→2→3→1 循环
        let t0 = Instant::now();
        let (g1, l1) = click_granularity(None, 5, Some((0, 3)), t0);
        assert_eq!(g1, SelectionGranularity::Character, "首击 = 字符");
        let (g2, l2) = click_granularity(l1.as_ref(), 5, Some((0, 3)), t0 + Duration::from_millis(100));
        assert_eq!(g2, SelectionGranularity::Word, "二击 = 词");
        let (g3, l3) = click_granularity(l2.as_ref(), 5, Some((0, 3)), t0 + Duration::from_millis(200));
        assert_eq!(g3, SelectionGranularity::Line, "三击 = 行");
        let (g4, _) = click_granularity(l3.as_ref(), 5, Some((0, 3)), t0 + Duration::from_millis(300));
        assert_eq!(g4, SelectionGranularity::Character, "四击回字符（%3+1 循环）");

        // 超窗重置 / 换行重置 / 词未命中重置
        let (_, l_stale) = click_granularity(None, 5, Some((0, 3)), t0);
        let (g_timeout, _) = click_granularity(
            l_stale.as_ref(),
            5,
            Some((0, 3)),
            t0 + Duration::from_millis(501),
        );
        assert_eq!(g_timeout, SelectionGranularity::Character, "超 500ms 重置");
        let (g_row, _) = click_granularity(l_stale.as_ref(), 6, Some((0, 3)), t0 + Duration::from_millis(100));
        assert_eq!(g_row, SelectionGranularity::Character, "换行重置");
        let (g_noword, l_noword) = click_granularity(l_stale.as_ref(), 5, None, t0 + Duration::from_millis(100));
        assert_eq!(g_noword, SelectionGranularity::Character, "词未命中重置");
        assert!(l_noword.is_none(), "词未命中时连击记录清空");
    }
}
