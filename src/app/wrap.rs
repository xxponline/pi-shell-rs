//! 视口换行与底部锚定（纯函数，无终端依赖）。
//!
//! 背景：ratatui 的 `Paragraph` 在开启 `Wrap` 时，`scroll.y` 按**换行后的
//! 屏幕行**计数，而我们无法从公开 API 拿到精确的换行行数，导致偏移不足、
//! 最新内容被挤出视口。这里改为：自己贪心换行到视口宽度，再只保留末尾
//! `height` 行（底部锚定），渲染时不再使用 `Wrap`/`scroll`。

use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// 将逻辑行按视口宽度换行，并截取末尾 `height` 行（底部锚定）。
///
/// 输入:
///   lines  — 逻辑行（span 内可含 `\n`，样式随 span 保留）
///   width  — 视口宽度（单元格数，CJK 按 2 计）
///   height — 视口高度（保留的换行后行数上限）
/// 输出:
///   换行后的行序列；总行数 ≤ height，且**尾部**（最新内容）一定保留。
///   空逻辑行保留为一个空行，保证段落间距不塌缩。
pub fn wrap_for_viewport(
    lines: &[Line<'static>],
    width: u16,
    height: u16,
) -> Vec<Line<'static>> {
    let width = width.max(1) as usize;
    let height = height.max(1) as usize;

    let mut wrapped: Vec<Line<'static>> = Vec::new();
    for line in lines {
        // 父行 style（如工具卡片 bg 面板）传播到拆分出的子行（design D5）：
        // Paragraph 只给有文本的单元格上色，行尾还得补 pad。
        let style = line.style;
        let start = wrapped.len();
        for sub in split_on_newlines(line) {
            wrap_one(&sub, width, &mut wrapped);
        }
        for l in &mut wrapped[start..] {
            l.style = style;
        }
    }

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

    if wrapped.len() > height {
        let cut = wrapped.len() - height;
        wrapped.drain(..cut);
    }
    wrapped
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
        let out = wrap_for_viewport(&lines, 40, 10);
        assert_eq!(render(&out), "hello\nworld");
    }

    #[test]
    fn long_line_wraps_at_word_boundary() {
        let lines = vec![plain(&"word ".repeat(50))];
        let out = wrap_for_viewport(&lines, 20, 100);
        assert!(out.len() > 1, "should wrap");
        for l in &out {
            let w: usize = l.spans.iter().map(|s| s.content.as_ref().width()).sum();
            assert!(w <= 20, "line too wide: {w}");
        }
    }

    #[test]
    fn over_height_keeps_tail_not_head() {
        let lines = vec![plain(&format!("{}TAIL", "word ".repeat(500)))];
        let out = wrap_for_viewport(&lines, 20, 10);
        assert_eq!(out.len(), 10);
        let text = render(&out);
        assert!(text.trim_end().ends_with("TAIL"), "tail should be newest content: {text:?}");
        // 头部（最早的行）被丢弃
        assert!(!text.starts_with("word word word word word word word word word word word "));
    }

    #[test]
    fn embedded_newline_creates_lines() {
        let lines = vec![plain("line1\nline2\nline3")];
        let out = wrap_for_viewport(&lines, 40, 10);
        assert_eq!(render(&out), "line1\nline2\nline3");
    }

    #[test]
    fn empty_logical_line_preserved() {
        let lines = vec![plain("a"), plain(""), plain("b")];
        let out = wrap_for_viewport(&lines, 40, 10);
        assert_eq!(render(&out), "a\n\nb");
    }

    #[test]
    fn cjk_width_counts_double() {
        // 10 个中文字 = 20 宽，宽度 10 → 每行 5 个
        let lines = vec![plain(&"中".repeat(10))];
        let out = wrap_for_viewport(&lines, 10, 100);
        assert_eq!(out.len(), 2);
        assert_eq!(render(&out), "中中中中中\n中中中中中");
    }

    #[test]
    fn unbroken_long_token_hard_splits() {
        let lines = vec![plain(&"x".repeat(45))];
        let out = wrap_for_viewport(&lines, 10, 100);
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
        let out = wrap_for_viewport(&lines, 10, 100);
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
        let out = wrap_for_viewport(&lines, 20, 100);
        assert!(out.len() > 1, "应拆行: {}", out.len());
        for l in &out {
            assert_eq!(l.style.bg, Some(ratatui::style::Color::Rgb(40, 44, 52)), "子行保留 bg");
            let w: usize = l.spans.iter().map(|s| s.content.as_ref().width()).sum();
            assert_eq!(w, 20, "行尾补 pad 到视口宽");
            assert!(l.spans.last().unwrap().content.as_ref().ends_with(' ') || w == 0);
        }

        // 硬切路径：超宽单 token
        let lines = vec![Line::from(Span::styled("x".repeat(45), bg)).style(bg)];
        let out = wrap_for_viewport(&lines, 10, 100);
        assert_eq!(out.len(), 5);
        for l in &out {
            assert_eq!(l.style.bg, Some(ratatui::style::Color::Rgb(40, 44, 52)), "硬切子行保留 bg");
            let w: usize = l.spans.iter().map(|s| s.content.as_ref().width()).sum();
            assert_eq!(w, 10, "硬切行补 pad 到视口宽");
        }

        // 无 bg 的行不补 pad（不改变既有行为）
        let lines = vec![plain("hi")];
        let out = wrap_for_viewport(&lines, 20, 10);
        assert_eq!(render(&out), "hi");
    }
}
