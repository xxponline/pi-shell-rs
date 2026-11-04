//! Three-region layout: header / scrollable chat / input editor.

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use pi_rust_tui::session::{Phase, SessionState};

use crate::app::chat::chat_lines;
use crate::app::wrap::wrap_for_viewport;

/// Render one frame from immutable state.
pub fn render(
    frame: &mut Frame,
    state: &SessionState,
    input: &str,
    notice: Option<&str>,
) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header/status
            Constraint::Min(1),    // chat
            Constraint::Length(3), // input
        ])
        .split(area);

    render_header(frame, chunks[0], state, notice);
    render_chat(frame, chunks[1], state);
    render_editor(frame, chunks[2], input, state.phase);
}

fn render_header(frame: &mut Frame, area: ratatui::layout::Rect, state: &SessionState, notice: Option<&str>) {
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
    if let Some(notice) = notice {
        spans.push(Span::styled(
            truncate(notice, area.width as usize),
            Style::default().fg(Color::Red),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_chat(frame: &mut Frame, area: ratatui::layout::Rect, state: &SessionState) {
    let lines = chat_lines(state);
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
            .draw(|frame| render(frame, state, input, None))
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
}
