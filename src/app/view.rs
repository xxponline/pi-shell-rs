//! 三区布局渲染：状态行 / 可滚动聊天区 / 输入编辑器。

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use pi_rust_tui::session::{Phase, SessionState, Usage};

use crate::app::chat::{chat_lines, RenderFlags};
use crate::app::wrap::wrap_for_viewport;

/// 一帧所需的全部 UI 附加输入（与会话状态解耦，便于测试构造）。
#[derive(Default)]
pub struct ViewUi<'a> {
    /// 主输入框文本。
    pub input: &'a str,
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
}


/// 从不可变状态渲染一帧（含浮层）。
pub fn render(frame: &mut Frame, state: &SessionState, ui: &ViewUi<'_>) {
    let area = frame.area();
    let queue = queue_lines(state);
    let queue_h = queue.len() as u16;
    // widget 区含 TOP 边框/标题行，非空时多占 1 行。
    let widget_h = if ui.widget_lines.is_empty() {
        0
    } else {
        ui.widget_lines.len() as u16 + 1
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),            // header/status
            Constraint::Min(1),               // chat
            Constraint::Length(queue_h),      // queue strip（空队列不占行）
            Constraint::Length(widget_h),     // extension widget（未设置不占行）
            Constraint::Length(3),            // input
        ])
        .split(area);

    render_header(frame, chunks[0], state, ui);
    render_chat(frame, chunks[1], state, ui.flags);
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
    render_editor(frame, chunks[4], ui.input, state.phase);

    if let Some(overlay) = ui.overlay {
        crate::app::overlay::render_overlay(frame, overlay);
    }
}

/// 构建队列带行：`[steer]`/`[queue]` 前缀，最多 2 行，超出显示 `…N more`。
///
/// 输入: state — 含 steering_queue/follow_up_queue 的会话状态
/// 输出: 空队列返回空 vec（不占布局行）
fn queue_lines(state: &SessionState) -> Vec<Line<'static>> {
    let mut items: Vec<String> = Vec::new();
    for text in &state.steering_queue {
        items.push(format!("[steer] {text}"));
    }
    for text in &state.follow_up_queue {
        items.push(format!("[queue] {text}"));
    }
    if items.is_empty() {
        return Vec::new();
    }
    let max_lines = 2;
    let mut lines: Vec<Line> = items
        .into_iter()
        .take(max_lines)
        .map(|t| Line::from(Span::raw(t)))
        .collect();
    let total = state.steering_queue.len() + state.follow_up_queue.len();
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

fn render_chat(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    state: &SessionState,
    flags: RenderFlags,
) {
    let lines = chat_lines(state, flags);
    // 自行换行并底部锚定：ratatui 的 Wrap+scroll 偏移按换行后行数计，
    // 我们无法精确预知，会导致最新内容被挤出视口（见 wrap 模块说明）。
    let wrapped = wrap_for_viewport(&lines, area.width, area.height);
    let paragraph = Paragraph::new(wrapped);
    frame.render_widget(paragraph.block(Block::default().borders(Borders::NONE)), area);
}

fn render_editor(frame: &mut Frame, area: ratatui::layout::Rect, input: &str, phase: Phase) {
    let hint = match phase {
        Phase::Streaming => "esc: abort",
        Phase::Idle => "enter: send · q/esc: quit",
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" input ({hint}) "));
    let paragraph = Paragraph::new(input.to_string()).block(block);
    frame.render_widget(paragraph, area);
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
    use pi_rust_tui::session::SessionState;
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
        assert!(queue_lines(&empty).is_empty());

        // 单条 follow_up → 1 行，带 [queue] 前缀
        let mut one = SessionState::new();
        one.follow_up_queue.push("next question".into());
        let lines = queue_lines(&one);
        assert_eq!(lines.len(), 1);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("[queue] next question"), "{text}");

        // 超过 2 条 → 截断为 2 行 + “…N more”
        let mut many = SessionState::new();
        many.steering_queue.push("s1".into());
        many.follow_up_queue.push("f1".into());
        many.follow_up_queue.push("f2".into());
        many.follow_up_queue.push("f3".into());
        let lines = queue_lines(&many);
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
}
