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

/// syntect 语法集/主题集（进程内只加载一次）。
fn syntax_set() -> &'static syntect::parsing::SyntaxSet {
    static SS: OnceLock<syntect::parsing::SyntaxSet> = OnceLock::new();
    SS.get_or_init(syntect::parsing::SyntaxSet::load_defaults_newlines)
}

/// 内置语法高亮主题集（进程内只加载一次；按名查表，宿主配色注入
/// design D3）。
fn theme_set() -> &'static syntect::highlighting::ThemeSet {
    static TS: OnceLock<syntect::highlighting::ThemeSet> = OnceLock::new();
    TS.get_or_init(syntect::highlighting::ThemeSet::load_defaults)
}

/// 默认语法高亮主题名（与 `TuiTheme::default().syntax_theme` 一致）。
pub(crate) const DEFAULT_SYNTAX_THEME: &str = "base16-ocean.dark";

/// 校验语法高亮主题名是否有效（内置主题集中存在）。
///
/// 输入: name — 宿主注入的主题名（任意字符串）
/// 输出: true = 有效；false = 无效（调用方回退默认并让宿主感知，不 panic）
///
/// 消费方：[`crate::app::theme::TuiTheme::is_syntax_theme_valid`]（宿主预检）。
pub(crate) fn is_valid_syntax_theme(name: &str) -> bool {
    theme_set().themes.contains_key(name)
}

/// 按名取语法高亮主题；无效名回退默认主题（绝不 panic，design D3）。
///
/// 输入: name — 主题名（无效/空均回退）
/// 输出: `&'static Theme`——有效名取对应主题，否则取默认主题
fn theme_by_name(name: &str) -> &'static syntect::highlighting::Theme {
    let themes = &theme_set().themes;
    themes
        .get(name)
        .or_else(|| themes.get(DEFAULT_SYNTAX_THEME))
        .expect("bundled theme base16-ocean.dark")
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
///
/// 默认配色薄包装（design D1 方案 A）：转发 [`render_markdown_themed`]
/// 传默认配色，存量调用点签名零改动。
///
/// 分工：渲染主路径（view/chat）走 themed 入口；本包装服务存量测试与
/// 非渲染路径（默认外观调用方）。
#[allow(dead_code)]
pub fn render_markdown(text: &str) -> Vec<Line<'static>> {
    render_markdown_themed(text, &crate::app::theme::TuiTheme::default())
}

/// 带注入配色的 markdown 渲染（[`render_markdown`] 的 themed 实现体）。
///
/// 输入: text — markdown 源文本；theme — 注入配色（只用 `syntax_theme`，
///       面板/滚动条色不进 markdown 输出）
/// 输出: Vec<Line<'static>>；内容+主题相同则结果相同（缓存仅性能优化，
///       切主题须经 [`clear_render_caches`] 清空，design D4），绝不 panic
pub fn render_markdown_themed(
    text: &str,
    theme: &crate::app::theme::TuiTheme,
) -> Vec<Line<'static>> {
    let hash = fnv1a(text);
    if let Some(hit) = MD_CACHE.with(|c| c.borrow().get(&hash).cloned()) {
        return hit;
    }
    let lines = parse_to_lines(text, theme);
    MD_CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        if cache.len() >= MD_CACHE_CAP {
            cache.clear();
        }
        cache.insert(hash, lines.clone());
    });
    lines
}

/// 清空渲染双缓存（MD_CACHE + HL_CACHE；两者都烧入主题色，切主题时
/// 同清，design D4）。
///
/// 输入: 无
/// 输出: 无返回值；副作用为清空本线程（渲染线程）两个缓存——
///       `set_theme` 在 event loop 线程调用，与绘制同线程
///
/// 消费方：[`crate::app::App::set_theme`]（启动注入与 `UiEvent::Theme` arm）。
pub fn clear_render_caches() {
    MD_CACHE.with(|c| c.borrow_mut().clear());
    HL_CACHE.with(|c| c.borrow_mut().clear());
}

/// 解析入口（无缓存的实现，供测试与流式 memo 直接调用）。
///
/// 输入: text — markdown 源文本；theme — 注入配色（下传代码块高亮）
/// 输出: 渲染后的行序列
pub(crate) fn parse_to_lines(text: &str, theme: &crate::app::theme::TuiTheme) -> Vec<Line<'static>> {
    parse_to_lines_with(text, theme, Style::default())
}

/// 带初始样式的解析入口（流式 memo 的块区/后缀续渲用：标题样式在
/// parse_to_lines 中跨块泄漏（style_stack 只压不弹），续渲段必须以
/// 前缀末样式为初值才能逐字节对齐全量路径）。
///
/// 输入: text — markdown 源文本（块区/后缀切片）；theme — 注入配色；
///       style0 — 前缀末的样式栈顶（无泄漏时为默认样式）
/// 输出: 渲染后的行序列
pub(crate) fn parse_to_lines_with(
    text: &str,
    theme: &crate::app::theme::TuiTheme,
    style0: Style,
) -> Vec<Line<'static>> {
    // 临时探针（任务 1.1）：整段解析/语法高亮计入高亮段，量完即拆。
    let probe_t0 = std::time::Instant::now();
    let out = parse_to_lines_with_inner(text, theme, style0);
    crate::app::render_probe::record_highlight(probe_t0.elapsed().as_nanos() as u64);
    out
}

/// [`parse_to_lines_with`] 的实现体（外层只做临时分段计时，任务 1.1）。
///
/// 输入/输出: 同 [`parse_to_lines_with`]
fn parse_to_lines_with_inner(
    text: &str,
    theme: &crate::app::theme::TuiTheme,
    style0: Style,
) -> Vec<Line<'static>> {
    let opts = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let parser = Parser::new_ext(text, opts);

    let mut out: Vec<Line<'static>> = Vec::new();
    let mut cur: Vec<Span<'static>> = Vec::new();
    // 内联样式栈（粗体/斜体等在 End 时弹出；标题样式不弹（全量路径
    // 原语义），由 style0 承接前缀泄漏）。
    let mut style_stack: Vec<Style> = vec![style0];
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
                        push_code_block(&mut out, &lang, &body, theme);
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
        push_code_block(&mut out, &lang, &body, theme);
    }
    out
}

/// 计算标题颜色（H1/H2 用青色，其余默认色）；流式 memo 的样式泄漏
/// 推进与本函数同源。
///
/// 输入: level — 标题级别
/// 输出: 对应颜色
pub(crate) fn heading_color(level: HeadingLevel) -> Color {
    match level {
        HeadingLevel::H1 | HeadingLevel::H2 => Color::Cyan,
        _ => Color::Reset,
    }
}

/// 代码块顶栏行（语言标签）；[`push_code_block`] 与流式 memo 共用，
/// 保证两路径逐字节等价。
///
/// 输入: lang — fence 信息串（可空，空时标签为 "code"）
/// 输出: `┌─ {label} ` 样式行（暗色）
pub(crate) fn code_open_bar(lang: &str) -> Line<'static> {
    let label = if lang.is_empty() { "code" } else { lang };
    Line::from(Span::styled(
        format!("┌─ {label} "),
        Style::default().fg(Color::DarkGray),
    ))
}

/// 代码块底栏行（`└─`）；[`push_code_block`] 与流式 memo 共用。
///
/// 输入: 无
/// 输出: `└─` 样式行（暗色）
pub(crate) fn code_close_bar() -> Line<'static> {
    Line::from(Span::styled("└─", Style::default().fg(Color::DarkGray)))
}

/// 渲染一个代码块（顶栏语言标签 + 高亮正文 + 底栏）。
///
/// 输入: out — 结果收集器；lang — 语言标签（可空）；body — 代码原文；
///       theme — 注入配色（取 `syntax_theme` 选 syntect 主题）
/// 输出: 无返回值；副作用为向 out 追加代码块行
fn push_code_block(
    out: &mut Vec<Line<'static>>,
    lang: &str,
    body: &str,
    theme: &crate::app::theme::TuiTheme,
) {
    out.push(code_open_bar(lang));
    for line in highlight_cached(lang, body, theme) {
        out.push(line);
    }
    out.push(code_close_bar());
}

/// 按语言对代码做语法高亮（带缓存；未知语言/失败降级为纯文本行）。
///
/// 输入: lang — 语言标签；body — 代码原文；theme — 注入配色
///       （只在缓存 miss 时用于选主题；切主题经 `clear_render_caches`
///       同清，design D4——键不入主题）
/// 输出: 高亮后的行序列（每源行一行，tab 已展开为 4 空格）
fn highlight_cached(lang: &str, body: &str, theme: &crate::app::theme::TuiTheme) -> Vec<Line<'static>> {
    let key = (lang.to_string(), body.to_string());
    if let Some(hit) = HL_CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return hit;
    }
    let lines = highlight_uncached(lang, body, theme);
    HL_CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        if cache.len() >= HL_CACHE_CAP {
            cache.clear();
        }
        cache.insert(key, lines.clone());
    });
    lines
}

/// 高亮实现（无缓存）：逐行推进 [`CodeLineState`]（与流式 memo 同一
/// 逐行算法，等价按构造成立）；任何失败降级纯文本。
///
/// 输入: lang — 语言标签；body — 代码原文；theme — 注入配色
///       （按 `syntax_theme` 名取主题，无效名回退默认，design D3）
/// 输出: 高亮或降级后的行序列
fn highlight_uncached(
    lang: &str,
    body: &str,
    theme: &crate::app::theme::TuiTheme,
) -> Vec<Line<'static>> {
    let mut st = CodeLineState::new(lang, theme);
    syntect::util::LinesWithEndings::from(body)
        .map(|line| st.push_line(line))
        .collect()
}

/// 代码块逐行高亮状态（design D1：按完结行前缀推进的行级 memo，流式
/// 尾槽与全量路径共用同一算法）。未知语法/高亮失败均降级纯文本行。
///
/// # Safety（Send 实现依据）
/// 内部 `ParseState` 因 onig `Region` 裸指针未标 `Send`，但其全部内容
/// （语法集下标、`Region` 捕获缓冲）都是**自有普通数据**：无 TLS、无
/// 共享句柄，整体搬迁/单线程使用/析构均安全；本类型只随 App 整体移入
/// 渲染线程后单线程使用，不存在并发访问。
#[derive(Clone)]
pub(crate) struct CodeLineState {
    /// syntect 主题（按名查表得来，生命周期恒长）。
    syn_theme: &'static syntect::highlighting::Theme,
    /// 逐行推进状态（未知语言为 None → 纯文本降级）。
    state: Option<(
        syntect::parsing::ParseState,
        syntect::highlighting::HighlightState,
    )>,
}

// SAFETY: 见类型文档——ParseState 为自有普通数据，仅单线程使用。
unsafe impl Send for CodeLineState {}

impl CodeLineState {
    /// 创建逐行高亮状态（按语言选语法；未知/空语言降级纯文本）。
    ///
    /// 输入: lang — fence 信息串（trim 后的标签）；theme — 注入配色
    ///       （取 `syntax_theme` 选 syntect 主题，无效名回退默认）
    /// 输出: 行级状态机；后续 [`push_line`] 逐行推进
    pub(crate) fn new(lang: &str, theme: &crate::app::theme::TuiTheme) -> Self {
        let ss = syntax_set();
        let syntax = ss
            .find_syntax_by_token(lang)
            .or_else(|| ss.find_syntax_by_extension(lang))
            .filter(|_| !lang.is_empty());
        let state = syntax.map(|s| {
            let hl = syntect::easy::HighlightLines::new(s, theme_by_name(&theme.syntax_theme));
            let (hs, ps) = hl.state();
            (ps, hs)
        });
        Self {
            syn_theme: theme_by_name(&theme.syntax_theme),
            state,
        }
    }

    /// 推进并渲染一行（完结行须带行尾换行，与 LinesWithEndings 口径一致；
    /// tab 展开为 4 空格）。
    ///
    /// 输入: line — 单行源文本（含或不含行尾换行）
    /// 输出: 该行渲染；内部状态推进到行末（高亮失败时降级纯文本行，
    ///       状态照常推进，与全量路径行为一致）
    pub(crate) fn push_line(&mut self, line: &str) -> Line<'static> {
        let normalized = line.replace('\t', "    ");
        let Some((ps, hs)) = self.state.take() else {
            return plain_code_line(&normalized);
        };
        let mut hl = syntect::easy::HighlightLines::from_state(self.syn_theme, hs, ps);
        let out = match hl.highlight_line(&normalized, syntax_set()) {
            Ok(ranges) => code_ranges_line(&ranges),
            Err(_) => Line::from(Span::raw(
                normalized.trim_end_matches(['\n', '\r']).to_string(),
            )),
        };
        let (hs, ps) = hl.state();
        self.state = Some((ps, hs));
        out
    }

    /// 渲染开放行（不推进状态）：克隆行级状态后在克隆上推进（spike 实测
    /// HighlightState/ParseState Clone ≈ 0.85µs，远低于单行高亮成本）。
    ///
    /// 输入: line — 开放行源文本（不带行尾换行）
    /// 输出: 该行渲染；`self` 状态不变（开放行每帧可单独重排）
    pub(crate) fn peek_line(&self, line: &str) -> Line<'static> {
        self.clone().push_line(line)
    }
}

/// syntect 高亮区间 → ratatui 行（高亮两路共用的逐行映射；只取 fg 色，
/// 不携带主题 bg 以保工具卡面板底色，design D3）。
///
/// 输入: ranges — 单行的 (样式, 文本) 区间（文本可含行尾换行）
/// 输出: 该行渲染；区间全空 → 空行占位（保持行数与源一致）
fn code_ranges_line(ranges: &[(syntect::highlighting::Style, &str)]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (style, text) in ranges {
        let text = text.trim_end_matches(['\n', '\r']);
        if text.is_empty() {
            continue;
        }
        spans.push(Span::styled(
            text.to_string(),
            Style::default().fg(Color::Rgb(
                style.foreground.r,
                style.foreground.g,
                style.foreground.b,
            )),
        ));
    }
    if spans.is_empty() {
        Line::default()
    } else {
        Line::from(spans)
    }
}

/// 纯文本代码行（未知语法降级；与 `.lines()` 逐行口径一致，行尾换行
/// 按 `\n` 后单个 `\r` 剥离）。
///
/// 输入: line — 单行源文本（已 tab 归一，可含行尾换行）
/// 输出: 无样式原文行
fn plain_code_line(line: &str) -> Line<'static> {
    let stripped = line
        .strip_suffix('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .unwrap_or(line);
    Line::from(Span::raw(stripped.to_string()))
}

/// 按文件扩展名做语法高亮（add-tool-card-syntax-highlighting design D1）。
///
/// 输入: path — 文件路径（取最后一段扩展名，大小写归一；无扩展名 →
///       None）；body — 内容
/// 输出: Some(高亮行)——转发 [`highlight_by_path_themed`] 传默认配色
///       （存量调用点签名零改动；渲染主路径走 themed 入口）
#[allow(dead_code)]
pub(crate) fn highlight_by_path(path: &str, body: &str) -> Option<Vec<Line<'static>>> {
    highlight_by_path_themed(path, body, &crate::app::theme::TuiTheme::default())
}

/// 按文件扩展名做语法高亮（带注入配色；[`highlight_by_path`] 的
/// themed 实现体）。
///
/// 输入: path/body — 同上；theme — 注入配色（只用 `syntax_theme`）
/// 输出: Some(高亮行)——复用 `highlight_cached` 链（tab 归一、哈希缓存、
///       主题按名回退全部继承；其 Style 转换仅取 fg、不携带主题 bg，
///       design D3 保工具卡面板底色）；None = 扩展名缺失或无对应语法 →
///       **调用方走现状纯文本路径**（降级逐字节等价，design D1——不在此
///       处内部降级）
pub(crate) fn highlight_by_path_themed(
    path: &str,
    body: &str,
    theme: &crate::app::theme::TuiTheme,
) -> Option<Vec<Line<'static>>> {
    let ext = std::path::Path::new(path).extension()?.to_str()?.to_lowercase();
    if ext.is_empty() {
        return None;
    }
    // 语法存在性门控：miss → None（交调用方原路径，design D1）。
    syntax_set().find_syntax_by_extension(&ext)?;
    Some(highlight_cached(&ext, body, theme))
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

    /// 任务 1.1：按路径高亮入口（add-tool-card-syntax-highlighting
    /// design D1/D3）——扩展名命中走缓存链且仅 fg（不携带 bg）、
    /// tab 归一继承；无扩展名/未知扩展/空路径 → None（调用方走现状）。
    #[test]
    fn highlight_by_path_hits_extension_and_returns_none_for_unknown() {
        let py = "def hello():\n    return 42\n";
        let hit = highlight_by_path("/tmp/demo.py", py).expect(".py 应命中");
        assert!(!hit.is_empty());
        assert!(
            hit.iter()
                .flat_map(|l| l.spans.iter())
                .any(|s| s.style.fg.is_some()),
            "应有带前景色的 span"
        );
        assert!(
            hit.iter().all(|l| l.spans.iter().all(|s| s.style.bg.is_none())),
            "fg-only——不携带主题 bg（design D3，保工具卡面板底色）"
        );
        // 大小写归一：大写扩展名同样命中
        assert!(highlight_by_path("/tmp/DEMO.PY", py).is_some(), "大写扩展名归一");
        // tab 归一继承（与 markdown 代码块同链）
        let tabs = highlight_by_path("/tmp/t.rs", "a\tb").expect(".rs 应命中");
        assert!(
            render(&tabs).contains("    "),
            "tab 已展开为 4 空格: {:?}",
            render(&tabs)
        );

        // None：未知扩展 / 无扩展名 / 空路径 / 点文件
        assert!(highlight_by_path("/tmp/x.xyzzy", py).is_none(), "未知扩展 → None");
        assert!(highlight_by_path("/tmp/Makefile", py).is_none(), "无扩展名 → None");
        assert!(highlight_by_path("", py).is_none(), "空路径 → None");
        assert!(
            highlight_by_path("/tmp/.gitignore", py).is_none(),
            "点文件无扩展名 → None"
        );
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

    /// 收集代码块渲染中出现的全部前景色（去重）。
    fn code_fg_colors(lines: &[Line<'static>]) -> std::collections::HashSet<Color> {
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter_map(|s| s.style.fg)
            .collect()
    }

    /// 宿主配色注入（design D3，spec「无效主题名回退默认」）：无效主题名
    /// 不 panic、渲染回退默认主题色，且 `is_valid_syntax_theme` 让调用方
    /// 可感知注入未生效。
    ///
    /// 输入: 无（构造无效主题名的 TuiTheme）
    /// 输出: 断言通过；无效名渲染色 = 默认主题色
    #[test]
    fn invalid_syntax_theme_falls_back_to_default() {
        assert!(
            is_valid_syntax_theme(DEFAULT_SYNTAX_THEME),
            "默认主题名应有效"
        );
        assert!(
            !is_valid_syntax_theme("no-such-theme"),
            "无效名应可被调用方检出"
        );

        let md = "```rust\nfn main() { let x = 42; }\n```";
        let bad = crate::app::theme::TuiTheme {
            syntax_theme: "no-such-theme".to_string(),
            ..Default::default()
        };
        clear_render_caches();
        let bad_lines = render_markdown_themed(md, &bad);
        clear_render_caches();
        let default_lines = render_markdown_themed(md, &Default::default());
        assert_eq!(
            code_fg_colors(&bad_lines),
            code_fg_colors(&default_lines),
            "无效主题名应回退默认主题色"
        );
        assert!(code_fg_colors(&bad_lines).len() > 1, "应有高亮色");
    }

    /// 宿主配色注入（design D4，spec「切换后缓存不漏旧色」）：双缓存都
    /// 烧入主题色——切主题（经 clear_render_caches，set_theme 同路径）后
    /// 重绘全部为新主题色，无旧色残留。
    ///
    /// 输入: 无（两个不同 syntect 主题交替渲染）
    /// 输出: 断言通过；切后色集 = 新主题干净渲染色集，且与旧主题不同
    #[test]
    fn theme_switch_clears_caches_and_leaks_no_old_colors() {
        let md = "```rust\nfn main() { let x = 42; }\n```";
        let theme_a = crate::app::theme::TuiTheme {
            syntax_theme: "base16-ocean.dark".to_string(),
            ..Default::default()
        };
        let theme_b = crate::app::theme::TuiTheme {
            syntax_theme: "base16-ocean.light".to_string(),
            ..Default::default()
        };
        assert!(
            is_valid_syntax_theme("base16-ocean.light"),
            "syntect 应内置 base16-ocean.light"
        );

        // 主题 A 渲染（填充 MD_CACHE + HL_CACHE）
        clear_render_caches();
        let colors_a = code_fg_colors(&render_markdown_themed(md, &theme_a));

        // 切主题 B（set_theme 路径：清双缓存）→ 重绘不得残留 A 色
        clear_render_caches();
        let colors_b = code_fg_colors(&render_markdown_themed(md, &theme_b));
        assert_ne!(colors_a, colors_b, "两主题高亮色应不同");
        assert!(colors_b.len() > 1, "B 主题应有高亮色");

        // B 干净渲染（新线程缓存语义等价：先清后渲）应与切后一致
        clear_render_caches();
        let colors_b_fresh = code_fg_colors(&render_markdown_themed(md, &theme_b));
        assert_eq!(
            colors_b, colors_b_fresh,
            "切主题后重绘 = 新主题干净渲染，无旧色残留"
        );
    }
}



