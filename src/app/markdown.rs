//! markdown → 带样式 ratatui 行（纯函数 + 线程级缓存）。
//!
//! 管线：pulldown-cmark 事件流 → 逐事件映射为 styled span/line。
//! 渲染结果与宽度无关（宽度由 wrap 层处理），因此缓存键只需内容哈希；
//! 语法高亮单独按 (lang, code) 缓存。畸形/未闭合输入交给 pulldown-cmark
//! 的安全解析，本层只负责不 panic。

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

use pulldown_cmark::{
    CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// markdown 解析结果缓存容量（超过即整体清空，简单防膨胀）。
const MD_CACHE_CAP: usize = 128;
/// 单个代码块高亮结果缓存条数上限。
const HL_CACHE_CAP: usize = 64;

thread_local! {
    /// 内容哈希 → 渲染结果；历史消息每帧重复渲染时命中。
    static MD_CACHE: RefCell<HashMap<u64, Vec<Line<'static>>>> =
        RefCell::new(HashMap::new());
    /// (语言, 代码) → 高亮 span 行。
    static HL_CACHE: RefCell<HashMap<(String, String), Vec<Line<'static>>>> =
        RefCell::new(HashMap::new());
}

/// syntect 语法集/主题（进程内只加载一次）。
fn syntax_set() -> &'static syntect::parsing::SyntaxSet {
    static SS: OnceLock<syntect::parsing::SyntaxSet> = OnceLock::new();
    SS.get_or_init(syntect::parsing::SyntaxSet::load_defaults_newlines)
}

fn theme() -> &'static syntect::highlighting::Theme {
    static T: OnceLock<syntect::highlighting::Theme> = OnceLock::new();
    T.get_or_init(|| {
        syntect::highlighting::ThemeSet::load_defaults()
            .themes
            .remove("base16-ocean.dark")
            .expect("bundled theme base16-ocean.dark")
    })
}

/// FNV-1a 64 位内容哈希（缓存键，不需要密码学强度）。
///
/// 输入: text — 任意 markdown 源文本
/// 输出: 64 位哈希值
fn fnv1a(text: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// 把 markdown 文本渲染为带样式的 ratatui 行（带缓存）。
///
/// 输入: text — assistant/user 的 markdown 源文本（任意，含畸形输入）
/// 输出: Vec<Line<'static>>；相同内容返回相同结果，绝不 panic
pub fn render_markdown(text: &str) -> Vec<Line<'static>> {
    let hash = fnv1a(text);
    if let Some(hit) = MD_CACHE.with(|c| c.borrow().get(&hash).cloned()) {
        return hit;
    }
    let lines = parse_to_lines(text);
    MD_CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        if cache.len() >= MD_CACHE_CAP {
            cache.clear();
        }
        cache.insert(hash, lines.clone());
    });
    lines
}

/// 解析入口（无缓存的实现，供测试直接调用）。
///
/// 输入: text — markdown 源文本
/// 输出: 渲染后的行序列
fn parse_to_lines(text: &str) -> Vec<Line<'static>> {
    let opts = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let parser = Parser::new_ext(text, opts);

    let mut out: Vec<Line<'static>> = Vec::new();
    let mut cur: Vec<Span<'static>> = Vec::new();
    // 内联样式栈（粗体/斜体等在 End 时弹出）。
    let mut style_stack: Vec<Style> = vec![Style::default()];
    // 代码块收集态：(语言, 正文)。
    let mut code: Option<(String, String)> = None;
    // 列表栈：(是否有序, 计数)。
    let mut list_stack: Vec<(bool, u64)> = Vec::new();
    // 当前引用深度（行首 "> " 前缀）。
    let mut quote_depth: usize = 0;
    // 下一个文本前要加的列表标记（Item 开始时设置）。
    let mut pending_marker: Option<String> = None;
    // 链接目标（End(Link) 时追加 "(url)"）。
    let mut pending_link: Option<String> = None;

    // 冲刷当前行为一行（引用块在行首补 "> " 前缀）。
    macro_rules! flush_line {
        () => {
            if !cur.is_empty() {
                let mut spans = std::mem::take(&mut cur);
                if quote_depth > 0 {
                    let prefix = "> ".repeat(quote_depth);
                    let mut prefixed = vec![Span::styled(
                        prefix,
                        Style::default().fg(Color::DarkGray),
                    )];
                    prefixed.append(&mut spans);
                    out.push(Line::from(prefixed));
                } else {
                    out.push(Line::from(spans));
                }
            }
        };
    }
    // 块级空行（只在必要时补，避免连续空行）。
    macro_rules! blank_line {
        () => {
            if out.last().map(|l| !l.spans.is_empty()).unwrap_or(false) {
                out.push(Line::default());
            }
        };
    }

    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {}
                Tag::Heading { level, .. } => {
                    style_stack.push(
                        style_stack.last().copied().unwrap_or_default()
                            .add_modifier(Modifier::BOLD)
                            .fg(heading_color(level)),
                    );
                }
                Tag::BlockQuote(_) => quote_depth += 1,
                Tag::CodeBlock(kind) => {
                    let lang = match kind {
                        CodeBlockKind::Fenced(lang) => lang.to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    code = Some((lang, String::new()));
                }
                Tag::List(ordered) => {
                    let start = ordered.unwrap_or(1);
                    list_stack.push((ordered.is_some(), start.saturating_sub(1)));
                }
                Tag::Item => {
                    let (ordered, counter) = list_stack.last_mut().map(|(o, c)| {
                        *c += 1;
                        (*o, *c)
                    }).unwrap_or((false, 1));
                    let indent = "  ".repeat(list_stack.len().saturating_sub(1));
                    pending_marker = Some(if ordered {
                        format!("{indent}{counter}. ")
                    } else {
                        format!("{indent}• ")
                    });
                }
                Tag::Emphasis => {
                    style_stack.push(
                        style_stack.last().copied().unwrap_or_default()
                            .add_modifier(Modifier::ITALIC),
                    );
                }
                Tag::Strong => {
                    style_stack.push(
                        style_stack.last().copied().unwrap_or_default()
                            .add_modifier(Modifier::BOLD),
                    );
                }
                Tag::Strikethrough => {
                    style_stack.push(
                        style_stack.last().copied().unwrap_or_default()
                            .add_modifier(Modifier::CROSSED_OUT),
                    );
                }
                Tag::Link { dest_url, .. } => {
                    pending_link = Some(dest_url.to_string());
                    style_stack.push(
                        style_stack.last().copied().unwrap_or_default()
                            .fg(Color::Blue)
                            .add_modifier(Modifier::UNDERLINED),
                    );
                }
                Tag::Image { dest_url, .. } => {
                    // 图片本体不显示，alt 文本照常流出；结束后补目标。
                    pending_link = Some(format!("(image: {dest_url})"));
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph | TagEnd::Heading(..) => {
                    flush_line!();
                    blank_line!();
                }
                TagEnd::BlockQuote(..) => {
                    flush_line!();
                    quote_depth = quote_depth.saturating_sub(1);
                    blank_line!();
                }
                TagEnd::CodeBlock => {
                    if let Some((lang, body)) = code.take() {
                        push_code_block(&mut out, &lang, &body);
                        blank_line!();
                    }
                }
                TagEnd::List(..) => {
                    flush_line!();
                    list_stack.pop();
                    blank_line!();
                }
                TagEnd::Item => flush_line!(),
                TagEnd::Emphasis
                | TagEnd::Strong
                | TagEnd::Strikethrough
                | TagEnd::Link
                | TagEnd::Image => {
                    if let Some(url) = pending_link.take() {
                        let text = cur
                            .iter()
                            .map(|s| s.content.as_ref())
                            .collect::<String>();
                        // 目标与显示文本不同才追加，避免 "example (example)"。
                        if !url.is_empty() && url != text && !url.starts_with("(image:") {
                            cur.push(Span::styled(
                                format!(" ({url})"),
                                Style::default().fg(Color::DarkGray),
                            ));
                        } else if url.starts_with("(image:") {
                            cur.push(Span::styled(
                                format!(" {url}"),
                                Style::default().fg(Color::DarkGray),
                            ));
                        }
                    }
                    style_stack.pop();
                }
                _ => {}
            },
            Event::Text(t) => {
                if let Some((_, body)) = code.as_mut() {
                    body.push_str(&t);
                } else {
                    let style = style_stack.last().copied().unwrap_or_default();
                    if let Some(marker) = pending_marker.take() {
                        cur.push(Span::styled(
                            marker,
                            Style::default().fg(Color::DarkGray),
                        ));
                    }
                    cur.push(Span::styled(t.to_string(), style));
                }
            }
            Event::Code(t) => {
                // 行内代码：黄色，独立于外层样式。
                if let Some(marker) = pending_marker.take() {
                    cur.push(Span::styled(marker, Style::default().fg(Color::DarkGray)));
                }
                cur.push(Span::styled(
                    t.to_string(),
                    Style::default().fg(Color::Yellow),
                ));
            }
            Event::SoftBreak | Event::HardBreak => {
                flush_line!();
            }
            Event::Rule => {
                flush_line!();
                out.push(Line::from(Span::styled(
                    "─".repeat(24),
                    Style::default().fg(Color::DarkGray),
                )));
            }
            Event::TaskListMarker(checked) => {
                let mark = if checked { "[x] " } else { "[ ] " };
                cur.push(Span::styled(mark, Style::default().fg(Color::Cyan)));
            }
            Event::FootnoteReference(r) => {
                cur.push(Span::styled(
                    format!("[^{r}]"),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            Event::InlineMath(m) | Event::DisplayMath(m) => {
                // 数学公式不在本阶段范围（非目标），降级为暗色原文展示。
                cur.push(Span::styled(
                    m.to_string(),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            Event::Html(h) | Event::InlineHtml(h) => {
                // 原始 HTML 降级为暗色文本展示（不执行、不解析）。
                cur.push(Span::styled(
                    h.to_string(),
                    Style::default().fg(Color::DarkGray),
                ));
            }
        }
    }
    // 尾部冲刷（未闭合段落/代码块）。
    flush_line!();
    if let Some((lang, body)) = code.take() {
        push_code_block(&mut out, &lang, &body);
    }
    out
}

/// 计算标题颜色（H1/H2 用青色，其余默认色）。
///
/// 输入: level — 标题级别
/// 输出: 对应颜色
fn heading_color(level: HeadingLevel) -> Color {
    match level {
        HeadingLevel::H1 | HeadingLevel::H2 => Color::Cyan,
        _ => Color::Reset,
    }
}

/// 渲染一个代码块（顶栏语言标签 + 高亮正文 + 底栏）。
///
/// 输入: out — 结果收集器；lang — 语言标签（可空）；body — 代码原文
/// 输出: 无返回值；副作用为向 out 追加代码块行
fn push_code_block(out: &mut Vec<Line<'static>>, lang: &str, body: &str) {
    let label = if lang.is_empty() { "code" } else { lang };
    out.push(Line::from(Span::styled(
        format!("┌─ {label} "),
        Style::default().fg(Color::DarkGray),
    )));
    for line in highlight_cached(lang, body) {
        out.push(line);
    }
    out.push(Line::from(Span::styled(
        "└─",
        Style::default().fg(Color::DarkGray),
    )));
}

/// 按语言对代码做语法高亮（带缓存；未知语言/失败降级为纯文本行）。
///
/// 输入: lang — 语言标签；body — 代码原文
/// 输出: 高亮后的行序列（每源行一行，tab 已展开为 4 空格）
fn highlight_cached(lang: &str, body: &str) -> Vec<Line<'static>> {
    let key = (lang.to_string(), body.to_string());
    if let Some(hit) = HL_CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return hit;
    }
    let lines = highlight_uncached(lang, body);
    HL_CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        if cache.len() >= HL_CACHE_CAP {
            cache.clear();
        }
        cache.insert(key, lines.clone());
    });
    lines
}

/// 高亮实现（无缓存）：syntect 找语法 → 逐行着色；任何失败降级纯文本。
///
/// 输入: lang — 语言标签；body — 代码原文
/// 输出: 高亮或降级后的行序列
fn highlight_uncached(lang: &str, body: &str) -> Vec<Line<'static>> {
    let normalized = body.replace('\t', "    ");
    let ss = syntax_set();
    let syntax = ss
        .find_syntax_by_token(lang)
        .or_else(|| ss.find_syntax_by_extension(lang))
        .filter(|_| !lang.is_empty());

    let Some(syntax) = syntax else {
        // 未知语言：整块纯文本（暗色边框外不加样式）。
        return normalized
            .lines()
            .map(|l| Line::from(Span::raw(l.to_string())))
            .collect();
    };

    let mut highlighter = syntect::easy::HighlightLines::new(syntax, theme());
    let mut out = Vec::new();
    for line in syntect::util::LinesWithEndings::from(&normalized) {
        let spans = match highlighter.highlight_line(line, ss) {
            Ok(ranges) => {
                let mut spans: Vec<Span<'static>> = Vec::new();
                for (style, text) in ranges {
                    let text = text.trim_end_matches(['\n', '\r']);
                    if text.is_empty() {
                        continue;
                    }
                    spans.push(Span::styled(
                        text.to_string(),
                        Style::default()
                            .fg(Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b)),
                    ));
                }
                spans
            }
            Err(_) => vec![Span::raw(line.trim_end_matches(['\n', '\r']).to_string())],
        };
        // 空行也要占位，保持行数与源一致。
        if spans.is_empty() {
            out.push(Line::default());
        } else {
            out.push(Line::from(spans));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 提取行文本（拼接 span 内容）。
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

    /// 收集带某修饰符/颜色的 span 文本。
    fn styled_with(lines: &[Line<'static>], pred: impl Fn(&Style) -> bool) -> Vec<String> {
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| pred(&s.style))
            .map(|s| s.content.to_string())
            .collect()
    }

    #[test]
    fn structure_elements_render_with_styles() {
        let md = "# Title\n\nSome **bold** and *italic* and `code`.\n\n- item one\n- item two";
        let lines = render_markdown(md);
        let text = render(&lines);
        assert!(text.contains("Title"), "{text}");
        assert!(text.contains("bold"), "{text}");
        // 粗体样式
        let bold = styled_with(&lines, |s| s.add_modifier.contains(Modifier::BOLD));
        assert!(bold.iter().any(|t| t.contains("bold")), "bold span: {bold:?}");
        let italic = styled_with(&lines, |s| s.add_modifier.contains(Modifier::ITALIC));
        assert!(italic.iter().any(|t| t.contains("italic")), "italic span: {italic:?}");
        let code = styled_with(&lines, |s| s.fg == Some(Color::Yellow));
        assert!(code.iter().any(|t| t.contains("code")), "code span: {code:?}");
        assert!(text.contains("• item one"), "list marker: {text}");
        assert!(text.contains("• item two"), "list marker: {text}");
    }

    #[test]
    fn heading_colors_differ_by_level() {
        let lines = render_markdown("# H1\n\n### H3");
        let h1 = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.as_ref() == "H1"))
            .expect("h1 line");
        let h3 = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.as_ref() == "H3"))
            .expect("h3 line");
        let h1_style = h1.spans.iter().find(|s| s.content.as_ref() == "H1").unwrap().style;
        let h3_style = h3.spans.iter().find(|s| s.content.as_ref() == "H3").unwrap().style;
        assert_eq!(h1_style.fg, Some(Color::Cyan));
        assert_ne!(h3_style.fg, Some(Color::Cyan));
        assert!(h1_style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn code_block_has_fence_and_highlight() {
        let md = "```rust\nfn main() { let x = 42; }\n```";
        let lines = render_markdown(md);
        let text = render(&lines);
        assert!(text.contains("┌─ rust"), "{text}");
        assert!(text.contains("fn main()"), "{text}");
        assert!(text.contains("└─"), "{text}");
        // 高亮产生多种前景色（>1 种 span 颜色）
        let colors: std::collections::HashSet<_> = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter_map(|s| s.style.fg)
            .collect();
        assert!(colors.len() > 1, "expected highlighted colors, got {colors:?}");
    }

    #[test]
    fn unclosed_fence_renders_as_code_block_to_eof() {
        let md = "text before\n```python\nprint('hi')";
        let lines = render_markdown(md);
        let text = render(&lines);
        assert!(text.contains("┌─ python"), "{text}");
        assert!(text.contains("print('hi')"), "{text}");
    }

    #[test]
    fn unknown_language_degrades_to_plain() {
        let md = "```notalang123\nsome plain code\n```";
        let lines = render_markdown(md);
        let text = render(&lines);
        assert!(text.contains("some plain code"), "{text}");
        assert!(text.contains("┌─ notalang123"), "{text}");
    }

    #[test]
    fn link_renders_text_and_dest() {
        let lines = render_markdown("see [example](https://example.com) here");
        let text = render(&lines);
        assert!(text.contains("example"), "{text}");
        assert!(text.contains("https://example.com"), "{text}");
    }

    #[test]
    fn blockquote_prefixed() {
        let lines = render_markdown("> quoted wisdom");
        let text = render(&lines);
        assert!(text.contains("> quoted wisdom"), "{text}");
    }

    #[test]
    fn malformed_and_huge_inputs_do_not_panic() {
        // 未闭合强调/链接/列表/嵌套残缺
        let malformed = "**unclosed *nested [link](\n\n> unclosed quote\n- [ ] task";
        let _ = render_markdown(malformed);
        // 10KB 长文
        let huge = format!("# Big\n\n{}", "word ".repeat(2000));
        let lines = render_markdown(&huge);
        assert!(!lines.is_empty());
        // 二进制样字符
        let _ = render_markdown("a\u{0}b\u{1}c\0\0\0");
        // 无闭合粗体
        let _ = render_markdown("this **never closes");
    }

    #[test]
    fn repeated_render_hits_cache_and_is_stable() {
        let md = "# Cache\n\n**same** content `x`";
        let a = render_markdown(md);
        let b = render_markdown(md);
        assert_eq!(render(&a), render(&b));
        // 100 次重复渲染（历史消息每帧重画路径）应在缓存下迅速完成
        let start = std::time::Instant::now();
        for _ in 0..100 {
            let _ = render_markdown(md);
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn perf_10kb_100_frames_under_2s() {
        // 10KB 混合 markdown（标题/粗体/代码块/列表）连续渲染 100 帧。
        let mut md = String::new();
        for i in 0..90 {
            md.push_str(&format!("## Section {i}\n\nsome **bold** text with `code` and [l](https://x.com/{i})\n\n"));
            md.push_str("```rust\nfn f() -> u32 { 42 }\n```\n\n");
            md.push_str("- alpha\n- beta\n\n");
        }
        assert!(md.len() >= 10_000, "need 10KB, got {}", md.len());
        let start = std::time::Instant::now();
        for _ in 0..100 {
            let lines = render_markdown(&md);
            assert!(!lines.is_empty());
        }
        let elapsed = start.elapsed();
        assert!(elapsed < std::time::Duration::from_secs(2), "took {elapsed:?}");
    }
}
