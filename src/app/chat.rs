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
            Entry::Bash {
                command,
                output,
                exit_code,
                cancelled,
                truncated,
                full_output_path,
                excluded,
            } => append_bash(
                &mut lines,
                command,
                output,
                *exit_code,
                *cancelled,
                *truncated,
                full_output_path.as_deref(),
                *excluded,
                false,
            ),
        }
    }
    // 流式中尚未落盘的内容。
    if state.is_in_flight() {
        append_blocks(&mut lines, state.in_flight(), flags);
    }
    // 运行中的用户 bash（分片实时增长）。
    if state.bash_running {
        let excluded = state.bash_excluded;
        let command = state.bash_command.clone();
        let output = state.bash_output.clone();
        append_bash(&mut lines, &command, &output, None, false, false, None, excluded, true);
    }
    lines
}

/// bash 输出渲染行数上限（超出保留末尾并提示；pi 侧另有整体截断策略）。
const BASH_OUTPUT_MAX_LINES: usize = 500;

/// 追加一条 bash 渲染：命令行 + 输出（弱化、行数上限）+ 状态行。
///
/// 输入: out — 目标行；command/output — 命令与输出；exit_code/cancelled/
///       truncated/full_output_path — 结果态；excluded — `!!` 不进上下文；
///       live — 运行中（状态行替换为运行标记）
/// 输出: 无返回值；副作用为 push 若干行
#[allow(clippy::too_many_arguments)]
fn append_bash(
    out: &mut Vec<Line<'static>>,
    command: &str,
    output: &str,
    exit_code: Option<i64>,
    cancelled: bool,
    truncated: bool,
    full_output_path: Option<&str>,
    excluded: bool,
    live: bool,
) {
    let mut cmd_spans = vec![Span::styled(
        format!("$ {command}"),
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
    )];
    if excluded {
        cmd_spans.push(Span::styled(
            "（不进上下文）",
            Style::default().fg(Color::DarkGray),
        ));
    }
    out.push(Line::from(cmd_spans));

    // 输出行（超上限保留末尾）
    let all: Vec<&str> = output.split('\n').collect();
    let total = all.len();
    let start = total.saturating_sub(BASH_OUTPUT_MAX_LINES);
    if start > 0 {
        out.push(Line::from(Span::styled(
            format!("…（已截断 {} 行）", start),
            Style::default().fg(Color::DarkGray),
        )));
    }
    for (i, row) in all[start..].iter().enumerate() {
        if row.is_empty() && start + i == total - 1 {
            continue; // 仅跳过结尾换行产生的最后一个空元素（保留中间空行）
        }
        out.push(Line::from(Span::styled(
            (*row).to_string(),
            Style::default().fg(Color::DarkGray),
        )));
    }

    // 状态行
    let status = if live {
        Span::styled("⟳ 运行中…（esc 中止）", Style::default().fg(Color::Yellow))
    } else if cancelled {
        Span::styled("cancelled", Style::default().fg(Color::Yellow))
    } else {
        match exit_code {
            Some(0) => Span::styled("exit 0", Style::default().fg(Color::Green)),
            Some(code) => Span::styled(
                format!("exit {code}"),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            None => Span::styled("done", Style::default().fg(Color::DarkGray)),
        }
    };
    let mut status_spans = vec![status];
    if truncated {
        status_spans.push(Span::styled(
            match full_output_path {
                Some(p) => format!(" · truncated → {p}"),
                None => " · truncated".to_string(),
            },
            Style::default().fg(Color::Yellow),
        ));
    }
    out.push(Line::from(status_spans));
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

    #[test]
    fn bash_entry_and_live_rendering() {
        use pi_rust_tui::session::Entry;
        let mut st = SessionState::new();

        // 完成态：exit 0 + 状态行 + 不进上下文标记
        st.transcript.push(Entry::Bash {
            command: "ls".into(),
            output: "a.rs\nb.rs".into(),
            exit_code: Some(0),
            cancelled: false,
            truncated: false,
            full_output_path: None,
            excluded: true,
        });
        // 失败态：非零退出 + 截断路径
        st.transcript.push(Entry::Bash {
            command: "make".into(),
            output: "error".into(),
            exit_code: Some(2),
            cancelled: false,
            truncated: true,
            full_output_path: Some("/tmp/full.log".into()),
            excluded: false,
        });
        // 取消态
        st.transcript.push(Entry::Bash {
            command: "sleep 99".into(),
            output: String::new(),
            exit_code: Some(130),
            cancelled: true,
            truncated: false,
            full_output_path: None,
            excluded: false,
        });
        let lines = chat_lines(&st, RenderFlags::default());
        let text: String = lines
            .iter()
            .map(|l| {
                let mut s = String::new();
                for sp in &l.spans {
                    s.push_str(sp.content.as_ref());
                }
                s.push('\n');
                s
            })
            .collect();
        assert!(text.contains("$ ls"), "{text}");
        assert!(text.contains("（不进上下文）"), "{text}");
        assert!(text.contains("exit 0"), "{text}");
        assert!(text.contains("exit 2"), "{text}");
        assert!(text.contains("truncated → /tmp/full.log"), "{text}");
        assert!(text.contains("cancelled"), "{text}");

        // 运行中 live 块
        let mut st2 = SessionState::new();
        st2.start_bash("ping", false);
        st2.bash_output.push_str("64 bytes from x\n");
        let lines = chat_lines(&st2, RenderFlags::default());
        let text: String = lines
            .iter()
            .map(|l| {
                let mut s = String::new();
                for sp in &l.spans {
                    s.push_str(sp.content.as_ref());
                }
                s.push('\n');
                s
            })
            .collect();
        assert!(text.contains("$ ping"), "{text}");
        assert!(text.contains("64 bytes from x"), "live 输出实时显示: {text}");
        assert!(text.contains("运行中"), "{text}");
        assert!(!text.contains("exit 0"), "live 无完成态");
    }

    #[test]
    fn bash_output_line_cap_keeps_tail() {
        use pi_rust_tui::session::Entry;
        let mut st = SessionState::new();
        let big: String = (0..600).map(|i| format!("row{i}\n")).collect();
        st.transcript.push(Entry::Bash {
            command: "big".into(),
            output: big,
            exit_code: Some(0),
            cancelled: false,
            truncated: false,
            full_output_path: None,
            excluded: false,
        });
        let lines = chat_lines(&st, RenderFlags::default());
        let text: String = lines
            .iter()
            .map(|l| {
                let mut s = String::new();
                for sp in &l.spans {
                    s.push_str(sp.content.as_ref());
                }
                s.push('\n');
                s
            })
            .collect();
        assert!(text.contains("已截断"), "超限提示: {text}");
        assert!(text.contains("row599"), "保留末尾");
        assert!(!text.contains("row0\n"), "丢弃开头（或至少不含首行）");
    }
}
