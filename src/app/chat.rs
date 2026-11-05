//! 会话状态 → 可渲染 ratatui 行的纯映射（markdown/工具卡片/thinking/通知）。
//!
//! 渲染偏好通过 [`RenderFlags`] 传入（不进 SessionState——它们是 UI 偏好，
//! 不是会话事实），保证相同状态 + 相同 flags 产出相同内容，可离线单测。

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use pi_rust_tui::session::{Block, Entry, SessionState};

/// 渲染偏好标志（App 级 UI 状态，经键位切换）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RenderFlags {
    /// thinking 块是否展开（默认折叠为单行占位，ctrl+t 切换）。
    pub show_thinking: bool,
    /// 工具输出是否展开（默认折叠，ctrl+o 切换）。
    pub expand_tools: bool,
}

/// 构建全部聊天行：先历史 transcript，再流式 in-flight 内容。
///
/// 输入: state — 会话状态；flags — 渲染偏好（thinking/工具展开）
/// 输出: 带样式的行序列，供 wrap 层按视口处理
pub fn chat_lines(state: &SessionState, flags: RenderFlags) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for entry in state.entries() {
        match entry {
            Entry::User { text } => {
                lines.push(prefixed("You", text, Color::Cyan, true));
            }
            Entry::Assistant { blocks, .. } => {
                append_blocks(&mut lines, blocks, flags);
            }
            Entry::Notice { kind, text } => {
                lines.push(notice_line(kind, text));
            }
        }
    }
    // 流式中尚未落盘的内容。
    if state.is_in_flight() {
        append_blocks(&mut lines, state.in_flight(), flags);
    }
    lines
}

/// 按通知类型生成样式行：error 红色加粗、retry 黄色、其余弱化斜体灰。
///
/// 输入: kind — 通知类别（"error"/"retry"/其它）；text — 通知正文
/// 输出: 单行样式化通知
fn notice_line(kind: &str, text: &str) -> Line<'static> {
    let style = match kind {
        "error" => Style::default()
            .fg(Color::Red)
            .add_modifier(Modifier::BOLD),
        "retry" => Style::default().fg(Color::Yellow),
        _ => Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
    };
    Line::from(vec![Span::styled(format!("[{kind}] {text}"), style)])
}

/// 把一组内容块追加到输出行（markdown 文本/thinking 两态/工具卡片）。
///
/// 输入: out — 结果收集器；blocks — 内容块；flags — 渲染偏好
/// 输出: 无返回值；副作用为向 out 追加行
fn append_blocks(out: &mut Vec<Line<'static>>, blocks: &[Block], flags: RenderFlags) {
    for block in blocks {
        match block {
            Block::Text { text } => {
                if !text.is_empty() {
                    // assistant 文本按 markdown 渲染（流式未闭合语法安全降级）。
                    out.extend(crate::app::markdown::render_markdown(text));
                }
            }
            Block::Thinking { text } => {
                if text.is_empty() {
                    continue;
                }
                if flags.show_thinking {
                    // 展开态：完整思考文本，弱化斜体灰（逐行保留换行）。
                    let style = Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC);
                    for part in text.split('\n') {
                        out.push(Line::from(Span::styled(part.to_string(), style)));
                    }
                } else {
                    // 折叠态：单行占位（含字符数）。
                    let n = text.chars().count();
                    out.push(Line::from(vec![Span::styled(
                        format!("<thinking {n} chars>"),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    )]));
                }
            }
            Block::ToolCall { name, args_json, tool, .. } => {
                // 工具调用渲染为结构化卡片（展开态由 flags.expand_tools 控制）。
                out.extend(crate::app::tools::tool_lines(
                    name,
                    args_json,
                    tool,
                    flags.expand_tools,
                ));
            }
        }
    }
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
        let lines = chat_lines(&s, RenderFlags::default());
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
        let lines = chat_lines(&s, RenderFlags::default());
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
        let lines = chat_lines(&s, RenderFlags::default());
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
        let lines = chat_lines(&st, RenderFlags::default());
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
        let lines = chat_lines(&st, RenderFlags::default());
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

    fn thinking_state() -> SessionState {
        let mut s = SessionState::new();
        s.handle(&serde_json::json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        s.handle(&serde_json::json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_start","contentIndex":0}}));
        s.handle(&serde_json::json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","contentIndex":0,"delta":"secret thoughts here"}}));
        s.handle(&serde_json::json!({"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":1}}));
        s.handle(&serde_json::json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":1,"delta":"answer"}}));
        s
    }

    #[test]
    fn thinking_collapsed_by_default() {
        let s = thinking_state();
        let lines = chat_lines(&s, RenderFlags::default());
        let text = render_to_string(&lines);
        assert!(text.contains("<thinking 20 chars>"), "{text}");
        assert!(!text.contains("secret thoughts"), "折叠态不得展开全文: {text}");
    }

    #[test]
    fn thinking_expanded_shows_full_text() {
        let s = thinking_state();
        let flags = RenderFlags { show_thinking: true, expand_tools: false };
        let lines = chat_lines(&s, flags);
        let text = render_to_string(&lines);
        assert!(text.contains("secret thoughts here"), "{text}");
        assert!(!text.contains("<thinking"), "展开态不应再有占位符: {text}");
        // 展开样式为弱化斜体灰
        let thinking_span = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|sp| sp.content.as_ref().contains("secret thoughts"))
            .expect("thinking span");
        assert_eq!(thinking_span.style.fg, Some(Color::DarkGray));
        assert!(thinking_span.style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn thinking_toggle_roundtrip_restores_placeholder() {
        let s = thinking_state();
        let on = RenderFlags { show_thinking: true, expand_tools: false };
        let off = RenderFlags::default();
        let expanded = render_to_string(&chat_lines(&s, on));
        let collapsed = render_to_string(&chat_lines(&s, off));
        assert!(expanded.contains("secret thoughts"));
        assert!(collapsed.contains("<thinking"));
    }

    #[test]
    fn expand_tools_flag_propagates_to_tool_cards() {
        use pi_rust_tui::session::{Block, ToolExecution, ToolStatus};
        let mut s = SessionState::new();
        s.handle(&serde_json::json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        // 构造一个多行结果的工具块
        let result = serde_json::json!({"content":[{"type":"text","text":"l1\nl2\nl3\nl4\nl5\nl6"}]});
        s.transcript.push(pi_rust_tui::session::Entry::Assistant {
            turn: 1,
            blocks: vec![Block::ToolCall {
                id: "c1".into(),
                name: "read".into(),
                args_json: r#"{"path":"/a/b.txt"}"#.into(),
                tool: ToolExecution {
                    status: ToolStatus::Ended,
                    is_error: false,
                    result: Some(result),
                    duration_ms: Some(3),
                },
            }],
            usage: None,
        });
        let collapsed = render_to_string(&chat_lines(&s, RenderFlags::default()));
        let expanded = render_to_string(&chat_lines(
            &s,
            RenderFlags { show_thinking: false, expand_tools: true },
        ));
        assert!(collapsed.contains("l1"), "{collapsed}");
        assert!(!collapsed.contains("l6"), "折叠态不应到 l6: {collapsed}");
        assert!(expanded.contains("l6"), "展开态应含 l6: {expanded}");
    }

    #[test]
    fn retry_notice_is_yellow_and_error_stays_red() {
        let retry = notice_line("retry", "重试 1/3: boom");
        let err = notice_line("error", "400 bad");
        let info = notice_line("info", "note");
        let fg = |l: &Line| l.spans[0].style.fg;
        assert_eq!(fg(&retry), Some(Color::Yellow), "retry 应为黄色");
        assert_eq!(fg(&err), Some(Color::Red), "error 保持红色");
        assert_eq!(fg(&info), Some(Color::DarkGray));
        let retry_text: String = retry.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(retry_text.contains("[retry] 重试 1/3: boom"));
    }
}
