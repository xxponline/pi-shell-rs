//! Three-region layout: header / scrollable chat / input editor.

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use pi_rust_tui::session::{Phase, SessionState};

use crate::app::chat::chat_lines;

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
    let line_count = lines.len() as u16;
    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((line_count.saturating_sub(area.height), 0));
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
