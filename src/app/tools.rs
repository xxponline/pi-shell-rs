//! 工具调用卡片渲染（纯函数：块 → 带样式行）。
//!
//! 每张卡片 = 头行（工具名 · 关键参数 · 状态 · 耗时）+ 展开体（结果预览）。
//! 结果形状防御式读取：`content[]` / `structuredContent` / `details` 任一
//! 缺失都降级，绝不 panic——pi 协议会演进，卡片不能脆。

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use pi_shell_rs::session::{ToolExecution, ToolStatus};

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
    let mut out = vec![head_line(name, &args, tool, expanded)];

    let body = match name {
        "read" => read_body(&args, tool),
        "bash" | "powershell" => bash_output(&args, tool),
        "edit" => edit_body(&args, tool),
        "write" => write_body(&args, tool),
        "grep" | "find" | "ls" => text_result_body(tool),
        _ => generic_body(&args, tool, expanded),
    };
    if expanded {
        out.extend(body);
    } else {
        out.extend(collapse(name, body));
    }
    // 受保护尾部：exit 状态行 + 截断警告——不参与折叠计数、始终可见
    //（对照 pi：warnings 在 preview 之外渲染；也使 bash 隐藏行数只计输出行）。
    out.extend(protected_tail(name, tool));
    out
}

/// 受保护尾部（不参与折叠）：bash 的 exit 状态行 + 各工具截断警告行。
///
/// 输入: name — 工具名；tool — 执行状态
/// 输出: 行序列（无内容时为空）
fn protected_tail(name: &str, tool: &ToolExecution) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    if matches!(name, "bash" | "powershell") {
        out.extend(bash_exit_line(tool));
    }
    for w in truncation_warnings(tool, name) {
        out.push(Line::from(Span::styled(
            w,
            Style::default().fg(Color::Yellow),
        )));
    }
    out
}

/// 折叠策略（design D6，对照 pi 各工具 renderResult 的 maxLines）：
/// (保留行数, 是否保留尾部)。bash 尾部 5 行（exit code/最终结果在末尾），
/// grep 15、find/ls 20、其余头部 10 行。
fn collapse_policy(name: &str) -> (usize, bool) {
    match name {
        "bash" | "powershell" => (5, true),
        "grep" => (15, false),
        "find" | "ls" => (20, false),
        _ => (10, false),
    }
}

/// 按策略折叠结果体；实际发生截断时追加隐藏行数提示行
/// （spec：`... (N earlier/more lines, ctrl+o to expand)`）。
///
/// 输入: name — 工具名（选策略）；body — 全量结果体行
/// 输出: 折叠后的行序列（未超限时原样返回）
fn collapse(name: &str, body: Vec<Line<'static>>) -> Vec<Line<'static>> {
    let (keep, keep_tail) = collapse_policy(name);
    if body.len() <= keep {
        return body;
    }
    let hidden = body.len() - keep;
    let shown: Vec<Line<'static>> = if keep_tail {
        body[body.len() - keep..].to_vec()
    } else {
        body[..keep].to_vec()
    };
    let hint = if keep_tail {
        format!("... ({hidden} earlier lines, ctrl+o to expand)")
    } else if name == "write" {
        // pi write：额外提示总行数
        format!(
            "... ({hidden} more lines, {} total, ctrl+o to expand)",
            body.len()
        )
    } else {
        format!("... ({hidden} more lines, ctrl+o to expand)")
    };
    let mut out = shown;
    out.push(Line::from(Span::styled(
        hint,
        Style::default().fg(Color::DarkGray),
    )));
    out
}

use serde_json::Value;

/// 头行：`● name · 关键参数 · 状态 · 耗时`。
///
/// 输入: name — 工具名；args — 已解析参数；tool — 执行状态
/// 输出: 单行头行（状态决定颜色：成功绿/失败红/进行中黄）
fn head_line(name: &str, args: &Value, tool: &ToolExecution, expanded: bool) -> Line<'static> {
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

    if let Some(key) = head_arg(name, args, expanded) {
        spans.push(Span::raw(format!(" {key}")));
    }
    spans.push(Span::styled(
        format!(" · {status_text}"),
        Style::default().fg(status_color),
    ));
    if let Some(timer) = timer_text(tool) {
        spans.push(Span::styled(
            format!(" · {timer}"),
            Style::default().fg(Color::DarkGray),
        ));
    }
    Line::from(spans)
}

/// 卡片计时行（design D2，对照 pi `toolTimerText`）：终态且有服务端时长 →
/// `Took …`（服务端值单调且跨重载存活）；否则有起点 → `Elapsed …`（每帧
/// 重算，运行中走字）；两者皆无 → None 不显示。
///
/// 输入: tool — 工具执行状态（读 status/duration_ms/started_at）
/// 输出: Some(计时文本) / None
fn timer_text(tool: &ToolExecution) -> Option<String> {
    match tool.status {
        ToolStatus::Ended => tool
            .duration_ms
            .map(|ms| format!("Took {}", format_tool_duration(ms)))
            .or_else(|| {
                tool.started_at
                    .map(|s| format!("Took {}", format_tool_duration(s.elapsed().as_millis() as u64)))
            }),
        _ => tool
            .started_at
            .map(|s| format!("Elapsed {}", format_tool_duration(s.elapsed().as_millis() as u64))),
    }
}

/// 时长格式化（对照 pi `formatToolDuration`）：`4.2s`（<1 分钟）/
/// `1m 30s`（<1 小时）/ `2h 3m 4s`（≥1 小时）；替代裸毫秒。
///
/// 输入: ms — 毫秒数
/// 输出: 格式化时长字符串
fn format_tool_duration(ms: u64) -> String {
    let seconds = ms as f64 / 1000.0;
    if seconds < 60.0 {
        return format!("{seconds:.1}s");
    }
    let total_seconds = ms / 1000;
    let minutes = total_seconds / 60;
    let remainder = total_seconds % 60;
    if minutes < 60 {
        return format!("{minutes}m {remainder}s");
    }
    format!("{}h {}m {}s", minutes / 60, minutes % 60, remainder)
}

/// 按工具类型提取头行的关键参数（对照 pi renderCall，design D7）。
///
/// 输入: name — 工具名；args — 已解析参数；expanded — 展开态（compact 分类
///       仅折叠态生效）
/// 输出: 关键参数摘要（截断 60 字符，未知工具 100 字符）；无则 None
fn head_arg(name: &str, args: &Value, expanded: bool) -> Option<String> {
    match name {
        "grep" => {
            let pattern = args.get("pattern").and_then(|p| p.as_str())?;
            let path = args.get("path").and_then(|p| p.as_str());
            let mut head = match path {
                Some(p) if !p.is_empty() => format!("/{pattern}/ in {p}"),
                _ => format!("/{pattern}/"),
            };
            if let Some(glob) = args.get("glob").and_then(|g| g.as_str())
                && !glob.is_empty()
            {
                head.push_str(&format!(" ({glob})"));
            }
            if let Some(limit) = args.get("limit").and_then(|l| l.as_u64()) {
                head.push_str(&format!(" limit {limit}"));
            }
            Some(truncate(&head, 60))
        }
        "find" => {
            let pattern = args.get("pattern").and_then(|p| p.as_str())?;
            let path = args.get("path").and_then(|p| p.as_str());
            let mut head = match path {
                Some(p) if !p.is_empty() => format!("{pattern} in {p}"),
                _ => pattern.to_string(),
            };
            if let Some(limit) = args.get("limit").and_then(|l| l.as_u64()) {
                head.push_str(&format!(" (limit {limit})"));
            }
            Some(truncate(&head, 60))
        }
        "bash" | "powershell" => {
            // pi：prompt 样式 `$ command` + `(timeout Ns)` 后缀
            let command = args.get("command").and_then(|c| c.as_str())?;
            let prompt = if name == "powershell" { "PS>" } else { "$" };
            let mut head = format!("{prompt} {command}");
            if let Some(timeout) = args.get("timeout").and_then(|t| t.as_u64()) {
                head.push_str(&format!(" (timeout {timeout}s)"));
            }
            Some(truncate(&head, 60))
        }
        "read" => {
            // 兼容 pi 的几种字段命名。
            let raw = args
                .get("path")
                .or_else(|| args.get("filePath"))
                .or_else(|| args.get("file_path"))?;
            let raw = raw.as_str()?;
            let range = read_line_range(args);
            // 折叠态 compact 分类（design D7：raw path 字符串判断，不解析 cwd）
            if !expanded
                && let Some((kind, label)) = compact_read_class(raw)
            {
                return Some(format!(
                    "{kind} {label}{range} (ctrl+o to expand)"
                ));
            }
            Some(truncate(&format!("{raw}{range}"), 60))
        }
        "ls" => {
            let raw = args
                .get("path")
                .or_else(|| args.get("file_path"))?
                .as_str()?;
            let mut head = if raw.is_empty() { ".".to_string() } else { raw.to_string() };
            if let Some(limit) = args.get("limit").and_then(|l| l.as_u64()) {
                head.push_str(&format!(" (limit {limit})"));
            }
            Some(truncate(&head, 60))
        }
        "edit" | "write" => {
            // 兼容 pi 的几种字段命名。
            let raw = args
                .get("path")
                .or_else(|| args.get("filePath"))
                .or_else(|| args.get("file_path"))?
                .as_str()?;
            Some(truncate(raw, 60))
        }
        _ => {
            // 未知工具：pi formatToolCallWithArgs 折叠态 `key="value"` 摘要
            unknown_args_preview(args)
        }
    }
}

/// read 行范围（对照 pi `formatReadLineRange`）：`:1-20`、`:5` 形式；
/// offset/limit 均缺省时为空。
fn read_line_range(args: &Value) -> String {
    let offset = args
        .get("offset")
        .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()));
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()));
    match (offset, limit) {
        (None, None) => String::new(),
        (Some(start), None) => format!(":{start}"),
        (None, Some(count)) => format!(":1-{count}"),
        (Some(start), Some(count)) => format!(":{start}-{}", start + count.saturating_sub(1)),
    }
}

/// read 折叠态 compact 分类（对照 pi `getCompactReadClassification`，
/// design D7：按 raw path 字符串判断，不解析 cwd）。
///
/// 输入: raw — 原始路径参数
/// 输出: Some((分类词, 展示 label))；未分类返回 None
fn compact_read_class(raw: &str) -> Option<(&'static str, String)> {
    let file = raw.rsplit('/').next().unwrap_or(raw);
    if file == "SKILL.md" {
        // label = 目录名（pi：basename(dirname)）
        let dir = raw.rsplit_once('/').map(|(d, _)| d.rsplit('/').next().unwrap_or(d));
        return Some(("[skill]", dir.unwrap_or(file).to_string()));
    }
    if matches!(
        file,
        "AGENTS.override.md" | "AGENTS.md" | "AGENTS.MD" | "CLAUDE.md" | "CLAUDE.MD"
    ) {
        return Some(("resource", raw.to_string()));
    }
    if raw == "README.md" || raw.starts_with("docs/") || raw.starts_with("examples/") {
        return Some(("docs", raw.to_string()));
    }
    None
}

/// 未知工具的参数摘要（对照 pi `formatToolCallWithArgs` 折叠态）：
/// `key="value" …` 空格连接，100 字符截断；非对象参数回退 `args=<json>`。
fn unknown_args_preview(args: &Value) -> Option<String> {
    let pairs: Vec<String> = match args {
        Value::Object(map) if !map.is_empty() => map
            .iter()
            .map(|(k, v)| {
                let val = serde_json::to_string(v).unwrap_or_default();
                format!("{k}={val}")
            })
            .collect(),
        Value::Object(_) => return None,
        other => vec![format!("args={}", serde_json::to_string(other).unwrap_or_default())],
    };
    Some(truncate(&pairs.join(" "), 100))
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

/// 有效结果（design D1）：终态 `result` 优先，运行中回落 `partial`
/// （`tool_execution_update` 的部分结果）——渲染层据此显示运行中输出。
///
/// 输入: tool — 执行状态
/// 输出: Some(结果 JSON)；两者皆无返回 None
fn effective_result(tool: &ToolExecution) -> Option<&serde_json::Value> {
    tool.result.as_ref().or(tool.partial.as_ref())
}

/// 读取结果中的文本正文（content[].text 拼接）。
///
/// 输入: tool — 执行结果（result 可能为 None，运行中回落 partial）
/// 输出: Some(正文)；无内容返回 None
fn result_text(tool: &ToolExecution) -> Option<String> {
    let result = effective_result(tool)?;
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

/// bash：命令输出（structuredContent.output 优先）。exit 状态行在**末尾**
///（pi 把 `Command exited with code N` 追加到输出尾部）——bash 折叠取尾
/// （design D6）才不会切掉退出码。
fn bash_output(_args: &Value, tool: &ToolExecution) -> Vec<Line<'static>> {
    let result = effective_result(tool);
    let structured = result.and_then(|r| r.get("structuredContent"));

    let output = structured
        .and_then(|s| s.get("output"))
        .and_then(|o| o.as_str())
        .map(String::from)
        .or_else(|| result_text(tool));

    output
        .map(|o| text_lines(o.trim_end()))
        .unwrap_or_default()
}

/// bash 的 exit 状态行（绿 0 / 红非 0）——pi 将其追加在输出尾部，
/// 我们作为受保护尾部渲染，折叠不切。
fn bash_exit_line(tool: &ToolExecution) -> Vec<Line<'static>> {
    let exit = effective_result(tool)
        .and_then(|r| r.get("structuredContent"))
        .and_then(|s| s.get("exit_code"))
        .and_then(|x| x.as_i64());
    match exit {
        Some(exit) => {
            let color = if exit == 0 { Color::Green } else { Color::Red };
            vec![Line::from(Span::styled(
                format!("exit {exit}"),
                Style::default().fg(color),
            ))]
        }
        None => Vec::new(),
    }
}

/// 截断警告（对照 pi 各工具 renderResult 的 warnings 文案，带具体
/// 数字/路径而非仅省略号）。渲染为受保护尾部，折叠不切。
///
/// 输入: tool — 执行状态（读 details/structuredContent）；name — 工具名
/// 输出: 警告文本行（无截断时为空）
fn truncation_warnings(tool: &ToolExecution, name: &str) -> Vec<String> {
    let result = effective_result(tool);
    let details = result.and_then(|r| r.get("details"));
    let structured = result.and_then(|r| r.get("structuredContent"));
    let mut out = Vec::new();

    match name {
        "bash" | "powershell" => {
            // pi：details.truncation/fullOutputPath 优先，structuredContent 回退
            let path = details
                .and_then(|d| d.get("fullOutputPath"))
                .and_then(|v| v.as_str())
                .or_else(|| {
                    structured
                        .and_then(|s| s.get("full_output_path"))
                        .and_then(|v| v.as_str())
                });
            let truncation = details.and_then(|d| d.get("truncation"));
            let truncated = truncation
                .and_then(|t| t.get("truncated"))
                .and_then(|x| x.as_bool())
                .unwrap_or(false)
                || structured
                    .and_then(|s| s.get("truncated"))
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false);
            if truncated || path.is_some() {
                let mut parts = Vec::new();
                if let Some(p) = path {
                    parts.push(format!("Full output: {p}"));
                }
                if truncated {
                    if truncation
                        .and_then(|t| t.get("truncatedBy"))
                        .and_then(|x| x.as_str())
                        == Some("lines")
                    {
                        parts.push(format!(
                            "Truncated: showing {} of {} lines",
                            json_u64(truncation, "outputLines"),
                            json_u64(truncation, "totalLines")
                        ));
                    } else {
                        parts.push(format!(
                            "Truncated: {} lines shown ({} limit)",
                            json_u64(truncation, "outputLines"),
                            format_size(json_u64(truncation, "maxBytes").max(1))
                        ));
                    }
                }
                out.push(format!("[{}]", parts.join(". ")));
            }
        }
        "grep" | "find" | "ls" => {
            let limit_key = match name {
                "grep" => "matchLimitReached",
                "find" => "resultLimitReached",
                _ => "entryLimitReached",
            };
            let limit_unit = match name {
                "grep" => "matches",
                "find" => "results",
                _ => "entries",
            };
            let mut parts = Vec::new();
            if let Some(n) = details.and_then(|d| d.get(limit_key)).and_then(|x| x.as_u64()) {
                parts.push(format!("{n} {limit_unit} limit"));
            }
            if details
                .and_then(|d| d.get("truncation"))
                .and_then(|t| t.get("truncated"))
                .and_then(|x| x.as_bool())
                .unwrap_or(false)
            {
                let max = json_u64(details.and_then(|d| d.get("truncation")), "maxBytes");
                parts.push(format!("{} limit", format_size(max.max(1))));
            }
            // pi：linesTruncated 为 bool；兼容数字形状
            match details.and_then(|d| d.get("linesTruncated")) {
                Some(Value::Bool(true)) => parts.push("some lines truncated".into()),
                Some(v) if v.as_u64().unwrap_or(0) > 0 => {
                    parts.push(format!("{} lines truncated", v.as_u64().unwrap()))
                }
                _ => {}
            }
            if !parts.is_empty() {
                out.push(format!("[Truncated: {}]", parts.join(", ")));
            }
        }
        "read" => {
            let truncation = details.and_then(|d| d.get("truncation"));
            if truncation
                .and_then(|t| t.get("truncated"))
                .and_then(|x| x.as_bool())
                .unwrap_or(false)
            {
                let max_bytes = json_u64(truncation, "maxBytes");
                if truncation
                    .and_then(|t| t.get("firstLineExceedsLimit"))
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false)
                {
                    out.push(format!(
                        "[First line exceeds {} limit]",
                        format_size(max_bytes.max(1))
                    ));
                } else if truncation
                    .and_then(|t| t.get("truncatedBy"))
                    .and_then(|x| x.as_str())
                    == Some("lines")
                {
                    out.push(format!(
                        "[Truncated: showing {} of {} lines ({} line limit)]",
                        json_u64(truncation, "outputLines"),
                        json_u64(truncation, "totalLines"),
                        json_u64(truncation, "maxLines")
                    ));
                } else {
                    out.push(format!(
                        "[Truncated: {} lines shown ({} limit)]",
                        json_u64(truncation, "outputLines"),
                        format_size(max_bytes.max(1))
                    ));
                }
            }
        }
        _ => {}
    }
    out
}

/// 读 details 子对象里的 u64 字段，缺失/类型不符回退 0。
fn json_u64(v: Option<&Value>, key: &str) -> u64 {
    v.and_then(|o| o.get(key))
        .and_then(|x| x.as_u64())
        .unwrap_or(0)
}

/// 大小格式化（对照 pi formatSize）：`512B` / `50.0KB` / `1.0MB`。
fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
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
fn text_result_body(tool: &ToolExecution) -> Vec<Line<'static>> {
    // 截断提示已移入 truncation_warnings（受保护尾部，不参与折叠）。
    result_text(tool)
        .map(|t| text_lines(t.trim_end()))
        .unwrap_or_default()
}

/// 通用卡片结果体：展开态逐参数 `key: value` 分行（对照 pi
/// `formatToolCallWithArgs` 展开态），折叠态参数摘要已上移头行；
/// 结果正文摘要。
fn generic_body(args: &Value, tool: &ToolExecution, expanded: bool) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    if expanded {
        let entries: Vec<(String, String)> = match args {
            Value::Object(map) => map
                .iter()
                .map(|(k, v)| {
                    let text = if let Some(s) = v.as_str() {
                        s.to_string()
                    } else {
                        serde_json::to_string(v).unwrap_or_default()
                    };
                    (k.clone(), text)
                })
                .collect(),
            Value::Null => vec![],
            other => vec![(
                "args".to_string(),
                serde_json::to_string(other).unwrap_or_default(),
            )],
        };
        for (k, v) in entries {
            out.push(Line::from(Span::styled(
                format!("  {k}: {v}"),
                Style::default().fg(Color::DarkGray),
            )));
        }
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

    /// format_tool_duration 三档边界（spec：4.2s / 1m 30s / 2h 3m 4s）。
    #[test]
    fn format_tool_duration_three_tiers() {
        assert_eq!(format_tool_duration(4200), "4.2s");
        assert_eq!(format_tool_duration(999), "1.0s");
        assert_eq!(format_tool_duration(59_999), "60.0s", "<60s 仍用秒档");
        assert_eq!(format_tool_duration(90_000), "1m 30s");
        assert_eq!(format_tool_duration(3_599_000), "59m 59s");
        assert_eq!(format_tool_duration(3_600_000), "1h 0m 0s");
        assert_eq!(format_tool_duration(7_384_000), "2h 3m 4s");
    }

    /// 计时行三态：Ended+duration → Took（服务端值）；Running+起点 →
    /// Elapsed（每帧重算）；皆无 → 不显示。
    #[test]
    fn timer_text_three_states() {
        // Ended 且有 duration_ms → Took 服务端值（不受 started_at 影响）
        let ended = ToolExecution {
            status: ToolStatus::Ended,
            duration_ms: Some(90_000),
            started_at: Some(std::time::Instant::now()),
            ..Default::default()
        };
        assert_eq!(timer_text(&ended).unwrap(), "Took 1m 30s");

        // Ended 无 duration_ms 但有起点 → Took 兼容时长
        let ended2 = ToolExecution {
            status: ToolStatus::Ended,
            ..Default::default()
        };
        assert!(timer_text(&ended2).is_none(), "皆无不显示");

        // Running + 起点 → Elapsed（运行中走字）
        let running = ToolExecution {
            status: ToolStatus::Running,
            started_at: Some(std::time::Instant::now() - std::time::Duration::from_secs(5)),
            ..Default::default()
        };
        let t = timer_text(&running).unwrap();
        assert!(t.starts_with("Elapsed 5."), "运行中 Elapsed: {t}");

        // Running 无起点（如 get_messages 重建）→ 不显示
        let rebuilt = ToolExecution {
            status: ToolStatus::Running,
            ..Default::default()
        };
        assert!(timer_text(&rebuilt).is_none(), "无起点降级为不显示");
    }

    #[test]
    fn unknown_tool_renders_generic_card() {
        let tool = ToolExecution {
            status: ToolStatus::Ended,
            is_error: false,
            result: Some(serde_json::json!({"content":[{"type":"text","text":"ok result"}]})),
            duration_ms: Some(5),
            ..Default::default()
        };
        let lines = tool_lines("mystery_tool", r#"{"foo":1}"#, &tool, true);
        let text = render(&lines);
        assert!(text.contains("mystery_tool"), "{text}");
        assert!(text.contains("done"), "{text}");
        assert!(text.contains("Took 0.0s"), "计时格式: {text}");
        // 头行 `name key="value"` 摘要 + 展开态逐参数分行（spec：未知工具头行参数摘要）
        assert!(text.contains("foo=1"), "头行参数摘要: {text}");
        assert!(text.contains("  foo: 1"), "展开态分行: {text}");
        assert!(text.contains("ok result"), "{text}");
    }

    #[test]
    fn failed_tool_head_is_red() {
        let tool = ToolExecution {
            status: ToolStatus::Ended,
            is_error: true,
            result: None,
            duration_ms: None,
            ..Default::default()
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
            ..Default::default()
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
        // read：头部 10 行折叠（超出则截断 + 提示行）。
        let (name, args, tool) = fixture_tool_call("tool_call", "read");
        let collapsed = tool_lines(&name, &args, &tool, false);
        let expanded = tool_lines(&name, &args, &tool, true);
        assert!(collapsed.len() <= 1 + 10 + 1, "read 折叠含提示行: {}", collapsed.len());
        assert!(expanded.len() > collapsed.len(), "展开应更多行");
        if expanded.len() > 11 {
            assert!(
                render(&collapsed).contains("more lines, ctrl+o to expand"),
                "截断时应有提示行: {}",
                render(&collapsed)
            );
        }
        // bash 结果只有 2 行：折叠态不截断（无提示行）
        let (bn, ba, bt) = fixture_tool_call("tool_bash", "bash");
        let bc = tool_lines(&bn, &ba, &bt, false);
        assert!(render(&bc).contains("hello-bash-fixture"));
        assert!(
            !render(&bc).contains("earlier lines"),
            "未超限不应有提示行"
        );
    }

    /// bash 折叠保留**尾部** 5 行（含 exit code 行）+ earlier-lines 提示
    /// （design D6：exit code/最终结果在末尾，取头会切掉）。
    #[test]
    fn bash_collapsed_keeps_tail_with_exit_and_hint() {
        let output: String = (1..=20).map(|i| format!("line{i}\n")).collect();
        let tool = ToolExecution {
            status: ToolStatus::Ended,
            result: Some(serde_json::json!({
                "content": [{"type":"text","text": output}],
                "structuredContent": {"output": output, "exit_code": 0, "truncated": false}
            })),
            ..Default::default()
        };
        let collapsed = tool_lines("bash", r#"{"command":"x"}"#, &tool, false);
        let text = render(&collapsed);
        // body = 20 输出行（exit/警告在受保护尾部不计折叠）；尾 5 行 = line16..line20
        assert!(text.contains("line20"), "尾行保留: {text}");
        assert!(text.contains("line16"), "尾 5 行: {text}");
        assert!(!text.contains("line15"), "更早行截断: {text}");
        assert!(text.contains("exit 0"), "exit 行作为受保护尾部保留: {text}");
        assert!(text.contains("(15 earlier lines, ctrl+o to expand)"), "提示行: {text}");

        // 展开态：全量 + 无提示
        let expanded = tool_lines("bash", r#"{"command":"x"}"#, &tool, true);
        let etext = render(&expanded);
        assert!(etext.contains("line1"), "展开含首行: {etext}");
        assert!(!etext.contains("earlier lines"), "展开无提示: {etext}");
    }

    #[test]
    fn partial_json_args_do_not_panic() {
        let tool = ToolExecution {
            status: ToolStatus::Running,
            is_error: false,
            result: None,
            duration_ms: None,
            ..Default::default()
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
            ..Default::default()
        };
        let lines = tool_lines("write", r#"{"path":"/x"}"#, &tool_no_content, true);
        assert!(render(&lines).contains("Successfully wrote"));
    }

    /// 截断警告（spec：带具体数字/路径而非仅省略号）：bash 完整输出路径
    /// + 行数详情、grep/find/ls 各自 limit、read 三形态。
    #[test]
    fn truncation_warnings_carry_numbers_and_paths() {
        // bash：details.truncation + fullOutputPath（pi 主路径）
        let bash = ToolExecution {
            status: ToolStatus::Ended,
            result: Some(serde_json::json!({
                "content": [{"type":"text","text":"out"}],
                "details": {
                    "fullOutputPath": "/tmp/pi-bash-123.log",
                    "truncation": {"truncated": true, "truncatedBy": "lines",
                        "outputLines": 500, "totalLines": 12000, "maxBytes": 51200}
                }
            })),
            ..Default::default()
        };
        let text = render(&tool_lines("bash", r#"{"command":"x"}"#, &bash, false));
        assert!(text.contains("[Full output: /tmp/pi-bash-123.log. Truncated: showing 500 of 12000 lines]"), "bash 警告: {text}");

        // bash 字节截断（structuredContent 回退路径）
        let bash2 = ToolExecution {
            status: ToolStatus::Ended,
            result: Some(serde_json::json!({
                "content": [{"type":"text","text":"out"}],
                "structuredContent": {"truncated": true, "full_output_path": "/tmp/x.log"},
                "details": {"truncation": {"truncated": true, "truncatedBy": "bytes",
                    "outputLines": 42, "maxBytes": 51200}}
            })),
            ..Default::default()
        };
        let text = render(&tool_lines("bash", "{}", &bash2, false));
        assert!(text.contains("Truncated: 42 lines shown (50.0KB limit)"), "字节截断: {text}");

        // grep：matchLimit + linesTruncated（pi 为 bool）
        let grep = ToolExecution {
            status: ToolStatus::Ended,
            result: Some(serde_json::json!({
                "content": [{"type":"text","text":"a:1"}],
                "details": {"matchLimitReached": 500, "linesTruncated": true}
            })),
            ..Default::default()
        };
        let text = render(&tool_lines("grep", "{}", &grep, false));
        assert!(text.contains("[Truncated: 500 matches limit, some lines truncated]"), "grep: {text}");

        // find / ls 的 limit 单位不同
        let find = ToolExecution {
            status: ToolStatus::Ended,
            result: Some(serde_json::json!({
                "content": [{"type":"text","text":"./a"}],
                "details": {"resultLimitReached": 100}
            })),
            ..Default::default()
        };
        let text = render(&tool_lines("find", "{}", &find, false));
        assert!(text.contains("[Truncated: 100 results limit]"), "find: {text}");
        let ls = ToolExecution {
            status: ToolStatus::Ended,
            result: Some(serde_json::json!({
                "content": [{"type":"text","text":"a"}],
                "details": {"entryLimitReached": 50}
            })),
            ..Default::default()
        };
        let text = render(&tool_lines("ls", "{}", &ls, false));
        assert!(text.contains("[Truncated: 50 entries limit]"), "ls: {text}");

        // read 三形态：行数截断（含 maxLines）
        let read = ToolExecution {
            status: ToolStatus::Ended,
            result: Some(serde_json::json!({
                "content": [{"type":"text","text":"l1"}],
                "details": {"truncation": {"truncated": true, "truncatedBy": "lines",
                    "outputLines": 200, "totalLines": 9999, "maxLines": 200}}
            })),
            ..Default::default()
        };
        let text = render(&tool_lines("read", "{}", &read, false));
        assert!(
            text.contains("[Truncated: showing 200 of 9999 lines (200 line limit)]"),
            "read 行数截断: {text}"
        );
        // read 首行超限
        let read2 = ToolExecution {
            status: ToolStatus::Ended,
            result: Some(serde_json::json!({
                "content": [{"type":"text","text":"x"}],
                "details": {"truncation": {"truncated": true, "firstLineExceedsLimit": true,
                    "maxBytes": 51200}}
            })),
            ..Default::default()
        };
        let text = render(&tool_lines("read", "{}", &read2, false));
        assert!(text.contains("[First line exceeds 50.0KB limit]"), "read 首行超限: {text}");

        // 无截断 → 无警告行
        let clean = ToolExecution {
            status: ToolStatus::Ended,
            result: Some(serde_json::json!({"content": [{"type":"text","text":"ok"}]})),
            ..Default::default()
        };
        let text = render(&tool_lines("grep", "{}", &clean, false));
        assert!(!text.contains("Truncated"), "无截断不警告: {text}");
    }

    /// 头行扩展（spec：头行参数与分类）：bash prompt+timeout、read 行范围
    /// 与 compact 分类、grep glob/limit、find/ls limit。
    #[test]
    fn head_args_align_with_pi() {
        let ended = |result: serde_json::Value| ToolExecution {
            status: ToolStatus::Ended,
            result: Some(result),
            ..Default::default()
        };

        // bash：`$ <cmd>` + `(timeout Ns)` 后缀
        let bash = ended(serde_json::json!({"content":[] , "structuredContent": {"output":""}}));
        let text = render(&tool_lines(
            "bash",
            r#"{"command":"ls -la","timeout":5}"#,
            &bash,
            false,
        ));
        assert!(text.contains("$ ls -la (timeout 5s)"), "bash 头行: {text}");

        // read：行范围 `:5-24`（offset=5 limit=20 → 5..24）
        let read = ended(serde_json::json!({"content":[] }));
        let text = render(&tool_lines(
            "read",
            r#"{"path":"src/main.rs","offset":5,"limit":20}"#,
            &read,
            false,
        ));
        assert!(text.contains("src/main.rs:5-24"), "read 行范围: {text}");

        // read 折叠态 compact 分类：SKILL.md → [skill] 目录名 + expand 提示
        let text = render(&tool_lines(
            "read",
            r#"{"path":"skills/foo/SKILL.md"}"#,
            &read,
            false,
        ));
        assert!(text.contains("[skill] foo"), "skill 分类: {text}");
        assert!(text.contains("ctrl+o to expand"), "expand 提示: {text}");
        // 展开态不分类（显示原始路径）
        let text = render(&tool_lines(
            "read",
            r#"{"path":"skills/foo/SKILL.md"}"#,
            &read,
            true,
        ));
        assert!(!text.contains("[skill]"), "展开态不分类: {text}");
        assert!(text.contains("skills/foo/SKILL.md"), "原始路径: {text}");

        // read 资源与 docs 分类
        let text = render(&tool_lines("read", r#"{"path":"AGENTS.md"}"#, &read, false));
        assert!(text.contains("resource AGENTS.md"), "resource 分类: {text}");
        let text = render(&tool_lines("read", r#"{"path":"docs/usage.md"}"#, &read, false));
        assert!(text.contains("docs docs/usage.md"), "docs 分类: {text}");

        // grep：glob 与 limit
        let grep = ended(serde_json::json!({"content":[] }));
        let text = render(&tool_lines(
            "grep",
            r#"{"pattern":"foo","path":"src","glob":"*.rs","limit":50}"#,
            &grep,
            false,
        ));
        assert!(text.contains("/foo/ in src (*.rs) limit 50"), "grep 头行: {text}");

        // find / ls：limit
        let find = ended(serde_json::json!({"content":[] }));
        let text = render(&tool_lines(
            "find",
            r#"{"pattern":"*.rs","path":".","limit":100}"#,
            &find,
            false,
        ));
        assert!(text.contains("*.rs in . (limit 100)"), "find 头行: {text}");
        let ls = ended(serde_json::json!({"content":[] }));
        let text = render(&tool_lines("ls", r#"{"path":"src","limit":30}"#, &ls, false));
        assert!(text.contains("src (limit 30)"), "ls 头行: {text}");

        // write 折叠：预览行数与总行数
        let long: String = (1..=30).map(|i| format!("w{i}\n")).collect();
        let write = ended(serde_json::json!({"content":[] }));
        let write_args =
            serde_json::json!({"path": "out.txt", "content": long}).to_string();
        let text = render(&tool_lines("write", &write_args, &write, false));
        assert!(text.contains("more lines,"), "write 提示: {text}");
        assert!(text.contains("total, ctrl+o to expand"), "write 总行数: {text}");
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
            ..Default::default()
        };
        let args = r#"{"pattern":"foo","path":"src"}"#;
        let lines = tool_lines("grep", args, &tool, true);
        let text = render(&lines);
        assert!(text.contains("/foo/ in src"), "头行: {text}");
        assert!(text.contains("src/a.rs:12"), "输出行: {text}");
        assert!(text.contains("7 lines truncated"), "截断提示: {text}");
        assert!(text.contains("done") && text.contains("Took 0.0s"), "状态/耗时: {text}");
    }

    #[test]
    fn find_and_ls_cards_render() {
        let find_tool = ToolExecution {
            status: ToolStatus::Ended,
            is_error: false,
            result: Some(serde_json::json!({"content":[{"type":"text","text":"./src/main.rs\n./src/lib.rs"}]})),
            duration_ms: None,
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
        // ls 策略为头部 20 行：造 25 行以体现折叠/展开差异。
        let text: String = (1..=25).map(|i| format!("l{i}\n")).collect();
        let tool = ToolExecution {
            status: ToolStatus::Ended,
            is_error: false,
            result: Some(serde_json::json!({"content":[{"type":"text","text": text}]})),
            duration_ms: None,
            ..Default::default()
        };
        let collapsed = tool_lines("ls", r#"{"path":"."}"#, &tool, false);
        let expanded = tool_lines("ls", r#"{"path":"."}"#, &tool, true);
        assert!(collapsed.len() <= 1 + 20 + 1, "{}", collapsed.len());
        assert!(expanded.len() > collapsed.len());
        assert!(!render(&collapsed).contains("l21"), "折叠只到 20 行");
        assert!(render(&collapsed).contains("more lines, ctrl+o to expand"), "提示行");
        assert!(render(&expanded).contains("l25"), "展开含尾行");
    }
}
