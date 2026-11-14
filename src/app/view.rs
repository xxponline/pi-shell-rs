//! 三区布局渲染：状态行 / 可滚动聊天区 / 输入编辑器。

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use pi_shell_rs::session::{Phase, SessionState, Usage};

use crate::app::chat::{chat_lines, RenderFlags};
use crate::app::scroll::ChatScroll;
use crate::app::wrap::wrap_for_viewport;
use unicode_width::UnicodeWidthChar;

/// 一帧所需的全部 UI 附加输入（与会话状态解耦，便于测试构造）。
#[derive(Default)]
pub struct ViewUi<'a> {
    /// 主输入框文本（含 \n 的全文）。
    pub input: &'a str,
    /// 输入框逻辑行数（决定动态高度）。
    pub input_lines: usize,
    /// 输入框光标（行, 字符序号列；显示格列在渲染层按 unicode-width 换算）；
    /// None 表示隐藏（如浮层打开时）。
    pub input_cursor: Option<(usize, usize)>,
    /// 瞬时提示与颜色。
    pub notice: Option<(&'a str, Color)>,
    /// setStatus 的状态行附加文本。
    pub status_text: Option<&'a str>,
    /// setWidget 的编辑器上方行（空切片=隐藏）。
    pub widget_lines: &'a [String],
    /// thinking/工具展开偏好。
    pub flags: RenderFlags,
    /// 当前浮层（最上层绘制）。
    pub overlay: Option<&'a crate::app::overlay::Overlay>,
    /// 当前会话文件路径（选择器标记"当前"用）。
    pub current_session: Option<&'a str>,
    /// compaction 期间本地排队的提交（队列带显示）。
    pub local_queue: &'a [String],
    /// 斜杠补全菜单（Some=打开，渲染于输入框下方）。
    pub completion: Option<&'a crate::app::autocomplete::CompletionState>,
    /// 消息区滚动状态（design D1；按值 Copy，渲染端经返回值回写）。
    pub scroll: ChatScroll,
    /// 单卡工具展开覆盖表（None=无覆盖，全部跟随全局，design D1/D5）。
    pub tool_expanded: Option<&'a std::collections::HashMap<String, bool>>,
    /// thinking 单块展开覆盖表（None=无覆盖，全部跟随全局，design D3/D5）。
    pub thinking_overrides: Option<&'a std::collections::HashMap<usize, bool>>,
    /// 滚动条绘制覆盖（None=本帧不画，design D2）。
    pub scrollbar: Option<crate::app::wrap::ScrollbarOverlay>,
}


/// 从不可变状态渲染一帧（含浮层）。
///
/// 输入: frame — ratatui 帧；state — 会话状态；ui — 一帧的 UI 附加输入
/// 输出: `(吸收后的 ChatScroll, Vec<Hit>, 消息区 Rect)`——前者为渲染端回写
///       （视口首行/贴底跟随/视口高，design D2），次者为可点击内容
///       （工具卡/thinking）绝对屏行命中区间（design D1/D2），末者为
///       消息区上帧布局（滚动条列命中判定，design D5）；
///       副作用为绘制整帧
pub fn render(
    frame: &mut Frame,
    state: &SessionState,
    ui: &ViewUi<'_>,
) -> (ChatScroll, Vec<crate::app::wrap::Hit>, ratatui::layout::Rect) {
    let area = frame.area();
    let queue = queue_lines(state, ui.local_queue);
    let queue_h = queue.len() as u16;
    // widget 区含 TOP 边框/标题行，非空时多占 1 行。
    let widget_h = if ui.widget_lines.is_empty() {
        0
    } else {
        ui.widget_lines.len() as u16 + 1
    };
    let editor_h = editor_height(ui.input_lines.max(1));
    let completion_h = ui.completion.map(completion_height).unwrap_or(0);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),            // header/status
            Constraint::Min(1),               // chat
            Constraint::Length(queue_h),      // queue strip（空队列不占行）
            Constraint::Length(widget_h),     // extension widget（未设置不占行）
            Constraint::Length(editor_h),     // input（随行数增长，上限 8）
            Constraint::Length(completion_h), // 斜杠补全菜单（关闭时 0）
        ])
        .split(area);

    render_header(frame, chunks[0], state, ui);
    let (scroll, hits) = render_chat(
        frame,
        chunks[1],
        state,
        ui.flags,
        ui.scroll,
        ui.tool_expanded,
        ui.thinking_overrides,
        ui.scrollbar,
    );
    if queue_h > 0 {
        frame.render_widget(
            Paragraph::new(queue).style(Style::default().fg(Color::DarkGray)),
            chunks[2],
        );
    }
    if widget_h > 0 {
        let widget: Vec<Line> = ui
            .widget_lines
            .iter()
            .map(|l| Line::from(l.clone()))
            .collect();
        frame.render_widget(
            Paragraph::new(widget)
                .style(Style::default().fg(Color::Cyan))
                .block(Block::default().borders(Borders::TOP).title(" widget ")),
            chunks[3],
        );
    }
    // 浮层打开时隐藏终端光标（避免光标漂在浮层上）
    let editor_ui = ViewUi {
        input_cursor: if ui.overlay.is_some() {
            None
        } else {
            ui.input_cursor
        },
        ..ViewUi {
            input: ui.input,
            input_lines: ui.input_lines,
            input_cursor: ui.input_cursor,
            notice: ui.notice,
            status_text: ui.status_text,
            widget_lines: ui.widget_lines,
            flags: ui.flags,
            overlay: ui.overlay,
            current_session: ui.current_session,
            local_queue: ui.local_queue,
            completion: ui.completion,
            scroll: ui.scroll,
            tool_expanded: ui.tool_expanded,
            thinking_overrides: ui.thinking_overrides,
            scrollbar: ui.scrollbar,
        }
    };
    render_editor(frame, chunks[4], &editor_ui, state.phase);
    if let (Some(comp), Some(area)) = (ui.completion, chunks.get(5)) {
        render_completion(frame, *area, comp);
    }

    if let Some(overlay) = ui.overlay {
        crate::app::overlay::render_overlay(frame, overlay, ui.current_session);
    }
    (scroll, hits, chunks[1])
}

/// 构建队列带行：`[steer]`/`[queue]` 前缀，最多 2 行，超出显示 `…N more`。
///
/// 输入: state — 含 steering_queue/follow_up_queue 的会话状态
/// 输出: 空队列返回空 vec（不占布局行）
fn queue_lines(state: &SessionState, local_queue: &[String]) -> Vec<Line<'static>> {
    let mut items: Vec<String> = Vec::new();
    for text in local_queue {
        items.push(format!("[queued] {text}"));
    }
    for text in &state.steering_queue {
        items.push(format!("[steer] {text}"));
    }
    for text in &state.follow_up_queue {
        items.push(format!("[queue] {text}"));
    }
    if items.is_empty() {
        return Vec::new();
    }
    let total = items.len();
    let max_lines = 2;
    let mut lines: Vec<Line> = items
        .into_iter()
        .take(max_lines)
        .map(|t| Line::from(Span::raw(t)))
        .collect();
    if total > max_lines {
        lines.push(Line::from(Span::styled(
            format!("… {} more", total - max_lines),
            Style::default().fg(Color::DarkGray),
        )));
    }
    lines
}

/// token 数千分位缩写：999→"999"、1234→"1.2k"、1_500_000→"1.5M"。
///
/// 输入: n — token 计数
/// 输出: 紧凑表示字符串
fn fmt_tokens(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{}.{}k", n / 1000, (n % 1000) / 100),
        _ => format!("{}.{}M", n / 1_000_000, (n % 1_000_000) / 100_000),
    }
}

/// 费用格式化：0/缺失不显示；<1 保留 3 位小数（$0.004）；≥1 保留 2 位（$1.23）。
///
/// 输入: total — 总费用（美元）
/// 输出: Some("$x…")；为 0 时 None（优雅降级，不显示误导值）
fn fmt_cost(total: f64) -> Option<String> {
    if total <= 0.0 {
        return None;
    }
    if total < 1.0 {
        Some(format!("${total:.3}"))
    } else {
        Some(format!("${total:.2}"))
    }
}

/// 用量行 span 序列：`1.2k↑/340↓/8kcache` + 可选 `$0.012`。
///
/// 输入: usage — 含 token 与 cost 的用量快照
/// 输出: 可拼入状态行的 span 序列（cost 为 0 时省略费用段）
fn usage_spans(usage: &Usage) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(
        format!(
            "{}/{}↑↓/{}cache",
            fmt_tokens(usage.input),
            fmt_tokens(usage.output),
            fmt_tokens(usage.cache_read)
        ),
        Style::default().fg(Color::DarkGray),
    )];
    if let Some(cost) = fmt_cost(usage.cost_total) {
        spans.push(Span::styled(
            format!(" {cost}"),
            Style::default().fg(Color::Green),
        ));
    }
    spans
}

/// 状态行渲染：phase · 会话名 · 模型 · thinking · 用量/费用 · 瞬时提示。
///
/// 输入: frame — ratatui 帧；area — 状态行区域；state — 会话状态；
///       notice — 瞬时提示（可空）
/// 输出: 无返回值；副作用为向帧写入状态行
fn render_header(frame: &mut Frame, area: ratatui::layout::Rect, state: &SessionState, ui: &ViewUi<'_>) {
    let notice = ui.notice;
    let (phase_label, color) = match state.phase {
        Phase::Streaming => ("● streaming", Color::Yellow),
        Phase::Idle => ("○ idle", Color::Green),
    };
    let mut spans = vec![
        Span::styled(phase_label, Style::default().fg(color).add_modifier(Modifier::BOLD)),
        Span::raw("  "),
    ];
    if let Some(name) = &state.session_name {
        spans.push(Span::styled(
            format!("{name}  "),
            Style::default().fg(Color::Cyan),
        ));
    }
    if let Some(level) = &state.thinking_level {
        spans.push(Span::raw(format!("thinking:{level}  ")));
    }
    if let Some(model) = &state.model {
        spans.push(Span::styled(
            format!("model:{model}  "),
            Style::default().fg(Color::Magenta),
        ));
    }
    if let Some(usage) = &state.usage {
        spans.extend(usage_spans(usage));
        spans.push(Span::raw("  "));
    }
    if let Some(text) = ui.status_text {
        spans.push(Span::styled(
            format!(" {text} "),
            Style::default().fg(Color::Magenta),
        ));
    }
    if let Some((text, color)) = notice {
        spans.push(Span::styled(
            truncate(text, area.width as usize),
            Style::default().fg(color),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// 渲染消息区（滚动窗口）。
///
/// 输入:
///   frame  — ratatui 帧
///   area   — 消息区矩形（三分区布局的 Min(1) 块）
///   state  — 会话状态（chat_lines 展开源）
///   flags  — thinking/工具展开偏好
///   scroll — 当前滚动状态（following 贴底 / 钉住开窗）
///   tool_expanded — 单卡展开覆盖表（None=无覆盖，design D1）
///   thinking_overrides — thinking 单块覆盖（None=无覆盖，design D3）
/// 输出: `(吸收后的 ChatScroll, Vec<Hit>)`——前者为窗口首行/贴底判定/
///       视口高回写，后者为可点击内容绝对屏行命中区间（design D1/D2）；
///       副作用为在 area 上绘制消息区
// 参：frame/area/state/flags/scroll/tool_expanded/thinking_overrides/
// scrollbar —— 逐参语义直白优于打包结构（design D5 取舍）。
#[allow(clippy::too_many_arguments)]
fn render_chat(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    state: &SessionState,
    flags: RenderFlags,
    scroll: ChatScroll,
    tool_expanded: Option<&std::collections::HashMap<String, bool>>,
    thinking_overrides: Option<&std::collections::HashMap<usize, bool>>,
    scrollbar: Option<crate::app::wrap::ScrollbarOverlay>,
) -> (ChatScroll, Vec<crate::app::wrap::Hit>) {
    let empty = std::collections::HashMap::new();
    let overrides = tool_expanded.unwrap_or(&empty);
    let empty_t = std::collections::HashMap::new();
    let thinking = thinking_overrides.unwrap_or(&empty_t);
    let (lines, meta) = chat_lines(state, flags, overrides, thinking);
    // 自行换行后按滚动状态开窗口：ratatui 的 Wrap+scroll 偏移按换行后行数
    // 计，我们无法精确预知，会导致最新内容被挤出视口（见 wrap 模块说明）。
    let (wrapped, next, hits) =
        wrap_for_viewport(&lines, area.width, area.height, &scroll, &meta, scrollbar);
    let paragraph = Paragraph::new(wrapped);
    frame.render_widget(paragraph.block(Block::default().borders(Borders::NONE)), area);
    (next, hits)
}

/// 输入区高度：clamp(逻辑行数 + 2 边框, 3, 8)——随行数增长但不挤压消息区。
///
/// 输入: lines — 逻辑行数（至少 1）
/// 输出: 输入框总高度（含上下边框）
fn editor_height(lines: usize) -> u16 {
    (lines as u16 + 2).clamp(3, 8)
}

/// 补全菜单同时可见上限（对齐 pi autocompleteMaxVisible 默认 5）。
const COMPLETION_MAX_VISIBLE: usize = 5;

/// 补全菜单占高：可见条目 + 超限时的 `(i/n)` 计数行。
///
/// 输入: comp — 补全状态（打开时有候选）
/// 输出: 菜单行数（关闭/空候选为 0）
fn completion_height(comp: &crate::app::autocomplete::CompletionState) -> u16 {
    if !comp.open || comp.items.is_empty() {
        return 0;
    }
    let shown = comp.items.len().min(COMPLETION_MAX_VISIBLE);
    let count = usize::from(comp.items.len() > COMPLETION_MAX_VISIBLE);
    (shown + count) as u16
}

/// 渲染补全菜单（输入框正下方）：滚动窗口保证选中可见、选中行高亮、
/// 描述列弱化、超限显示 `(当前/总数)` 计数（spec：菜单渲染与预选）。
///
/// 输入: frame — ratatui 帧；area — 菜单区域；comp — 补全状态
/// 输出: 无返回值；副作用为绘制菜单
fn render_completion(frame: &mut Frame, area: ratatui::layout::Rect, comp: &crate::app::autocomplete::CompletionState) {
    let total = comp.items.len();
    if !comp.open || total == 0 || area.height == 0 {
        return;
    }
    let max = COMPLETION_MAX_VISIBLE.min(area.height as usize);
    // 滚动窗口：选中项始终可见（靠近底部时上移窗口）
    let start = comp.selected.saturating_sub(max - 1).min(total.saturating_sub(max));
    let width = area.width as usize;
    let name_w = 15; // scoped-models(13)+2 对齐列
    let mut lines: Vec<Line> = Vec::new();
    for (offset, item) in comp.items[start..].iter().take(max).enumerate() {
        let idx = start + offset;
        let selected = idx == comp.selected;
        let base = if selected {
            Style::default().fg(Color::Black).bg(Color::Cyan)
        } else {
            Style::default()
        };
        let name_raw = if item.label.is_empty() { &item.value } else { &item.label };
        let name = format!("{:<name_w$}", truncate(name_raw, name_w), name_w = name_w);
        let mut spans = vec![Span::styled(name, base)];
        if let Some(desc) = &item.description {
            let room = width.saturating_sub(name_w + 1);
            if room > 0 {
                let desc_style = if selected {
                    base
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                spans.push(Span::styled(truncate(desc, room), desc_style));
            }
        }
        lines.push(Line::from(spans));
    }
    if total > max {
        lines.push(Line::from(Span::styled(
            format!(" ({}/{})", comp.selected + 1, total),
            Style::default().fg(Color::DarkGray),
        )));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// 按显示列跳过并截取一行文本（宽字符不切断）。
///
/// 输入: line — 源行（不含换行）；skip_w — 起始显示列偏移（格）；
///       take_w — 最多保留的显示格数
/// 输出: (片段, 片段实际起始显示列)。跳过仅丢弃"整字结束列 ≤ skip_w"的
///       字符，跨偏移点的宽字符整字保留（起始列可能小于 skip_w）；截取按
///       累计显示宽 ≤ take_w 停止，不会留下半个宽字符；纯 ASCII 下退化为
///       原来的 skip/take 字符数语义
fn slice_display(line: &str, skip_w: usize, take_w: usize) -> (String, usize) {
    let mut out = String::new();
    let mut start = 0usize; // 片段实际起始显示列（跨偏移宽字符时 < skip_w）
    let mut pos = 0usize; // 当前字符起始显示列
    let mut taken = 0usize; // 已保留的显示宽
    let mut started = false;
    for ch in line.chars() {
        let w = ch.width().unwrap_or(0).max(1);
        if !started {
            if pos + w <= skip_w {
                pos += w;
                continue;
            }
            started = true;
            start = pos;
        }
        if taken + w > take_w {
            break;
        }
        taken += w;
        out.push(ch);
        pos += w;
    }
    (out, start)
}

/// 渲染输入框：多行文本 + 滚动偏移（保证光标可见）+ 终端光标定位。
///
/// 输入: frame — ratatui 帧；area — 输入框区域；ui — 视图输入（文本/行数/光标）；
///       phase — 会话状态（决定边框提示）
/// 输出: 无返回值；副作用为绘制输入框与（光标未隐藏时）设置终端光标
fn render_editor(frame: &mut Frame, area: ratatui::layout::Rect, ui: &ViewUi<'_>, phase: Phase) {
    let hint = match phase {
        Phase::Streaming => "esc: abort",
        Phase::Idle => "enter: send · ctrl+d: quit",
    };
    // bash 模式提示：输入以 ! 起始时边框变黄（对齐 pi getBashModeBorderColor 语义）
    let bash_mode = ui.input.trim_start().starts_with('!');
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(if bash_mode {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        })
        .title(format!(" input ({hint}) "));

    // 可视内部区与滚动偏移（水平+垂直，保证光标列/行可见）
    let inner_w = area.width.saturating_sub(2) as usize;
    let inner_h = area.height.saturating_sub(2) as usize;
    let (cur_row, cur_col) = ui.input_cursor.unwrap_or((0, 0));
    let row_off = (cur_row + 1).saturating_sub(inner_h);
    // 光标行前缀的显示宽（字符序号 → 格数：全角/emoji 计 2），
    // 滚动偏移与光标定位均按显示列，而非字符序号（宽字符行否则光标左偏）
    let cursor_line = ui.input.split('\n').nth(cur_row).unwrap_or("");
    let prefix_w: usize = cursor_line
        .chars()
        .take(cur_col)
        .map(|c| c.width().unwrap_or(0).max(1))
        .sum();
    let col_off = (prefix_w + 1).saturating_sub(inner_w);
    let visible: Vec<Line> = ui
        .input
        .split('\n')
        .skip(row_off)
        .take(inner_h.max(1))
        .map(|l| Line::from(slice_display(l, col_off, inner_w).0))
        .collect();
    frame.render_widget(Paragraph::new(visible).block(block), area);

    // 终端光标定位（浮层打开时 input_cursor=None → 不设置）
    if ui.input_cursor.is_some() && inner_w > 0 && area.height >= 2 {
        // 格列 = 前缀显示宽 − 实际滚动起始列（跨偏移宽字符整字保留时起始列前移）
        let start_w = slice_display(cursor_line, col_off, inner_w).1;
        let cx = area.x + 1 + (prefix_w.saturating_sub(start_w)) as u16;
        let cy = area.y + 1 + (cur_row.saturating_sub(row_off)) as u16;
        frame.set_cursor_position((cx.min(area.x + area.width - 2), cy));
    }
}

fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let taken: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{taken}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_respects_width_and_never_panics() {
        assert_eq!(truncate("", 10), "");
        assert_eq!(truncate("abc", 10), "abc");
        assert_eq!(truncate("abcdef", 3), "ab…");
        assert_eq!(truncate("abcdef", 0), "");
        // CJK-safe: no panic, respects char boundaries
        assert_eq!(truncate("中文测试", 3), "中文…");
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;
    use pi_shell_rs::session::SessionState;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use serde_json::json;

    /// Render the view into a TestBackend buffer and return its text.
    fn render_to_buffer(state: &SessionState, input: &str, w: u16, h: u16) -> String {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let ui = ViewUi {
                    input,
                    ..ViewUi::default()
                };
                render(frame, state, &ui);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn long_reply_state() -> SessionState {
        let mut s = SessionState::new();
        s.handle(&json!({"type":"message_start","message":{"role":"user","content":[{"type":"text","text":"give me a long answer"}]}}));
        s.handle(&json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        let body = "word ".repeat(600) + "TAIL-MARKER-END";
        s.handle(&json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text": body}]}}));
        s
    }

    #[test]
    fn long_reply_tail_visible_in_viewport() {
        let state = long_reply_state();
        let buf = render_to_buffer(&state, "", 60, 20);
        assert!(
            buf.contains("TAIL-MARKER-END"),
            "reply tail must be visible in viewport; got:\n{buf}"
        );
    }

    /// 任务 2.2：render 返回吸收后的滚动状态（following / pinned / 越界三态）。
    #[test]
    fn render_returns_absorbed_scroll_state() {
        let state = long_reply_state();
        let fb = std::cell::Cell::new((
            ChatScroll::default(),
            Vec::<crate::app::wrap::Hit>::new(),
            ratatui::layout::Rect::default(),
        ));
        let draw_with = |scroll: ChatScroll| {
            let backend = TestBackend::new(40, 12);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| {
                    let ui = ViewUi {
                        input: "",
                        scroll,
                        ..ViewUi::default()
                    };
                    fb.set(render(frame, &state, &ui));
                })
                .unwrap();
            fb.take().0
        };

        // following 态：贴底、回写视口高（40x12 → chat = 12 − 1 header − 3 editor）
        let following = draw_with(ChatScroll::default());
        assert!(following.following, "following 态渲染后仍贴底");
        assert_eq!(following.viewport, 8, "回写消息区视口高");
        assert!(following.top > 0, "内容溢出时贴底 top 应大于 0");

        // pinned 态（top=0）：视口钉在头部，不恢复跟随
        let pinned = draw_with(ChatScroll {
            content_len: 0,
            top: 0,
            following: false,
            viewport: 0,
        });
        assert!(!pinned.following, "中段维持钉住");
        assert_eq!(pinned.top, 0, "钉住首行回写");

        // stale top 越界：clamp 到底并恢复跟随，首行与贴底态一致
        let stale = draw_with(ChatScroll {
            content_len: 0,
            top: usize::MAX,
            following: false,
            viewport: 0,
        });
        assert!(stale.following, "越界 clamp 到底应恢复跟随");
        assert_eq!(stale.top, following.top, "clamp 到底与贴底首行一致");
    }

    /// 任务 2.3：render 返回的命中区间含已知卡片 kind 与其行区间
    /// （用户行占行 0，卡片从行 1 起；design D1/D2 渲染反馈）。
    #[test]
    fn render_returns_hits_with_kind_and_range() {
        use pi_shell_rs::session::{Block, Entry, ToolExecution, ToolStatus};
        let mut state = SessionState::new();
        state.transcript.push(Entry::User { text: "hi".into() });
        let text: String = (1..=15).map(|i| format!("l{i}\n")).collect();
        state.transcript.push(Entry::Assistant {
            turn: 1,
            blocks: vec![Block::ToolCall {
                id: "c1".into(),
                name: "read".into(),
                args_json: r#"{"path":"/a"}"#.into(),
                tool: ToolExecution {
                    status: ToolStatus::Ended,
                    is_error: false,
                    result: Some(serde_json::json!({"content":[{"type":"text","text": text}]})),
                    duration_ms: Some(1),
                    ..Default::default()
                },
            }],
            usage: None,
        });

        let backend = TestBackend::new(40, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let fb = std::cell::Cell::new((
            ChatScroll::default(),
            Vec::<crate::app::wrap::Hit>::new(),
            ratatui::layout::Rect::default(),
        ));
        terminal
            .draw(|frame| {
                let ui = ViewUi {
                    input: "",
                    ..ViewUi::default()
                };
                fb.set(render(frame, &state, &ui));
            })
            .unwrap();
        let (_scroll, hits, _area) = fb.take();

        // 只看工具类命中（用户/assistant 首行的 MessageStart 也在表中，
        // design D1 多标记）
        let tool_hits: Vec<_> = hits
            .iter()
            .filter(|h| matches!(h.kind, crate::app::wrap::HitKind::Tool(_)))
            .collect();
        assert_eq!(tool_hits.len(), 1, "应恰一个工具卡命中: {hits:?}");
        let hit = tool_hits[0];
        assert_eq!(
            hit.kind,
            crate::app::wrap::HitKind::Tool("c1".into()),
            "命中含已知卡片 kind"
        );
        assert_eq!(hit.start, 1, "用户行占行 0，卡片从行 1 起");
        assert!(hit.end > hit.start, "区间跨多行: {:?}", hit);
    }

    /// 任务 4.1 补充（端到端渲染段）：`ViewUi.tool_expanded` 覆盖表进入
    /// 渲染——全局折叠 + 覆盖 true → 展开可见（design D1/D5 生产接线，
    /// 点击 → 覆盖表 → 卡片展开的后半段）。
    #[test]
    fn render_uses_tool_expanded_override() {
        use pi_shell_rs::session::{Block, Entry, ToolExecution, ToolStatus};
        let mut state = SessionState::new();
        let text: String = (1..=15).map(|i| format!("l{i}\n")).collect();
        state.transcript.push(Entry::Assistant {
            turn: 1,
            blocks: vec![Block::ToolCall {
                id: "c1".into(),
                name: "read".into(),
                args_json: r#"{"path":"/a"}"#.into(),
                tool: ToolExecution {
                    status: ToolStatus::Ended,
                    is_error: false,
                    result: Some(serde_json::json!({"content":[{"type":"text","text": text}]})),
                    duration_ms: Some(1),
                    ..Default::default()
                },
            }],
            usage: None,
        });
        let mut map = std::collections::HashMap::new();
        map.insert("c1".to_string(), true);

        // 覆盖 true + 全局默认折叠 → 展开可见 l15
        let backend = TestBackend::new(40, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let ui = ViewUi {
                    input: "",
                    tool_expanded: Some(&map),
                    ..ViewUi::default()
                };
                let _ = render(frame, &state, &ui);
            })
            .unwrap();
        let mut overridden = String::new();
        for y in 0..20 {
            for x in 0..40 {
                overridden.push_str(terminal.backend().buffer()[(x, y)].symbol());
            }
            overridden.push('\n');
        }
        assert!(overridden.contains("l15"), "覆盖展开应含 l15: {overridden}");

        // None（无覆盖）→ 跟随全局折叠，无 l11
        let plain_buf = render_to_buffer(&state, "", 40, 20);
        assert!(!plain_buf.contains("l11"), "无覆盖时折叠: {plain_buf}");
    }

    /// 任务 2.3：`ViewUi.thinking_overrides` 覆盖表进入渲染——全局折叠 +
    /// 覆盖 true → thinking 全文可见（design D3/D5 生产接线，点击 →
    /// 覆盖表 → 单块展开的后半段）。
    #[test]
    fn render_uses_thinking_override() {
        use pi_shell_rs::session::{Block, Entry};
        let mut state = SessionState::new();
        state.transcript.push(Entry::Assistant {
            turn: 1,
            blocks: vec![Block::Thinking {
                text: "secret thoughts here".into(),
            }],
            usage: None,
        });
        let mut map = std::collections::HashMap::new();
        map.insert(0usize, true);

        // 覆盖 true + 全局默认折叠 → 全文可见
        let backend = TestBackend::new(40, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let ui = ViewUi {
                    input: "",
                    thinking_overrides: Some(&map),
                    ..ViewUi::default()
                };
                let _ = render(frame, &state, &ui);
            })
            .unwrap();
        let mut overridden = String::new();
        for y in 0..12 {
            for x in 0..40 {
                overridden.push_str(terminal.backend().buffer()[(x, y)].symbol());
            }
            overridden.push('\n');
        }
        assert!(
            overridden.contains("secret thoughts"),
            "覆盖展开应含全文: {overridden}"
        );

        // None（无覆盖）→ 跟随全局折叠，仅占位
        let plain_buf = render_to_buffer(&state, "", 40, 12);
        assert!(
            plain_buf.contains("<thinking"),
            "无覆盖时占位可见: {plain_buf}"
        );
        assert!(
            !plain_buf.contains("secret thoughts"),
            "无覆盖时折叠: {plain_buf}"
        );
    }

    /// 任务 2.2：ViewUi.scrollbar 透传到 wrap——Some → 末列滚动条字形，
    /// None → 无（design D2 生产接线，view 段）。
    #[test]
    fn render_paints_scrollbar_when_overlay_present() {
        use crate::app::wrap::ScrollbarOverlay;
        let state = long_reply_state();

        // Some：末列出现轨道/拖动条字形
        let backend = TestBackend::new(40, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let ui = ViewUi {
                    input: "",
                    scrollbar: Some(ScrollbarOverlay { hovered: false }),
                    ..ViewUi::default()
                };
                let _ = render(frame, &state, &ui);
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        let track = crate::app::wrap::SCROLLBAR_TRACK_COLOR;
        let thumb = crate::app::wrap::SCROLLBAR_THUMB_COLOR;
        let is_bar = |sym: &str, fg: Option<Color>| {
            (sym == "│" || sym == "┃") && (fg == Some(track) || fg == Some(thumb))
        };
        let mut bar_rows = 0;
        for y in 0..12u16 {
            let cell = &buf[(39, y)];
            if is_bar(cell.symbol(), cell.style().fg) {
                bar_rows += 1;
            }
        }
        assert!(bar_rows >= 5, "轨道应覆盖消息区多数行: {bar_rows}");

        // None：末列无滚动条字形（编辑器右边框的 │ 不计——颜色过滤）
        let backend2 = TestBackend::new(40, 12);
        let mut terminal2 = Terminal::new(backend2).unwrap();
        terminal2
            .draw(|frame| {
                let ui = ViewUi {
                    input: "",
                    scrollbar: None,
                    ..ViewUi::default()
                };
                let _ = render(frame, &state, &ui);
            })
            .unwrap();
        let buf2 = terminal2.backend().buffer();
        let mut none_rows = 0;
        for y in 0..12u16 {
            let cell = &buf2[(39, y)];
            if is_bar(cell.symbol(), cell.style().fg) {
                none_rows += 1;
            }
        }
        assert_eq!(none_rows, 0, "None 不画滚动条");
    }

    #[test]
    fn short_content_fully_visible() {
        let mut s = SessionState::new();
        s.handle(&json!({"type":"message_start","message":{"role":"user","content":[{"type":"text","text":"hi"}]}}));
        s.handle(&json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        s.handle(&json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"hello there"}]}}));
        let buf = render_to_buffer(&s, "", 60, 20);
        assert!(buf.contains("hi"));
        assert!(buf.contains("hello there"));
    }

    #[test]
    fn fmt_tokens_abbreviates() {
        assert_eq!(fmt_tokens(0), "0");
        assert_eq!(fmt_tokens(999), "999");
        assert_eq!(fmt_tokens(1234), "1.2k");
        assert_eq!(fmt_tokens(9_999), "9.9k");
        assert_eq!(fmt_tokens(1_500_000), "1.5M");
    }

    #[test]
    fn fmt_cost_boundaries() {
        // 0/负数 → 不显示
        assert_eq!(fmt_cost(0.0), None);
        assert_eq!(fmt_cost(-1.0), None);
        // <1 → 3 位小数
        assert_eq!(fmt_cost(0.004).as_deref(), Some("$0.004"));
        assert_eq!(fmt_cost(0.012).as_deref(), Some("$0.012"));
        // ≥1 → 2 位小数
        assert_eq!(fmt_cost(1.23).as_deref(), Some("$1.23"));
        assert_eq!(fmt_cost(12.5).as_deref(), Some("$12.50"));
    }

    #[test]
    fn usage_spans_hide_zero_cost_and_show_tokens() {
        let mut usage = Usage {
            input: 1234,
            output: 340,
            cache_read: 8000,
            cache_write: 0,
            total_tokens: 9574,
            cost_input: 0.0,
            cost_output: 0.0,
            cost_cache_read: 0.0,
            cost_cache_write: 0.0,
            cost_total: 0.0,
        };
        let spans = usage_spans(&usage);
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("1.2k"), "{text}");
        assert!(text.contains("340"), "{text}");
        assert!(!text.contains('$'), "cost 为 0 不得显示: {text}");

        usage.cost_total = 0.012;
        let text: String = usage_spans(&usage)
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("$0.012"), "{text}");
    }

    #[test]
    fn header_renders_model_and_usage_or_degrades() {
        // 有 model/usage 的状态
        let mut state = SessionState::new();
        state.apply_get_state(&serde_json::json!({
            "model": {"id":"m","name":"MiMo Pro"},
            "thinkingLevel": "high"
        }));
        state.usage = Some(Usage {
            input: 2000,
            output: 500,
            cache_read: 100,
            cache_write: 0,
            total_tokens: 2600,
            cost_input: 0.0,
            cost_output: 0.0,
            cost_cache_read: 0.0,
            cost_cache_write: 0.0,
            cost_total: 1.5,
        });
        let buf = render_to_buffer(&state, "", 100, 10);
        assert!(buf.contains("MiMo Pro"), "{buf}");
        assert!(buf.contains("2.0k"), "{buf}");
        assert!(buf.contains("$1.50"), "{buf}");

        // 字段缺失：空状态不 panic、不显示误导值
        let empty = SessionState::new();
        let buf = render_to_buffer(&empty, "", 60, 8);
        assert!(buf.contains("idle"), "{buf}");
        assert!(!buf.contains('$'), "{buf}");
    }

    #[test]
    fn queue_strip_empty_single_and_overflow() {
        // 空队列 → 无行
        let empty = SessionState::new();
        assert!(queue_lines(&empty, &[]).is_empty());

        // 单条 follow_up → 1 行，带 [queue] 前缀
        let mut one = SessionState::new();
        one.follow_up_queue.push("next question".into());
        let lines = queue_lines(&one, &[]);
        assert_eq!(lines.len(), 1);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("[queue] next question"), "{text}");

        // 超过 2 条 → 截断为 2 行 + “…N more”
        let mut many = SessionState::new();
        many.steering_queue.push("s1".into());
        many.follow_up_queue.push("f1".into());
        many.follow_up_queue.push("f2".into());
        many.follow_up_queue.push("f3".into());
        let lines = queue_lines(&many, &[]);
        assert_eq!(lines.len(), 3, "2 行内容 + 1 行 more");
        let last: String = lines[2].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(last.contains("… 2 more"), "{last}");
        let first: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(first.contains("[steer] s1"), "{first}");
    }

    #[test]
    fn queue_strip_renders_above_editor() {
        let mut state = SessionState::new();
        state.follow_up_queue.push("queued message".into());
        let buf = render_to_buffer(&state, "", 60, 15);
        assert!(buf.contains("[queue] queued message"), "{buf}");
        // 输入框仍在
        assert!(buf.contains("input"), "{buf}");
    }

    #[test]
    fn overlay_open_close_restores_frame() {
        let mut state = SessionState::new();
        state.handle(&serde_json::json!({"type":"message_start","message":{"role":"user","content":[{"type":"text","text":"hello"}]}}));
        let render_with = |overlay: Option<&crate::app::overlay::Overlay>| -> String {
            let backend = TestBackend::new(60, 20);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| {
                    let ui = ViewUi {
                        input: "draft",
                        overlay,
                        ..ViewUi::default()
                    };
                    render(frame, &state, &ui);
                })
                .unwrap();
            let buffer = terminal.backend().buffer().clone();
            let mut out = String::new();
            for y in 0..buffer.area.height {
                for x in 0..buffer.area.width {
                    out.push_str(buffer[(x, y)].symbol());
                }
                out.push('\n');
            }
            out
        };
        let base = render_with(None);
        let overlay = crate::app::overlay::Overlay::ExtConfirm {
            id: "x".into(),
            title: "t".into(),
            message: "sure?".into(),
            confirm_selected: true,
        };
        let with = render_with(Some(&overlay));
        assert!(with.contains("sure?"), "浮层内容应出现: {with}");
        assert!(with.contains("hello"), "底层消息仍可见: {with}");
        let after = render_with(None);
        assert_eq!(base, after, "关闭后帧应与打开前一致");
        assert_ne!(base, with, "打开时帧应不同");
    }

    #[test]
    fn widget_and_status_render_when_set() {
        let state = SessionState::new();
        let backend = TestBackend::new(70, 16);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let ui = ViewUi {
                    input: "",
                    status_text: Some("status-here"),
                    widget_lines: &["widget line 1".to_string(), "widget line 2".to_string()],
                    ..ViewUi::default()
                };
                render(frame, &state, &ui);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        assert!(out.contains("status-here"), "{out}");
        assert!(out.contains("widget line 1"), "{out}");
        assert!(out.contains("input"), "输入框仍在: {out}");

        // widget 清除后不再占行
        terminal
            .draw(|frame| {
                let ui = ViewUi { input: "", status_text: None, widget_lines: &[], ..ViewUi::default() };
                render(frame, &state, &ui);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut out2 = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out2.push_str(buffer[(x, y)].symbol());
            }
        }
        assert!(!out2.contains("widget line 1"), "{out2}");
    }

    #[test]
    fn editor_height_scales_and_caps() {
        assert_eq!(editor_height(1), 3, "最小 3（含边框）");
        assert_eq!(editor_height(6), 8, "6 行 → 6+2=8");
        assert_eq!(editor_height(20), 8, "上限 8");
        assert_eq!(editor_height(0), 3, "0 视为 1");
    }

    /// 渲染带输入元数据的一帧，返回 (缓冲文本, 终端光标)。
    fn render_with_input(
        state: &SessionState,
        input: &str,
        lines: usize,
        cursor: Option<(usize, usize)>,
        w: u16,
        h: u16,
    ) -> (String, (u16, u16)) {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let ui = ViewUi {
                    input,
                    input_lines: lines,
                    input_cursor: cursor,
                    ..ViewUi::default()
                };
                render(frame, state, &ui);
            })
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        let pos = terminal.get_cursor_position().unwrap_or_default();
        (out, (pos.x, pos.y))
    }

    fn input_title_row(buf: &str) -> usize {
        buf.lines()
            .position(|l| l.contains("input ("))
            .expect("input title row")
    }

    #[test]
    fn dynamic_editor_height_keeps_message_visible() {
        let mut state = SessionState::new();
        state.handle(&serde_json::json!({"type":"message_start","message":{"role":"user","content":[{"type":"text","text":"HELLO-MARKER"}]}}));

        // 1 行输入 → 输入框 3 高，标题在 y=27（30-3）
        let (short, _) = render_with_input(&state, "a", 1, Some((0, 0)), 60, 30);
        assert!(short.contains("HELLO-MARKER"), "消息区可见: {short}");
        assert_eq!(input_title_row(&short), 27);

        // 20 行输入 → 输入框 8 高（上限），标题上移到 y=22，消息区仍可见
        let long_input = (0..20).map(|i| format!("L{i}")).collect::<Vec<_>>().join("\n");
        let (long, _) = render_with_input(&state, &long_input, 20, Some((19, 2)), 60, 30);
        assert!(long.contains("HELLO-MARKER"), "消息区仍可见: {long}");
        assert_eq!(input_title_row(&long), 22, "20 行输入 → 高度 8");
        assert!(long.contains("L19"), "末行渲染: {long}");
    }

    /// 宽字符行：光标按显示列定位（字符序号会左偏 3 格）。
    #[test]
    fn wide_char_cursor_uses_display_columns() {
        let state = SessionState::new();
        let input = "你好😊hahah"; // 8 字符, 显示宽 11 (2+2+2+1+1+1+1+1)
        assert_eq!(input.chars().count(), 8);
        let (_, (cx, _)) = render_with_input(&state, input, 1, Some((0, 8)), 60, 30);
        assert_eq!(cx, 12, "光标应按显示列 1+11 定位, 而非字符序号 1+8=9");
    }

    /// 宽字符长行：水平滚动按显示列，行尾光标与行尾文本仍可见。
    #[test]
    fn wide_char_line_scrolls_by_display_columns() {
        let state = SessionState::new();
        let line = "汉".repeat(40); // 80 格 > inner_w (30-2=28)
        let input = format!("{line}END");
        let n = input.chars().count();
        let (buf, (cx, _)) = render_with_input(&state, &input, 1, Some((0, n)), 30, 10);
        assert!((1..=28).contains(&cx), "行尾光标应在输入区内: {cx}");
        assert!(buf.contains("END"), "行尾文本应因水平偏移可见: {buf}");
    }

    /// slice_display：宽字符不切断、跨偏移整字保留、ASCII 退化为原语义。
    #[test]
    fn slice_display_keeps_wide_chars_intact() {
        // 跨偏移的宽字符整字保留（实际起始列前移）
        let (s, start) = slice_display("a汉b", 2, 10);
        assert_eq!(s, "汉b");
        assert_eq!(start, 1, "跨偏移宽字符整字保留 → 实际起始列 1");
        // 截取按显示宽停止，不留半个宽字符
        let (s2, _) = slice_display("汉b", 0, 2);
        assert_eq!(s2, "汉");
        let (s3, _) = slice_display("汉b", 0, 1);
        assert_eq!(s3, "", "1 格放不下 2 格宽字符 → 空，不切半");
        // 纯 ASCII 退化为原 skip/take 字符数语义
        let (s4, start4) = slice_display("abcdef", 2, 3);
        assert_eq!(s4, "cde");
        assert_eq!(start4, 2);
    }

    #[test]
    fn cursor_visible_via_scroll_offsets() {
        let state = SessionState::new();

        // 长行：光标在行尾 → 水平偏移使尾部可见
        let long_line = "x".repeat(200);
        let input = format!("{long_line}TAIL-END");
        let (buf, (cx, cy)) = render_with_input(&state, &input, 1, Some((0, input.chars().count() - 1)), 60, 30);
        assert!(buf.contains("TAIL-END"), "行尾应因水平偏移可见");
        // 光标应落在输入框内（y ∈ [27,30)，x ∈ [1,59)）
        assert!((27..30).contains(&cy), "cursor y: {cy}");
        assert!((1..59).contains(&cx), "cursor x: {cx}");

        // 多行：光标在第 19 行 → 垂直偏移使末行可见
        let many = (0..20).map(|i| format!("row-{i}")).collect::<Vec<_>>().join("\n");
        let (buf2, (cx2, cy2)) = render_with_input(&state, &many, 20, Some((19, 4)), 60, 30);
        assert!(buf2.contains("row-19"), "末行可见");
        assert!(!buf2.contains("row-0\n"), "首行滚出可视区"); // 粗略断言
        assert!((22..30).contains(&cy2), "cursor y in editor: {cy2}");
        assert!(cx2 >= 1, "cursor x: {cx2}");

        // 光标隐藏（浮层态）→ 不设置光标（位置为默认原点或上次值，不 panic）
        let (_, _) = render_with_input(&state, "abc", 1, None, 40, 12);
    }

    #[test]
    fn extreme_input_stress_no_panic() {
        let state = SessionState::new();
        let huge = (0..60).map(|i| format!("{i}{}", "字".repeat(200))).collect::<Vec<_>>().join("\n");
        let (buf, _) = render_with_input(&state, &huge, 60, Some((59, 100)), 40, 15);
        assert!(buf.contains("input ("), "输入框仍在");
    }

    #[test]
    fn session_selector_renders_items_current_marker_and_confirm_title() {
        use crate::app::overlay::Overlay;
        use crate::app::sessions::SessionItem;
        use std::path::PathBuf;
        use std::time::SystemTime;
        let overlay = Overlay::SessionSelect {
            items: vec![
                SessionItem {
                    path: PathBuf::from("/s/cur.jsonl"),
                    name: Some("当前会话".into()),
                    first_message: String::new(),
                    modified: Some(SystemTime::now()),
                    cwd: String::new(),
                },
                SessionItem {
                    path: PathBuf::from("/s/old.jsonl"),
                    name: None,
                    first_message: "第一次的提问内容预览".into(),
                    modified: Some(
                        SystemTime::now() - std::time::Duration::from_secs(7200),
                    ),
                    cwd: String::new(),
                },
            ],
            cursor: 1,
            loading: false,
            seq: 1,
            confirming: false,
        };
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| crate::app::overlay::render_overlay(frame, &overlay, Some("/s/cur.jsonl")))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
        }
        // 双宽 CJK 在缓冲中占两格，断言前去空白（视觉上是连续的）
        let flat: String = out.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(flat.contains("Resume"), "标题");
        assert!(flat.contains("当前会话"), "名称项");
        assert!(flat.contains("第一次的提问内容预览"), "无名回退首条消息");
        assert!(flat.contains("2小时前"), "相对时间");
        assert!(flat.contains("•"), "当前会话标记");
        assert!(flat.contains(">"), "光标行");

        // 确认态标题变化
        let confirming = Overlay::SessionSelect {
            items: vec![SessionItem {
                path: PathBuf::from("/s/x.jsonl"),
                name: Some("x".into()),
                first_message: String::new(),
                modified: None,
                cwd: String::new(),
            }],
            cursor: 0,
            loading: false,
            seq: 1,
            confirming: true,
        };
        // 用全新 Terminal 渲染，避免两帧缓冲混合干扰断言
        let backend2 = TestBackend::new(100, 30);
        let mut terminal2 = Terminal::new(backend2).unwrap();
        terminal2
            .draw(|frame| crate::app::overlay::render_overlay(frame, &confirming, None))
            .unwrap();
        let buffer = terminal2.backend().buffer().clone();
        let mut out2 = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out2.push_str(buffer[(x, y)].symbol());
            }
        }
        let flat2: String = out2.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(flat2.contains("确认删除"), "确认态标题: {flat2}");
        assert!(flat2.contains("未知时间"), "modified=None 显示未知时间");
    }

    // ── 斜杠补全菜单渲染（spec：菜单渲染与最佳匹配预选） ──

    /// 造一个打开的补全菜单（前 n 个命令，选中指定项）。
    fn open_completion(selected: usize) -> crate::app::autocomplete::CompletionState {
        let items = crate::app::commands::COMMANDS
            .iter()
            .map(|c| crate::app::autocomplete::CompletionItem {
                value: c.name.to_string(),
                label: c.name.to_string(),
                description: Some(c.description.to_string()),
            })
            .collect();
        crate::app::autocomplete::CompletionState {
            open: true,
            prefix: "/".into(),
            items,
            selected,
            kind: Some(crate::app::autocomplete::CompletionKind::Command),
            force: false,
        }
    }

    /// 渲染带补全菜单的一帧，返回缓冲文本。
    fn render_with_completion(comp: &crate::app::autocomplete::CompletionState, w: u16, h: u16) -> String {
        let state = SessionState::new();
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let ui = ViewUi {
                    input: "/",
                    input_cursor: Some((0, 1)),
                    completion: Some(comp),
                    ..ViewUi::default()
                };
                render(frame, &state, &ui);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn at_file_completion_renders_dir_suffix_and_path_desc() {
        use crate::app::autocomplete::{CompletionItem, CompletionKind, CompletionState};
        let comp = CompletionState {
            open: true,
            prefix: "@".to_string(),
            items: vec![
                CompletionItem {
                    value: "@src/".to_string(),
                    label: "src/".to_string(),
                    description: Some("src".to_string()),
                },
                CompletionItem {
                    value: "@main.rs".to_string(),
                    label: "main.rs".to_string(),
                    description: Some("src/main.rs".to_string()),
                },
            ],
            selected: 0,
            kind: Some(CompletionKind::AtFile),
            force: false,
        };
        let out = render_with_completion(&comp, 160, 30);
        // 主列按 label 展示（条目名，目录带 `/`），非 `@` 完整值；描述列仍为路径
        assert!(out.contains("src/"), "目录 label: {out}");
        assert!(out.contains("main.rs"), "文件 label: {out}");
        assert!(!out.contains("@src/"), "主列不再展示 @ 值: {out}");
        assert!(out.contains("src/main.rs"), "路径描述行: {out}");
        // 菜单高度按条目数
        assert_eq!(completion_height(&comp), 2, "两条 = 2 行");
    }

    #[test]
    fn path_completion_renders_label_no_desc_with_count() {
        use crate::app::autocomplete::{CompletionItem, CompletionKind, CompletionState};
        // 6 条 Path 候选（超 5 条上限）：无描述列 + `(i/n)` 计数行
        let items = ["src/", "main.rs", "lib.rs", "Cargo.toml", "README.md", "build.rs"]
            .into_iter()
            .map(|n| CompletionItem {
                value: n.to_string(),
                label: n.to_string(),
                description: None, // spec: Path 候选无描述列
            })
            .collect();
        let comp = CompletionState {
            open: true,
            prefix: "".to_string(),
            items,
            selected: 0,
            kind: Some(CompletionKind::Path),
            force: true,
        };
        let out = render_with_completion(&comp, 80, 30);
        assert!(out.contains("src/"), "目录带 / 后缀: {out}");
        assert!(out.contains("(1/6)"), "超限应有计数行: {out}");
        assert_eq!(completion_height(&comp), 6, "5 可见 + 1 计数行");
    }

    #[test]
    fn completion_height_matches_visible_plus_count() {
        let mut comp = open_completion(0);
        assert_eq!(completion_height(&comp), 6, "24 条 = 5 可见 + 1 计数");
        comp.open = false;
        assert_eq!(completion_height(&comp), 0, "关闭不占行");
        comp.open = true;
        comp.items.truncate(3);
        assert_eq!(completion_height(&comp), 3, "3 条全显");
        comp.items.clear();
        assert_eq!(completion_height(&comp), 0, "空候选不占行");
    }

    #[test]
    fn completion_renders_items_count_and_highlight() {
        let comp = open_completion(0); // 选中 settings（首项）
        let buf = render_with_completion(&comp, 80, 30);
        let menu_row = buf
            .lines()
            .position(|l| l.contains("Open settings menu"))
            .expect("应渲染首项说明");
        assert!(buf.contains("(1/24)"), "超限应有计数行:\n{buf}");

        // 选中行高亮：取该行首列单元格背景色（Cyan）
        let backend = TestBackend::new(80, 30);
        let _ = backend;
        // 用带样式的缓冲重新取色
        let state = SessionState::new();
        let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
        terminal
            .draw(|frame| {
                let ui = ViewUi {
                    input: "/",
                    input_cursor: Some((0, 1)),
                    completion: Some(&comp),
                    ..ViewUi::default()
                };
                render(frame, &state, &ui);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let cell = &buffer[(0, menu_row as u16)];
        assert_eq!(
            cell.style().bg,
            Some(Color::Cyan),
            "选中行应有青色高亮背景"
        );
    }

    #[test]
    fn completion_scrolls_window_around_selection() {
        // 选中第 11 项（下标 10）→ 窗口应上移到显示下标 6..10 的尾部窗口
        let comp = open_completion(10);
        let buf = render_with_completion(&comp, 100, 30);
        assert!(buf.contains("import"), "窗口尾部应含下标 6 的 import:\n{buf}");
        assert!(!buf.contains("settings"), "首项已滚出窗口: {buf}");
        assert!(buf.contains("(11/24)"), "计数应反映选中项: {buf}");
    }

    #[test]
    fn force_path_menu_height_and_scroll_window() {
        use crate::app::autocomplete::{CompletionItem, CompletionKind, CompletionState};
        // force Path 菜单 8 条候选：高度 = 5 可见 + 1 计数；选中末项时窗口贴底上移
        let items = (0..8)
            .map(|i| CompletionItem {
                value: format!("item{i}.rs"),
                label: format!("item{i}.rs"),
                description: None,
            })
            .collect();
        let mut comp = CompletionState {
            open: true,
            prefix: "".into(),
            items,
            selected: 0,
            kind: Some(CompletionKind::Path),
            force: true,
        };
        assert_eq!(completion_height(&comp), 6, "8 条 = 5 可见 + 1 计数");
        comp.open = false;
        assert_eq!(completion_height(&comp), 0, "关闭不占行");
        comp.open = true;
        // 选中末项（下标 7）→ 窗口显示下标 3..7，首 3 项滚出
        comp.selected = 7;
        let buf = render_with_completion(&comp, 100, 30);
        assert!(buf.contains("item7.rs"), "选中末项应可见:\n{buf}");
        assert!(buf.contains("item3.rs"), "窗口应贴底起于下标 3:\n{buf}");
        assert!(!buf.contains("item0.rs"), "首项已滚出窗口: {buf}");
        assert!(buf.contains("(8/8)"), "计数反映选中项: {buf}");
    }

    #[test]
    fn editor_border_turns_yellow_in_bash_mode() {
        let state = SessionState::new();
        // 普通输入：边框非黄
        let (text, _) = render_with_input(&state, "ls", 1, Some((0, 2)), 60, 12);
        // ! 起始（含前导空格）：边框黄——通过单元格样式断言
        let backend = TestBackend::new(60, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let ui = ViewUi {
                    input: "!echo hi",
                    input_lines: 1,
                    input_cursor: Some((0, 9)),
                    ..ViewUi::default()
                };
                render(frame, &state, &ui);
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        // 输入框左上角（底起 3 行高 → y=9）边框单元格应为黄色
        let cell = &buf[(0, 9)];
        assert_eq!(cell.style().fg, Some(Color::Yellow), "bash 模式边框应为黄色");

        // 恢复普通输入 → 非黄
        let mut terminal = Terminal::new(TestBackend::new(60, 12)).unwrap();
        terminal
            .draw(|frame| {
                let ui = ViewUi {
                    input: "echo hi",
                    input_lines: 1,
                    input_cursor: Some((0, 7)),
                    ..ViewUi::default()
                };
                render(frame, &state, &ui);
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        let cell = &buf[(0, 9)];
        assert_ne!(cell.style().fg, Some(Color::Yellow), "普通输入边框非黄");
        let _ = text;
    }
}
