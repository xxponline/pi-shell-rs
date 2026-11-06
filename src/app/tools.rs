//! 工具调用卡片渲染（纯函数：块 → 带样式行）。
//!
//! 每张卡片 = 头行（工具名 · 关键参数 · 状态 · 耗时）+ 展开体（结果预览）。
//! 结果形状防御式读取：`content[]` / `structuredContent` / `details` 任一
//! 缺失都降级，绝不 panic——pi 协议会演进，卡片不能脆。

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use pi_shell_rs::session::{ToolExecution, ToolStatus};

/// 折叠态下结果体保留的最大行数。
const COLLAPSED_BODY_LINES: usize = 3;

/// 渲染一张工具卡片。
///
/// 输入: name — 工具名；args_json — 调用参数（可能为流式不完整 JSON）；
///       tool — 执行状态/结果；expanded — 是否展开完整输出（ctrl+o）
/// 输出: 卡片行序列（首行为头行，后续为结果体）
pub fn tool_lines(
    name: &str,
    args_json: &str,
    tool: &ToolExecution,
    expanded: bool,
) -> Vec<Line<'static>> {
    let args = serde_json::from_str::<Value>(args_json).unwrap_or(Value::Null);
    let mut out = vec![head_line(name, &args, tool)];

    let body = match name {
        "read" => read_body(&args, tool),
        "bash" | "powershell" => bash_body(&args, tool),
        "edit" => edit_body(&args, tool),
        "write" => write_body(&args, tool),
        "grep" | "find" | "ls" => text_result_body(tool, name),
        _ => generic_body(&args, tool),
    };
    if expanded {
        out.extend(body);
    } else {
        out.extend(body.into_iter().take(COLLAPSED_BODY_LINES));
    }
    out
}

use serde_json::Value;

/// 头行：`● name · 关键参数 · 状态 · 耗时`。
///
/// 输入: name — 工具名；args — 已解析参数；tool — 执行状态
/// 输出: 单行头行（状态决定颜色：成功绿/失败红/进行中黄）
fn head_line(name: &str, args: &Value, tool: &ToolExecution) -> Line<'static> {
    let (status_text, status_color) = match tool.status {
        ToolStatus::Pending => ("pending", Color::Yellow),
        ToolStatus::Running => ("running", Color::Yellow),
        ToolStatus::Ended if tool.is_error => ("error", Color::Red),
        ToolStatus::Ended => ("done", Color::Green),
    };

    let mut spans = vec![Span::styled(
        "● ",
        Style::default().fg(status_color),
    ), Span::styled(
        name.to_string(),
        Style::default().add_modifier(Modifier::BOLD),
    )];

    if let Some(key) = head_arg(name, args) {
        spans.push(Span::raw(format!(" {key}")));
    }
    spans.push(Span::styled(
        format!(" · {status_text}"),
        Style::default().fg(status_color),
    ));
    if let Some(ms) = tool.duration_ms {
        spans.push(Span::raw(format!(" · {ms}ms")));
    }
    Line::from(spans)
}

/// 按工具类型提取头行的关键参数（含 grep/find 的组合展示）。
///
/// 输入: name — 工具名；args — 已解析参数
/// 输出: 关键参数摘要（截断 60 字符）；无则 None
fn head_arg(name: &str, args: &Value) -> Option<String> {
    match name {
        "grep" => {
            let pattern = args.get("pattern").and_then(|p| p.as_str())?;
            let path = args.get("path").and_then(|p| p.as_str());
            let head = match path {
                Some(p) if !p.is_empty() => format!("/{pattern}/ in {p}"),
                _ => format!("/{pattern}/"),
            };
            Some(truncate(&head, 60))
        }
        "find" => {
            let pattern = args.get("pattern").and_then(|p| p.as_str())?;
            let path = args.get("path").and_then(|p| p.as_str());
            let head = match path {
                Some(p) if !p.is_empty() => format!("{pattern} in {p}"),
                _ => pattern.to_string(),
            };
            Some(truncate(&head, 60))
        }
        _ => {
            let key = match name {
                "read" | "edit" | "write" | "ls" => "path",
                "bash" | "powershell" => "command",
                _ => return None,
            };
            // 兼容 pi 的几种字段命名。
            let raw = args
                .get(key)
                .or_else(|| args.get("filePath"))
                .or_else(|| args.get("file_path"))?
                .as_str()?;
            Some(truncate(raw, 60))
        }
    }
}

/// 字符安全截断（按 char 数）。
///
/// 输入: text — 原文；max — 最大字符数
/// 输出: 截断后的字符串（超长以 … 结尾）
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let taken: String = text.chars().take(max.saturating_sub(1)).collect();
        format!("{taken}…")
    }
}

/// 读取结果中的文本正文（content[].text 拼接）。
///
/// 输入: tool — 执行结果（result 可能为 None）
/// 输出: Some(正文)；无内容返回 None
fn result_text(tool: &ToolExecution) -> Option<String> {
    let result = tool.result.as_ref()?;
    let items = result.get("content")?.as_array()?;
    let parts: Vec<&str> = items
        .iter()
        .filter_map(|i| i.get("text").and_then(|t| t.as_str()))
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(""))
    }
}

/// 把多行文本转为暗色结果行（保留空行占位）。
///
/// 输入: text — 多行正文
/// 输出: 行序列（默认样式）
fn text_lines(text: &str) -> Vec<Line<'static>> {
    let mut out: Vec<Line> = Vec::new();
    for l in text.lines() {
        out.push(Line::from(Span::raw(l.to_string())));
    }
    out
}

// ---------- 各工具结果体 ----------

/// read：文件内容预览（structuredContent 优先——实测为文件原文字符串，
/// 退回 content[].text）。
fn read_body(_args: &Value, tool: &ToolExecution) -> Vec<Line<'static>> {
    let result = tool.result.as_ref();
    if let Some(Value::String(content)) = result.and_then(|r| r.get("structuredContent")) {
        return text_lines(content);
    }
    result_text(tool)
        .map(|t| text_lines(&t))
        .unwrap_or_default()
}

/// bash：命令输出（structuredContent.output 优先，含 exit_code 提示）。
fn bash_body(_args: &Value, tool: &ToolExecution) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let result = tool.result.as_ref();
    let structured = result.and_then(|r| r.get("structuredContent"));

    if let Some(exit) = structured.and_then(|s| s.get("exit_code")).and_then(|x| x.as_i64()) {
        let color = if exit == 0 { Color::Green } else { Color::Red };
        out.push(Line::from(Span::styled(
            format!("exit {exit}"),
            Style::default().fg(color),
        )));
    }

    let output = structured
        .and_then(|s| s.get("output"))
        .and_then(|o| o.as_str())
        .map(String::from)
        .or_else(|| result_text(tool));

    if let Some(output) = output {
        out.extend(text_lines(output.trim_end()));
        if structured
            .and_then(|s| s.get("truncated"))
            .and_then(|t| t.as_bool())
            .unwrap_or(false)
        {
            out.push(Line::from(Span::styled(
                "… output truncated",
                Style::default().fg(Color::DarkGray),
            )));
        }
    }
    out
}

/// edit：details.diff 字符串按行着色（`-N` 红 / `+N` 绿 / `@@` 青）；
/// 缺失时对 args 的 old/new 文本做行级 LCS 对比降级。
fn edit_body(args: &Value, tool: &ToolExecution) -> Vec<Line<'static>> {
    let details = tool
        .result
        .as_ref()
        .and_then(|r| r.get("details"));

    if let Some(diff) = details.and_then(|d| d.get("diff")).and_then(|d| d.as_str()) {
        return diff
            .lines()
            .map(|l| {
                let style = if l.starts_with('-') {
                    Style::default().fg(Color::Red)
                } else if l.starts_with('+') {
                    Style::default().fg(Color::Green)
                } else if l.starts_with("@@") {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default()
                };
                Line::from(Span::styled(l.to_string(), style))
            })
            .collect();
    }

    // 降级：args.oldString/newString 行级对比。
    let old = args.get("oldString").and_then(|v| v.as_str()).unwrap_or("");
    let new = args.get("newString").and_then(|v| v.as_str()).unwrap_or("");
    line_diff(old, new)
}

/// 行级 LCS 对比，产出 `-old`/`+new` 风格行（diff 降级路径）。
///
/// 输入: old — 旧文本；new — 新文本（行数超 400 时降级为整块红绿）
/// 输出: 带红绿样式的 diff 行
fn line_diff(old: &str, new: &str) -> Vec<Line<'static>> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();

    // 行数过大时不做 O(n*m)，直接整块对比。
    if a.len().saturating_mul(b.len()) > 160_000 {
        let mut out: Vec<Line> = Vec::new();
        for l in &a {
            out.push(Line::from(Span::styled(
                format!("-{l}"),
                Style::default().fg(Color::Red),
            )));
        }
        for l in &b {
            out.push(Line::from(Span::styled(
                format!("+{l}"),
                Style::default().fg(Color::Green),
            )));
        }
        return out;
    }

    // 经典 LCS DP。
    let (n, m) = (a.len(), b.len());
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if a[i] == b[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0usize, 0usize);
    let mut out: Vec<Line> = Vec::new();
    while i < n && j < m {
        if a[i] == b[j] {
            out.push(Line::from(Span::raw(a[i].to_string())));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            out.push(Line::from(Span::styled(
                format!("-{}", a[i]),
                Style::default().fg(Color::Red),
            )));
            i += 1;
        } else {
            out.push(Line::from(Span::styled(
                format!("+{}", b[j]),
                Style::default().fg(Color::Green),
            )));
            j += 1;
        }
    }
    for l in &a[i..] {
        out.push(Line::from(Span::styled(
            format!("-{l}"),
            Style::default().fg(Color::Red),
        )));
    }
    for l in &b[j..] {
        out.push(Line::from(Span::styled(
            format!("+{l}"),
            Style::default().fg(Color::Green),
        )));
    }
    out
}

/// write：优先 args.content 预览（写入内容），退回结果文本。
fn write_body(args: &Value, tool: &ToolExecution) -> Vec<Line<'static>> {
    if let Some(content) = args.get("content").and_then(|c| c.as_str())
        && !content.is_empty()
    {
        return text_lines(content.trim_end());
    }
    result_text(tool)
        .map(|t| text_lines(&t))
        .unwrap_or_default()
}

/// grep/find/ls 共用：结果文本行 + details 截断提示。
///
/// 输入: tool — 执行结果；name — 工具名（ls 无 pattern 语义，仅取结果）
/// 输出: 行序列；结果缺失时为空（由上层降级为头行+args）
fn text_result_body(tool: &ToolExecution, name: &str) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    if let Some(text) = result_text(tool) {
        out.extend(text_lines(text.trim_end()));
    }
    // grep 等可能带 details.linesTruncated 提示。
    if let Some(n) = tool
        .result
        .as_ref()
        .and_then(|r| r.get("details"))
        .and_then(|d| d.get("linesTruncated"))
        .and_then(|x| x.as_u64())
        && n > 0
    {
        out.push(Line::from(Span::styled(
            format!("… {n} lines truncated"),
            Style::default().fg(Color::DarkGray),
        )));
    }
    let _ = name;
    out
}

/// 通用卡片结果体：args 摘要 + 结果正文摘要。
fn generic_body(args: &Value, tool: &ToolExecution) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let args_str = args.to_string();
    if args_str != "null" {
        out.push(Line::from(Span::styled(
            format!("args: {}", truncate(&args_str, 160)),
            Style::default().fg(Color::DarkGray),
        )));
    }
    if let Some(text) = result_text(tool) {
        out.extend(text_lines(&truncate(&text, 400)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pi_shell_rs::session::{replay_file, Entry};
    use std::path::PathBuf;

    /// 从真实 fixture 回放中取出指定工具的 ToolCall 块。
    fn fixture_tool_call(scenario: &str, tool_name: &str) -> (String, String, ToolExecution) {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/probe")
            .join(scenario)
            .join("snapshot/transcript.jsonl");
        let state = replay_file(&path).expect("replay fixture");
        for entry in state.entries() {
            if let Entry::Assistant { blocks, .. } = entry {
                for b in blocks {
                    if let Block::ToolCall {
                        name,
                        args_json,
                        tool,
                        ..
                    } = b
                        && name == tool_name {
                            return (name.clone(), args_json.clone(), tool.clone());
                        }
                }
            }
        }
        panic!("tool {tool_name} not found in {scenario}");
    }

    use pi_shell_rs::session::Block;

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

    #[test]
    fn unknown_tool_renders_generic_card() {
        let tool = ToolExecution {
            status: ToolStatus::Ended,
            is_error: false,
            result: Some(serde_json::json!({"content":[{"type":"text","text":"ok result"}]})),
            duration_ms: Some(5),
        };
        let lines = tool_lines("mystery_tool", r#"{"foo":1}"#, &tool, true);
        let text = render(&lines);
        assert!(text.contains("mystery_tool"), "{text}");
        assert!(text.contains("done"), "{text}");
        assert!(text.contains("5ms"), "{text}");
        assert!(text.contains("args:"), "{text}");
        assert!(text.contains("ok result"), "{text}");
    }

    #[test]
    fn failed_tool_head_is_red() {
        let tool = ToolExecution {
            status: ToolStatus::Ended,
            is_error: true,
            result: None,
            duration_ms: None,
        };
        let lines = tool_lines("read", r#"{"path":"/x"}"#, &tool, false);
        assert!(render(&lines).contains("error"));
        let status_span = lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref().contains("error"))
            .unwrap();
        assert_eq!(status_span.style.fg, Some(Color::Red));
    }

    #[test]
    fn read_card_from_fixture() {
        let (name, args, tool) = fixture_tool_call("tool_call", "read");
        let lines = tool_lines(&name, &args, &tool, true);
        let text = render(&lines);
        assert!(text.contains("read"), "{text}");
        assert!(text.contains("Cargo.toml"), "路径应出现: {text}");
        // 内容预览包含文件正文片段（fixture 为归档数据，正文是当时的 Cargo.toml）
        assert!(text.contains("[package]"), "内容预览: {text}");
        assert!(text.contains("done"), "{text}");
    }

    #[test]
    fn bash_card_from_fixture() {
        let (name, args, tool) = fixture_tool_call("tool_bash", "bash");
        let lines = tool_lines(&name, &args, &tool, true);
        let text = render(&lines);
        assert!(text.contains("bash"), "{text}");
        assert!(text.contains("echo hello-bash-fixture"), "命令摘要: {text}");
        assert!(text.contains("exit 0"), "退出码: {text}");
        assert!(text.contains("hello-bash-fixture"), "输出: {text}");
    }

    #[test]
    fn edit_card_from_fixture_shows_colored_diff() {
        let (name, args, tool) = fixture_tool_call("tool_edit", "edit");
        let lines = tool_lines(&name, &args, &tool, true);
        let text = render(&lines);
        assert!(text.contains("edit"), "{text}");
        assert!(text.contains("probe-tmp-edit.txt"), "路径: {text}");
        assert!(text.contains("-1 FIXTURE-MARKER"), "旧 diff 行: {text}");
        assert!(text.contains("+1 EDITED-MARKER"), "新 diff 行: {text}");
        // 红绿着色
        let has_red = lines.iter().flat_map(|l| l.spans.iter()).any(|s| {
            s.style.fg == Some(Color::Red) && s.content.as_ref().starts_with('-')
        });
        let has_green = lines.iter().flat_map(|l| l.spans.iter()).any(|s| {
            s.style.fg == Some(Color::Green) && s.content.as_ref().starts_with('+')
        });
        assert!(has_red, "diff 应有红色删除行");
        assert!(has_green, "diff 应有绿色新增行");
    }

    #[test]
    fn edit_without_details_falls_back_to_line_diff() {
        let tool = ToolExecution {
            status: ToolStatus::Ended,
            is_error: false,
            result: Some(serde_json::json!({"content":[{"type":"text","text":"ok"}]})),
            duration_ms: None,
        };
        let args = r#"{"file_path":"/x","oldString":"alpha\nbeta","newString":"alpha\ngamma"}"#;
        let lines = tool_lines("edit", args, &tool, true);
        let text = render(&lines);
        assert!(text.contains("-beta"), "{text}");
        assert!(text.contains("+gamma"), "{text}");
        assert!(text.contains("alpha"), "未变行原样: {text}");
    }

    #[test]
    fn collapsed_body_is_limited() {
        // read fixture 内容超过折叠行数，能体现折叠/展开差异。
        let (name, args, tool) = fixture_tool_call("tool_call", "read");
        let collapsed = tool_lines(&name, &args, &tool, false);
        let expanded = tool_lines(&name, &args, &tool, true);
        assert!(collapsed.len() <= 1 + COLLAPSED_BODY_LINES, "{}", collapsed.len());
        assert!(expanded.len() > collapsed.len(), "展开应更多行");
        // bash 结果只有 2 行：折叠态不截断（≤3 行全显示）
        let (bn, ba, bt) = fixture_tool_call("tool_bash", "bash");
        let bc = tool_lines(&bn, &ba, &bt, false);
        assert!(render(&bc).contains("hello-bash-fixture"));
    }

    #[test]
    fn partial_json_args_do_not_panic() {
        let tool = ToolExecution {
            status: ToolStatus::Running,
            is_error: false,
            result: None,
            duration_ms: None,
        };
        // 流式中不完整 JSON
        let lines = tool_lines("bash", r#"{"command":"ec"#, &tool, false);
        let text = render(&lines);
        assert!(text.contains("bash"));
        assert!(text.contains("running"));
    }

    #[test]
    fn write_card_shows_path_and_content_preview() {
        // tool_edit fixture 里包含真实的 write 调用
        let (name, args, tool) = fixture_tool_call("tool_edit", "write");
        let lines = tool_lines(&name, &args, &tool, true);
        let text = render(&lines);
        assert!(text.contains("write"), "{text}");
        assert!(text.contains("probe-tmp-edit.txt"), "路径: {text}");
        assert!(text.contains("FIXTURE-MARKER"), "内容预览来自 args.content: {text}");

        // content 缺失 → 退回 result 文本
        let tool_no_content = ToolExecution {
            status: ToolStatus::Ended,
            is_error: false,
            result: Some(serde_json::json!({"content":[{"type":"text","text":"Successfully wrote to /x"}]})),
            duration_ms: None,
        };
        let lines = tool_lines("write", r#"{"path":"/x"}"#, &tool_no_content, true);
        assert!(render(&lines).contains("Successfully wrote"));
    }

    #[test]
    fn grep_card_shows_pattern_path_output_and_truncation() {
        let tool = ToolExecution {
            status: ToolStatus::Ended,
            is_error: false,
            result: Some(serde_json::json!({
                "content":[{"type":"text","text":"src/a.rs:12: fn foo()\nsrc/b.rs:40: fn food()"}],
                "details":{"linesTruncated": 7}
            })),
            duration_ms: Some(9),
        };
        let args = r#"{"pattern":"foo","path":"src"}"#;
        let lines = tool_lines("grep", args, &tool, true);
        let text = render(&lines);
        assert!(text.contains("/foo/ in src"), "头行: {text}");
        assert!(text.contains("src/a.rs:12"), "输出行: {text}");
        assert!(text.contains("7 lines truncated"), "截断提示: {text}");
        assert!(text.contains("done") && text.contains("9ms"), "状态/耗时: {text}");
    }

    #[test]
    fn find_and_ls_cards_render() {
        let find_tool = ToolExecution {
            status: ToolStatus::Ended,
            is_error: false,
            result: Some(serde_json::json!({"content":[{"type":"text","text":"./src/main.rs\n./src/lib.rs"}]})),
            duration_ms: None,
        };
        let lines = tool_lines("find", r#"{"pattern":"*.rs","path":"."}"#, &find_tool, true);
        let text = render(&lines);
        assert!(text.contains("*.rs in ."), "find 头行: {text}");
        assert!(text.contains("./src/main.rs"), "find 结果: {text}");

        let ls_tool = ToolExecution {
            status: ToolStatus::Ended,
            is_error: false,
            result: Some(serde_json::json!({"content":[{"type":"text","text":"Cargo.toml\nsrc/\ndocs/"}]})),
            duration_ms: None,
        };
        let lines = tool_lines("ls", r#"{"path":"."}"#, &ls_tool, true);
        let text = render(&lines);
        assert!(text.contains("ls"), "{text}");
        assert!(text.contains("Cargo.toml"), "ls 结果: {text}");
    }

    #[test]
    fn new_cards_degrade_when_result_missing() {
        // 结果缺失 + 未知/缺失字段 → 头行 + 通用降级，不 panic
        let bare = ToolExecution {
            status: ToolStatus::Ended,
            is_error: true,
            result: None,
            duration_ms: None,
        };
        for (name, args) in [
            ("grep", "{}"),
            ("find", "{}"),
            ("ls", "{}"),
            ("write", "{}"),
            ("grep", "not-json"),
        ] {
            let lines = tool_lines(name, args, &bare, true);
            let text = render(&lines);
            assert!(text.contains(name), "{name}: {text}");
            assert!(text.contains("error"), "{name}: {text}");
        }
    }

    #[test]
    fn ctrl_o_expands_new_cards() {
        let tool = ToolExecution {
            status: ToolStatus::Ended,
            is_error: false,
            result: Some(serde_json::json!({"content":[{"type":"text","text":"l1\nl2\nl3\nl4\nl5\nl6"}]})),
            duration_ms: None,
        };
        let collapsed = tool_lines("ls", r#"{"path":"."}"#, &tool, false);
        let expanded = tool_lines("ls", r#"{"path":"."}"#, &tool, true);
        assert!(collapsed.len() <= 1 + COLLAPSED_BODY_LINES, "{}", collapsed.len());
        assert!(expanded.len() > collapsed.len());
        assert!(!render(&collapsed).contains("l6"));
        assert!(render(&expanded).contains("l6"));
    }
}
