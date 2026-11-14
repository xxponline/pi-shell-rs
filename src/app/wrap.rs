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

/// 将逻辑行按视口宽度换行，并按滚动状态截取视口窗口。
///
/// 输入:
///   lines  — 逻辑行（span 内可含 `\n`，样式随 span 保留）
///   width  — 视口宽度（单元格数，CJK 按 2 计）
///   height — 视口高度（窗口行数上限，至少 1）
///   scroll — 滚动状态：following 取末尾（贴底跟随，即原底部锚定行为）；
///            钉住取 `[top, top+height)`，top 越界时 clamp 到底
///   meta   — 逐逻辑行命中标记组（与 lines 等长；chat_lines 产出：已有
///            结果的工具卡含 Tool(id)、非空 thinking 块含 Thinking(seq)、
///            消息首行含 MessageStart(seq)——每行 Vec 多标记共存，
///            design D1/D2）
///   scrollbar — 滚动条覆盖（None=本帧不画，design D2）
/// 输出:
///   `(窗口行序列, 吸收后的 ChatScroll, Vec<Hit>)`——窗口总行数
///   ≤ height；返回状态已回写 `viewport=height`、`content_len=全量` 并
///   `absorb(...)`；hits 为**窗口化之前**的全量绝对屏行区间（供点击命中）。
///   空逻辑行保留为一个空行，保证段落间距不塌缩。
pub fn wrap_for_viewport(
    lines: &[Line<'static>],
    width: u16,
    height: u16,
    scroll: &ChatScroll,
    meta: &[Vec<HitKind>],
    scrollbar: Option<ScrollbarOverlay>,
) -> (Vec<Line<'static>>, ChatScroll, Vec<Hit>) {
    let width = width.max(1) as usize;
    let height = height.max(1) as usize;

    let mut wrapped: Vec<Line<'static>> = Vec::new();
    // 逐屏行的命中标记组：一条逻辑行的全部子行继承同一组（logical→
    // screen 传播）。
    let mut screen_meta: Vec<Vec<HitKind>> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        // 父行 style（如工具卡片 bg 面板）传播到拆分出的子行（design D5）：
        // Paragraph 只给有文本的单元格上色，行尾还得补 pad。
        let style = line.style;
        let mark = meta.get(i).cloned().unwrap_or_default();
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

    // 窗口化（design D3）：following 取末尾（现底部锚定行为），钉住取
    // [top, top+height)；top 越界（内容骤减）时 clamp 到底。
    let len = wrapped.len();
    let max_top = len.saturating_sub(height);
    let effective_top = if scroll.following {
        max_top
    } else {
        scroll.top.min(max_top)
    };
    // 贴底判定：窗口已覆盖到最后一行（含内容不足一屏、空内容）。
    let at_bottom = effective_top + height >= len;

    if effective_top > 0 {
        wrapped.drain(..effective_top);
    }
    wrapped.truncate(height);

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
                SCROLLBAR_THUMB_COLOR
            } else {
                SCROLLBAR_TRACK_COLOR
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
    // 内容总行数回写（drain 前全量，滚动条几何用，design D1）。
    next.content_len = len;
    next.absorb(effective_top, at_bottom);
    (wrapped, next, hits)
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
        None).0;
        assert_eq!(render(&out), "hello\nworld");
    }

    #[test]
    fn long_line_wraps_at_word_boundary() {
        let lines = vec![plain(&"word ".repeat(50))];
        let out = wrap_for_viewport(&lines, 20, 100, &ChatScroll::default(), &[],
        None).0;
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
        None).0;
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
        None).0;
        assert_eq!(render(&out), "line1\nline2\nline3");
    }

    #[test]
    fn empty_logical_line_preserved() {
        let lines = vec![plain("a"), plain(""), plain("b")];
        let out = wrap_for_viewport(&lines, 40, 10, &ChatScroll::default(), &[],
        None).0;
        assert_eq!(render(&out), "a\n\nb");
    }

    #[test]
    fn cjk_width_counts_double() {
        // 10 个中文字 = 20 宽，宽度 10 → 每行 5 个
        let lines = vec![plain(&"中".repeat(10))];
        let out = wrap_for_viewport(&lines, 10, 100, &ChatScroll::default(), &[],
        None).0;
        assert_eq!(out.len(), 2);
        assert_eq!(render(&out), "中中中中中\n中中中中中");
    }

    #[test]
    fn unbroken_long_token_hard_splits() {
        let lines = vec![plain(&"x".repeat(45))];
        let out = wrap_for_viewport(&lines, 10, 100, &ChatScroll::default(), &[],
        None).0;
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
        None).0;
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
        None).0;
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
        None).0;
        assert_eq!(out.len(), 5);
        for l in &out {
            assert_eq!(l.style.bg, Some(ratatui::style::Color::Rgb(40, 44, 52)), "硬切子行保留 bg");
            let w: usize = l.spans.iter().map(|s| s.content.as_ref().width()).sum();
            assert_eq!(w, 10, "硬切行补 pad 到视口宽");
        }

        // 无 bg 的行不补 pad（不改变既有行为）
        let lines = vec![plain("hi")];
        let out = wrap_for_viewport(&lines, 20, 10, &ChatScroll::default(), &[],
        None).0;
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
        None);
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
        None);
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
        None);
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
        None);
        assert_eq!(next.content_len, 50, "总行数 = drain 前全量");
        assert_eq!(next.top, 40, "following 取末尾");

        let pinned = ChatScroll {
            top: 10,
            following: false,
            viewport: 0,
            content_len: 0,
        };
        let (_out2, next2, _) = wrap_for_viewport(&lines, 20, 5, &pinned, &[],
        None);
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
            Some(ScrollbarOverlay { hovered: false }),
        );
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
            Some(ScrollbarOverlay { hovered: true }),
        );
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
            Some(ScrollbarOverlay { hovered: false }),
        );
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
            Some(ScrollbarOverlay { hovered: false }),
        );
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
            Some(ScrollbarOverlay { hovered: false }),
        );
        let st: String = s_out[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(st, "only", "内容不足不画");
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
            None,
        );
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
        let (_o2, _n2, hits2) = wrap_for_viewport(&lines, 20, 5, &scroll, &meta, None);
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
        None);
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
        None);
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
        None);
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
