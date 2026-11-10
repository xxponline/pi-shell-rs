//! 会话状态 → 可渲染 ratatui 行的纯映射（markdown/工具卡片/thinking/通知）。
//!
//! 渲染偏好通过 [`RenderFlags`] 传入（不进 SessionState——它们是 UI 偏好，
//! 不是会话事实），保证相同状态 + 相同 flags 产出相同内容，可离线单测。

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use pi_shell_rs::session::{Block, Entry, SessionState, ToolExecution, ToolStatus};

use crate::app::wrap::HitKind;

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
/// 输入: state — 会话状态；flags — 渲染偏好（thinking/工具展开）；
///       overrides — 单卡工具展开覆盖表（toolCallId → 是否展开，
///       无覆盖的卡跟随 flags.expand_tools，design D1）
/// 输出: `(行序列, 逐行命中 meta)`——meta 与 lines 等长，仅**已有最终结果**
///       的工具卡行记 Some(Tool(id))、非空 thinking 块整段记
///       Some(Thinking(seq))（design D1/D2）；其余（用户行/markdown/
///       notice/`!` bash）为 None；thinking_overrides — 单块展开覆盖
///       （seq → 是否展开，无覆盖跟随 flags.show_thinking，design D3）
pub fn chat_lines(
    state: &SessionState,
    flags: RenderFlags,
    overrides: &std::collections::HashMap<String, bool>,
    thinking_overrides: &std::collections::HashMap<usize, bool>,
) -> (Vec<Line<'static>>, Vec<Option<HitKind>>) {
    let mut lines = Vec::new();
    let mut meta: Vec<Option<HitKind>> = Vec::new();
    // thinking 全局渲染序号（design D2：无条件计数，跨流式/落盘稳定）。
    let mut thinking_seq = 0usize;
    for entry in state.entries() {
        match entry {
            Entry::User { text } => {
                lines.push(prefixed("You", text, Color::Cyan, true));
            }
            Entry::Assistant { blocks, .. } => {
                append_blocks(
                    &mut lines,
                    &mut meta,
                    blocks,
                    flags,
                    overrides,
                    thinking_overrides,
                    &mut thinking_seq,
                );
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
        // 非卡片路径补齐 None（与行等长；append_blocks 已自同步）。
        meta.resize(lines.len(), None);
    }
    // 流式中尚未落盘的内容。
    if state.is_in_flight() {
        append_blocks(
            &mut lines,
            &mut meta,
            state.in_flight(),
            flags,
            overrides,
            thinking_overrides,
            &mut thinking_seq,
        );
    }
    // 运行中的用户 bash（分片实时增长）。
    if state.bash_running {
        let excluded = state.bash_excluded;
        let command = state.bash_command.clone();
        let output = state.bash_output.clone();
        append_bash(&mut lines, &command, &output, None, false, false, None, excluded, true);
    }
    meta.resize(lines.len(), None);
    debug_assert_eq!(lines.len(), meta.len(), "meta 必须与行等长");
    (lines, meta)
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
/// 输入: out/meta — 行与逐行命中标识收集器（design D1）；blocks — 内容块；
///       flags — 渲染偏好；overrides — 单卡工具覆盖；thinking_overrides —
///       thinking 单块覆盖；thinking_seq — 全局 thinking 序号游标
///       （无条件递增、跨调用穿透，design D2）
/// 输出: 无返回值；副作用为向 out/meta 追加
fn append_blocks(
    out: &mut Vec<Line<'static>>,
    meta: &mut Vec<Option<HitKind>>,
    blocks: &[Block],
    flags: RenderFlags,
    overrides: &std::collections::HashMap<String, bool>,
    thinking_overrides: &std::collections::HashMap<usize, bool>,
    thinking_seq: &mut usize,
) {
    for block in blocks {
        let start = out.len();
        // 本块命中标识（None = 不入命中表，design D1）。
        let mut mark: Option<HitKind> = None;
        match block {
            Block::Text { text } => {
                if !text.is_empty() {
                    // assistant 文本按 markdown 渲染（流式未闭合语法安全降级）。
                    out.extend(crate::app::markdown::render_markdown(text));
                }
            }
            Block::Thinking { text } => {
                // 每个 thinking 块无条件消耗一个全局序号（含空块——空块后
                // 接 delta 变非空时既有序号不重排，design D2）。
                let seq = *thinking_seq;
                *thinking_seq += 1;
                if text.is_empty() {
                    continue;
                }
                mark = Some(HitKind::Thinking(seq));
                // 生效值：单块覆盖优先、否则全局（design D3，与工具 D1 同构）。
                let expanded = flags.show_thinking
                    || thinking_overrides.get(&seq).copied().unwrap_or(false);
                if expanded {
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
            Block::ToolCall { id, name, args_json, tool } => {
                // 工具调用渲染为结构化卡片（展开态 = 单卡覆盖优先、否则
                // 跟随全局 flags.expand_tools，design D1）；背景面板按状态
                // 着色（design D5），状态文字/圆点保留（spec #16）。
                let expanded = *overrides.get(id).unwrap_or(&flags.expand_tools);
                let bg = tool_panel_bg(tool);
                for mut line in crate::app::tools::tool_lines(name, args_json, tool, expanded) {
                    line.style = Style::default().bg(bg);
                    out.push(line);
                }
                // 门控前移到建表：仅已有最终结果的卡入表（运行中不入，D1）。
                if tool.result.is_some() {
                    mark = Some(HitKind::Tool(id.clone()));
                }
            }
        }
        // 逐块同步 meta（design D1/D2）：命中块的全部行整段打标（门控前移
        // 到建表，分发无需回查 state）。
        meta.resize(out.len(), None);
        if let Some(kind) = mark {
            for m in &mut meta[start..] {
                *m = Some(kind.clone());
            }
        }
    }
}

/// 工具卡片背景面板色（design D5）：由 pi dark.json 的 okhsl 转 RGB——
/// `toolPendingBg` okhsl(229 5% 24%) 运行灰、`toolSuccessBg` okhsl(158 46% 25%)
/// 成功墨绿、`toolErrorBg` okhsl(19 54% 25%) 失败暗红（转换经 pi oklab.ts
/// 权威算法核算）。
///
/// 输入: tool — 执行状态（读 status/is_error）
/// 输出: 面板背景色（spec #16：背景叠加、不替代状态文字）
fn tool_panel_bg(tool: &ToolExecution) -> Color {
    match tool.status {
        ToolStatus::Pending | ToolStatus::Running => Color::Rgb(52, 56, 58),
        ToolStatus::Ended if tool.is_error => Color::Rgb(91, 40, 42),
        ToolStatus::Ended => Color::Rgb(37, 65, 49),
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
    use pi_shell_rs::session::SessionState;
    use serde_json::json;

    #[test]
    fn renders_user_and_assistant_text() {
        let mut s = SessionState::new();
        s.handle(&json!({"type":"message_start","message":{"role":"user","content":[{"type":"text","text":"hello"}]}}));
        s.handle(&json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        s.handle(&json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"hi there"}]}}));
        let (lines, _) = chat_lines(&s, RenderFlags::default(), &std::collections::HashMap::new(), &std::collections::HashMap::new());
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
        let (lines, _) = chat_lines(&s, RenderFlags::default(), &std::collections::HashMap::new(), &std::collections::HashMap::new());
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
        let (lines, _) = chat_lines(&s, RenderFlags::default(), &std::collections::HashMap::new(), &std::collections::HashMap::new());
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
        let (lines, _) = chat_lines(&st, RenderFlags::default(), &std::collections::HashMap::new(), &std::collections::HashMap::new());
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
        st.transcript.push(pi_shell_rs::session::Entry::Notice {
            kind: "info".to_string(),
            text: "something".to_string(),
        });
        let (lines, _) = chat_lines(&st, RenderFlags::default(), &std::collections::HashMap::new(), &std::collections::HashMap::new());
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
        let (lines, _) = chat_lines(&s, RenderFlags::default(), &std::collections::HashMap::new(), &std::collections::HashMap::new());
        let text = render_to_string(&lines);
        assert!(text.contains("<thinking 20 chars>"), "{text}");
        assert!(!text.contains("secret thoughts"), "折叠态不得展开全文: {text}");
    }

    #[test]
    fn thinking_expanded_shows_full_text() {
        let s = thinking_state();
        let flags = RenderFlags { show_thinking: true, expand_tools: false };
        let (lines, _) = chat_lines(&s, flags, &std::collections::HashMap::new(), &std::collections::HashMap::new());
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
        let expanded = render_to_string(&chat_lines(&s, on, &std::collections::HashMap::new(), &std::collections::HashMap::new()).0);
        let collapsed = render_to_string(&chat_lines(&s, off, &std::collections::HashMap::new(), &std::collections::HashMap::new()).0);
        assert!(expanded.contains("secret thoughts"));
        assert!(collapsed.contains("<thinking"));
    }

    #[test]
    fn expand_tools_flag_propagates_to_tool_cards() {
        use pi_shell_rs::session::{Block, ToolExecution, ToolStatus};
        let mut s = SessionState::new();
        s.handle(&serde_json::json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        // 构造一个多行结果的工具块（read 折叠头部 10 行，造 15 行体现差异）
        let text: String = (1..=15).map(|i| format!("l{i}\n")).collect();
        let result = serde_json::json!({"content":[{"type":"text","text": text}]});
        s.transcript.push(pi_shell_rs::session::Entry::Assistant {
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
                    ..Default::default()
                },
            }],
            usage: None,
        });
        let collapsed = render_to_string(&chat_lines(&s, RenderFlags::default(), &std::collections::HashMap::new(), &std::collections::HashMap::new()).0);
        let expanded = render_to_string(&chat_lines(
            &s,
            RenderFlags { show_thinking: false, expand_tools: true },
            &std::collections::HashMap::new(),
            &std::collections::HashMap::new(),
        ).0);
        assert!(collapsed.contains("l1"), "{collapsed}");
        assert!(!collapsed.contains("l11"), "折叠态不应到 l11: {collapsed}");
        assert!(collapsed.contains("more lines, ctrl+o to expand"), "提示行: {collapsed}");
        assert!(expanded.contains("l15"), "展开态应含 l15: {expanded}");
    }

    /// 任务 1.1：单卡展开覆盖——无覆盖跟随全局、覆盖优先于全局、
    /// 未知 id 覆盖不生效（design D1 生效值规则）。
    #[test]
    fn tool_expanded_override_follows_global_then_wins() {
        use pi_shell_rs::session::{Block, ToolExecution, ToolStatus};
        let mut s = SessionState::new();
        s.handle(&serde_json::json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        // 多行结果的 read 卡（折叠头 10 行，l11+ 仅展开可见）
        let text: String = (1..=15).map(|i| format!("l{i}\n")).collect();
        let result = serde_json::json!({"content":[{"type":"text","text": text}]});
        s.transcript.push(pi_shell_rs::session::Entry::Assistant {
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
                    ..Default::default()
                },
            }],
            usage: None,
        });
        let no = std::collections::HashMap::new();
        let mut ov = std::collections::HashMap::new();
        let on = RenderFlags { show_thinking: false, expand_tools: true };

        // ① 无覆盖：跟随全局（折叠无 l11 / 展开有 l15）
        let off_text = render_to_string(&chat_lines(&s, RenderFlags::default(), &no, &std::collections::HashMap::new()).0);
        assert!(!off_text.contains("l11"), "全局折叠: {off_text}");
        let on_text = render_to_string(&chat_lines(&s, on, &no, &std::collections::HashMap::new()).0);
        assert!(on_text.contains("l15"), "全局展开: {on_text}");

        // ② 覆盖优先：全局折叠 + 覆盖 true → 展开
        ov.insert("c1".to_string(), true);
        let override_on = render_to_string(&chat_lines(&s, RenderFlags::default(), &ov, &std::collections::HashMap::new()).0);
        assert!(override_on.contains("l15"), "覆盖 true 压过全局折叠: {override_on}");

        // ②' 覆盖 false + 全局展开 → 折叠
        ov.insert("c1".to_string(), false);
        let override_off = render_to_string(&chat_lines(&s, on, &ov, &std::collections::HashMap::new()).0);
        assert!(!override_off.contains("l11"), "覆盖 false 压过全局展开: {override_off}");

        // 覆盖表对未知 id 无效
        let mut ov_unknown = std::collections::HashMap::new();
        ov_unknown.insert("nope".to_string(), true);
        let unknown = render_to_string(&chat_lines(&s, RenderFlags::default(), &ov_unknown, &std::collections::HashMap::new()).0);
        assert!(!unknown.contains("l11"), "未知 id 覆盖不生效: {unknown}");
    }

    /// 任务 2.2：thinking meta——非空块整段标 Thinking(seq)；空块消耗
    /// 序号但不打标（空→delta 变非空时既有序号不重排，design D2）；
    /// 两次渲染产出完全一致（稳定键前提）。
    #[test]
    fn thinking_meta_marks_nonempty_and_seq_skips_nothing() {
        let mut s = SessionState::new();
        s.transcript.push(Entry::User { text: "hi".into() });
        s.transcript.push(Entry::Assistant {
            turn: 1,
            blocks: vec![
                Block::Thinking { text: String::new() }, // 序号 0：空块，不打标
                Block::Thinking { text: "secret thoughts".into() }, // 序号 1
                Block::Text { text: "after".into() },    // 不占序号
                Block::Thinking { text: "second".into() }, // 序号 2
            ],
            usage: None,
        });

        let empty: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
        let empty_t: std::collections::HashMap<usize, bool> = std::collections::HashMap::new();
        let (lines, meta) = chat_lines(&s, RenderFlags::default(), &empty, &empty_t);
        assert_eq!(lines.len(), meta.len(), "meta 必须与行等长");
        let marks: Vec<HitKind> = meta.iter().flatten().cloned().collect();
        assert_eq!(
            marks,
            vec![HitKind::Thinking(1), HitKind::Thinking(2)],
            "空块（序号 0）不打标、非空块整段打标"
        );

        // 两次渲染产出完全一致（序号稳定，design D2）
        let (lines2, meta2) = chat_lines(&s, RenderFlags::default(), &empty, &empty_t);
        assert_eq!(lines, lines2, "行产出确定性");
        assert_eq!(meta, meta2, "meta 产出确定性");
    }

    /// 任务 2.1：命中 meta 门控——仅已有最终结果的工具卡入表，
    /// 运行中卡/`!` bash/普通行全 None，且 meta 与行等长（design D2）。
    #[test]
    fn card_meta_only_marks_finished_tool_cards() {
        use pi_shell_rs::session::{Block, ToolExecution, ToolStatus};
        let mut s = SessionState::new();
        let ended = ToolExecution {
            status: ToolStatus::Ended,
            is_error: false,
            result: Some(serde_json::json!({"content":[{"type":"text","text":"ok"}]})),
            duration_ms: Some(3),
            ..Default::default()
        };
        let running = ToolExecution {
            status: ToolStatus::Running,
            ..Default::default()
        };
        s.transcript.push(pi_shell_rs::session::Entry::User {
            text: "question".into(),
        });
        s.transcript.push(pi_shell_rs::session::Entry::Assistant {
            turn: 1,
            blocks: vec![
                Block::ToolCall {
                    id: "c1".into(),
                    name: "read".into(),
                    args_json: r#"{"path":"/a"}"#.into(),
                    tool: ended,
                },
                Block::ToolCall {
                    id: "c2".into(),
                    name: "bash".into(),
                    args_json: r#"{"command":"x"}"#.into(),
                    tool: running,
                },
            ],
            usage: None,
        });
        s.transcript.push(pi_shell_rs::session::Entry::Bash {
            command: "echo hi".into(),
            output: "hi".into(),
            exit_code: Some(0),
            cancelled: false,
            truncated: false,
            full_output_path: None,
            excluded: false,
        });

        let no = std::collections::HashMap::new();
        let (lines, meta) = chat_lines(&s, RenderFlags::default(), &no, &std::collections::HashMap::new());
        assert_eq!(lines.len(), meta.len(), "meta 必须与行等长");

        let marked: Vec<HitKind> = meta.iter().flatten().cloned().collect();
        assert!(!marked.is_empty(), "结果卡 c1 应入表");
        assert!(
            marked.iter().all(|m| *m == HitKind::Tool("c1".into())),
            "只标 c1——运行中 c2 与普通行不入表: {marked:?}"
        );

        // `!` bash 行明确为 None
        let bash_idx = lines
            .iter()
            .position(|l| l.spans.iter().any(|sp| sp.content.as_ref().contains("echo hi")))
            .expect("bash 行应存在");
        assert!(meta[bash_idx].is_none(), "`!` bash 行不入表");
        // 用户行同样 None
        assert!(meta[0].is_none(), "用户行不入表");
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
        use pi_shell_rs::session::Entry;
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
        let (lines, _) = chat_lines(&st, RenderFlags::default(), &std::collections::HashMap::new(), &std::collections::HashMap::new());
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
        let (lines, _) = chat_lines(&st2, RenderFlags::default(), &std::collections::HashMap::new(), &std::collections::HashMap::new());
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
        use pi_shell_rs::session::Entry;
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
        let (lines, _) = chat_lines(&st, RenderFlags::default(), &std::collections::HashMap::new(), &std::collections::HashMap::new());
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

    /// design D5 / spec 状态背景面板：三态着色、状态文字保留、非卡片行无背景。
    #[test]
    fn tool_card_panel_bg_three_states() {
        let mk = |status: ToolStatus, is_error: bool| Block::ToolCall {
            id: "c1".into(),
            name: "bash".into(),
            args_json: r#"{"command":"ls"}"#.into(),
            tool: ToolExecution {
                status,
                is_error,
                result: Some(serde_json::json!({"content":[{"type":"text","text":"out"}]})),
                ..Default::default()
            },
        };
        let mut s = SessionState::new();
        s.handle(&serde_json::json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        s.transcript.push(Entry::Assistant {
            turn: 1,
            blocks: vec![
                mk(ToolStatus::Running, false),
                mk(ToolStatus::Ended, false),
                mk(ToolStatus::Ended, true),
            ],
            usage: None,
        });
        let (lines, _) = chat_lines(&s, RenderFlags::default(), &std::collections::HashMap::new(), &std::collections::HashMap::new());

        // 每张卡片的所有行带对应 bg，且状态文字保留（spec #16）
        let expect = [
            Color::Rgb(52, 56, 58),  // 运行灰
            Color::Rgb(37, 65, 49),  // 成功墨绿
            Color::Rgb(91, 40, 42),  // 失败暗红
        ];
        let status_texts = ["running", "done", "error"];
        for (i, (&bg, status)) in expect.iter().zip(status_texts.iter()).enumerate() {
            // 找到该卡片的起始行（含工具名的头行）
            let start = lines
                .iter()
                .position(|l| {
                    l.spans.iter().any(|sp| sp.content.as_ref() == "bash")
                        && l.spans.iter().any(|sp| sp.content.as_ref().contains(status))
                })
                .unwrap_or_else(|| panic!("卡 {i} 头行未找到"));
            // 卡片行 = 从头行起连续带该 bg 的行
            let mut j = start;
            while j < lines.len() && lines[j].style.bg == Some(bg) {
                j += 1;
            }
            assert!(j > start, "卡 {i} 头行应带面板 bg");
            assert!(
                j == lines.len() || lines[j].style.bg != Some(bg),
                "卡 {i} 面板在卡片结束处终止"
            );
            // 状态文字仍可见（背景不替代文字，spec #16）
            let head_text: String = lines[start]
                .spans
                .iter()
                .map(|sp| sp.content.as_ref())
                .collect();
            assert!(head_text.contains(status), "状态文字保留: {head_text}");
        }

        // 非卡片行（用户消息等）无背景
        let mut s2 = SessionState::new();
        s2.handle(&serde_json::json!({"type":"message_start","message":{"role":"user","content":[{"type":"text","text":"hello"}]}}));
        s2.handle(&serde_json::json!({"type":"message_end","message":{"role":"user","content":[{"type":"text","text":"hello"}]}}));
        let (lines2, _) = chat_lines(&s2, RenderFlags::default(), &std::collections::HashMap::new(), &std::collections::HashMap::new());
        assert!(
            lines2.iter().all(|l| l.style.bg.is_none()),
            "非卡片行不应有背景"
        );
    }
}
