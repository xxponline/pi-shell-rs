//! Pure mapping from session state to renderable ratatui text lines.
//!
//! Stage 2 keeps it deliberately simple: assistant text is shown verbatim,
//! thinking blocks collapse to a one-line placeholder, and tool calls render
//! as one status line. Markdown highlighting and tool cards come later.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use pi_rust_tui::session::{Block, Entry, SessionState, ToolStatus};

/// Build all chat lines: finalized transcript followed by in-flight streaming.
pub fn chat_lines(state: &SessionState) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for entry in state.entries() {
        match entry {
            Entry::User { text } => {
                lines.push(prefixed("You", text, Color::Cyan, true));
            }
            Entry::Assistant { blocks, .. } => {
                append_blocks(&mut lines, blocks, true);
            }
            Entry::Notice { kind, text } => {
                // error 类通知（如模型 400/订阅失效）标红加粗，其余弱化为斜体灰。
                let style = if kind == "error" {
                    Style::default()
                        .fg(Color::Red)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC)
                };
                lines.push(Line::from(vec![Span::styled(
                    format!("[{kind}] {text}"),
                    style,
                )]));
            }
        }
    }
    // Live, not-yet-finalized streaming content.
    if state.is_in_flight() {
        append_blocks(&mut lines, state.in_flight(), false);
    }
    lines
}

fn append_lines(out: &mut Vec<Line<'static>>, blocks: &[Block], finalized: bool) {
    let _ = finalized;
    append_blocks(out, blocks, finalized);
}

fn append_blocks(out: &mut Vec<Line<'static>>, blocks: &[Block], _finalized: bool) {
    for block in blocks {
        match block {
            Block::Text { text } => {
                if !text.is_empty() {
                    out.push(assistant_text(text));
                }
            }
            Block::Thinking { text } => {
                if !text.is_empty() {
                    let n = text.chars().count();
                    out.push(Line::from(vec![Span::styled(
                        format!("<thinking {n} chars>"),
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                    )]));
                }
            }
            Block::ToolCall { name, args_json, tool, .. } => {
                let status = match tool.status {
                    ToolStatus::Pending => "pending",
                    ToolStatus::Running => "running",
                    ToolStatus::Ended if tool.is_error => "error",
                    ToolStatus::Ended => "done",
                };
                let color = match status {
                    "error" => Color::Red,
                    "running" | "pending" => Color::Yellow,
                    _ => Color::Green,
                };
                let preview = tool_preview(name, args_json);
                out.push(Line::from(vec![
                    Span::styled("● ", Style::default().fg(color)),
                    Span::styled(name.to_string(), Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(format!(" {preview} · {status}")),
                ]));
            }
        }
    }
}

fn tool_preview(name: &str, args_json: &str) -> String {
    // Best-effort short preview from common arguments; avoid pulling a pretty
    // printer in stage 2.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(args_json) {
        for key in ["command", "path", "file_path", "pattern", "query"] {
            if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
                let truncated: String = s.chars().take(48).collect();
                let ellipsis = if s.chars().count() > 48 { "…" } else { "" };
                let _ = name;
                return format!("{truncated}{ellipsis}");
            }
        }
    }
    String::new()
}

fn prefixed(label: &str, text: &str, color: Color, bold_label: bool) -> Line<'static> {
    let mut style = Style::default().fg(color);
    if bold_label {
        style = style.add_modifier(Modifier::BOLD);
    }
    Line::from(vec![
        Span::styled(format!("{label}: "), style),
        Span::raw(text.to_string()),
    ])
}

fn assistant_text(text: &str) -> Line<'static> {
    // Embedded newlines are preserved in the span; Paragraph wrap handles
    // width while explicit newlines still force line breaks.
    Line::from(Span::raw(text.to_string()))
}

// Allow the helper to be used even if future callers need the wrapper name.
#[allow(dead_code)]
fn _unused(_: &mut Vec<Line<'static>>, blocks: &[Block]) {
    append_lines(&mut Vec::new(), blocks, true);
}

#[cfg(test)]
mod tests {
    use super::*;
    use pi_rust_tui::session::SessionState;
    use serde_json::json;

    #[test]
    fn renders_user_and_assistant_text() {
        let mut s = SessionState::new();
        s.handle(&json!({"type":"message_start","message":{"role":"user","content":[{"type":"text","text":"hello"}]}}));
        s.handle(&json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        s.handle(&json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"hi there"}]}}));
        let lines = chat_lines(&s);
        let joined = render_to_string(&lines);
        assert!(joined.contains("You: hello"));
        assert!(joined.contains("hi there"));
    }

    #[test]
    fn thinking_is_placeholder_and_tool_shows_status() {
        let mut s = SessionState::new();
        s.handle(&json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        s.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_start","contentIndex":0}}));
        s.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","contentIndex":0,"delta":"abcdefgh"}}));
        s.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":1}}));
        s.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":1,"delta":"answer"}}));
        let lines = chat_lines(&s);
        let joined = render_to_string(&lines);
        assert!(joined.contains("<thinking 8 chars>"), "{joined}");
        assert!(joined.contains("answer"));
    }

    #[test]
    fn very_long_content_does_not_panic() {
        let mut s = SessionState::new();
        s.handle(&serde_json::json!({"type":"message_start","message":{"role":"user","content":[{"type":"text","text":"x"}]}}));
        s.handle(&serde_json::json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        let huge = "y".repeat(200_000);
        s.handle(&serde_json::json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":huge}]}}));
        let lines = chat_lines(&s);
        assert!(!lines.is_empty()); // no panic, content rendered
    }

    fn render_to_string(lines: &[Line]) -> String {        lines
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
    fn error_notice_renders_red_with_text() {
        let mut st = SessionState::new();
        st.handle(&serde_json::json!({"type":"message_end","message":{
            "role":"assistant","content":[],"stopReason":"error",
            "errorMessage":"400: subscription expired"
        }}));
        let lines = chat_lines(&st);
        let err_line = lines.iter().find(|l| {
            l.spans.iter().any(|s| s.content.as_ref().contains("[error]"))
        }).expect("error notice line");
        let span = err_line.spans.iter().find(|s| s.content.as_ref().contains("[error]")).unwrap();
        assert!(span.content.as_ref().contains("subscription expired"));
        assert_eq!(span.style.fg, Some(Color::Red), "error must be red");
    }

    #[test]
    fn non_error_notice_stays_subdued() {
        let mut st = SessionState::new();
        // Notice 可直接入公开 transcript（非 error 类型保持弱化样式）
        st.transcript.push(pi_rust_tui::session::Entry::Notice {
            kind: "info".to_string(),
            text: "something".to_string(),
        });
        let lines = chat_lines(&st);
        let info = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.as_ref().contains("[info]")))
            .expect("info notice line");
        let span = info
            .spans
            .iter()
            .find(|s| s.content.as_ref().contains("[info]"))
            .unwrap();
        assert_eq!(span.style.fg, Some(Color::DarkGray));
        assert_ne!(span.style.fg, Some(Color::Red));
    }
}
