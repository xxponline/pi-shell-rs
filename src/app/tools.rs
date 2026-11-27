//! 工具调用卡片渲染（纯函数：块 → 带样式行）。
//!
//! 每张卡片 = 头行（工具名 · 关键参数 · 状态 · 耗时）+ 展开体（结果预览）。
//! 结果形状防御式读取：`content[]` / `structuredContent` / `details` 任一
//! 缺失都降级，绝不 panic——pi 协议会演进，卡片不能脆。

use ratatui::style::{Color, Modifier, Style};

use super::EditPreviewState;
use ratatui::text::{Line, Span};

use pi_shell_rs::session::{ToolExecution, ToolStatus};

/// 渲染一张工具卡片。
///
/// 输入: name — 工具名；args_json — 调用参数（可能为流式不完整 JSON）；
///       tool — 执行状态/结果；expanded — 是否展开完整输出（ctrl+o）
/// 输出: 卡片行序列（首行为头行，后续为结果体）
/// 工具卡片行渲染（默认配色薄包装，design D1 方案 A）：转发
/// [`tool_lines_themed`] 传默认配色，存量调用点（含测试）签名零改动。
/// 渲染主路径（chat append_blocks）走 themed 入口。
#[allow(dead_code)]
pub fn tool_lines(
    name: &str,
    args_json: &str,
    tool: &ToolExecution,
    expanded: bool,
) -> Vec<Line<'static>> {
    tool_lines_themed(name, args_json, tool, expanded, &crate::app::theme::TuiTheme::default())
}

/// 工具卡片行渲染（带注入配色；[`tool_lines`] 的 themed 实现体）。
///
/// 输入: name — 工具名；args_json — 原始参数串；tool — 执行状态；
///       expanded — 展开态；theme — 注入配色（read/write 卡语法高亮
///       取 `syntax_theme`，其余卡不受影响）
/// 输出: 卡片行序列（头行 + 折叠/展开体 + 受保护尾部）
pub fn tool_lines_themed(
    name: &str,
    args_json: &str,
    tool: &ToolExecution,
    expanded: bool,
    theme: &crate::app::theme::TuiTheme,
) -> Vec<Line<'static>> {
    tool_lines_themed_ctx(name, args_json, tool, expanded, theme, None)
}

/// [`tool_lines_themed`] 预览版（task3.3）：edit 卡带执行前预览态。
///
/// 输入: 同 [`tool_lines_themed`]；preview — 该 toolId 的预览态；
/// 输出: 同 [`tool_lines_themed`]
pub fn tool_lines_themed_ctx(
    name: &str,
    args_json: &str,
    tool: &ToolExecution,
    expanded: bool,
    theme: &crate::app::theme::TuiTheme,
    preview: Option<&EditPreviewState>,
) -> Vec<Line<'static>> {
    // 分段计量（render_probe）：工具卡渲染归因段。
    let probe_t0 = std::time::Instant::now();
    let out = tool_lines_themed_inner(name, args_json, tool, expanded, theme, preview);
    crate::app::render_probe::record_toolcard(probe_t0.elapsed().as_nanos() as u64);
    out
}

/// [`tool_lines_themed`] 的实现体（外层只做分段计时归因）：
/// 消费中性模型 [`tool_card_model`] 再转回 ratatui 行——TUI 侧薄适配，
/// 不再持有任何卡片语义（design D1）。
///
/// 输入/输出: 同 [`tool_lines_themed`]
fn tool_lines_themed_inner(
    name: &str,
    args_json: &str,
    tool: &ToolExecution,
    expanded: bool,
    theme: &crate::app::theme::TuiTheme,
    preview: Option<&EditPreviewState>,
) -> Vec<Line<'static>> {
    let model = tool_card_model_ctx(name, args_json, tool, expanded, theme, preview);
    let mut out = Vec::with_capacity(model.body.len() + model.tail.len() + 2);
    out.push(segs_to_line(&model.head));
    for row in &model.body {
        out.push(segs_to_line(row));
    }
    if let Some(hint) = &model.hint {
        out.push(segs_to_line(std::slice::from_ref(hint)));
    }
    for row in &model.tail {
        out.push(segs_to_line(row));
    }
    out
}

/// 中性段序列 → ratatui 行（TUI 侧适配；role → 既有样式映射）。
///
/// 输入: segs — 单行的语义段（头行/正文行/提示/尾部行）
/// 输出: 等文本的 ratatui 行（样式与历史 `head_line`/`collapse`/高亮
///       路径逐项一致：状态黄绿红、名字粗体、计时/提示暗灰、Fg 透传）
pub(crate) fn segs_to_line(segs: &[Seg]) -> Line<'static> {
    Line::from(
        segs.iter()
            .map(|s| Span::styled(s.text.clone(), seg_style(s.role)))
            .collect::<Vec<_>>(),
    )
}

/// 段角色 → ratatui 样式（与历史各构建器产出逐项对齐）。
///
/// 输入: role — 段角色（`Fg` 携带显式前景色）
/// 输出: 该段的 ratatui 样式
fn seg_style(role: SegRole) -> Style {
    match role {
        SegRole::StatusRun | SegRole::NoticeRetry | SegRole::Warn => {
            Style::default().fg(Color::Yellow)
        }
        SegRole::StatusOk | SegRole::ExitOk => Style::default().fg(Color::Green),
        SegRole::StatusErr => Style::default().fg(Color::Red),
        SegRole::NoticeError | SegRole::ExitBad => {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        }
        SegRole::Name => Style::default().add_modifier(Modifier::BOLD),
        // 命令行黄粗：HEAD 与 spec「命令行黄色加粗」同款（task1.2 段化
        // 重构曾把黄色丢成纯粗体——chat 侧回归测试看住）。
        SegRole::Cmd => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        SegRole::Arg | SegRole::Body => Style::default(),
        SegRole::Timer | SegRole::Hint | SegRole::Dim => Style::default().fg(Color::DarkGray),
        SegRole::Notice | SegRole::ThinkingText => Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
        SegRole::Fg(c) => Style::default().fg(c),
        // 反视频变更词：终端 SGR7 真反视频（与 pi theme.inverse 同语义）。
        SegRole::InverseFg(c) => Style::default().fg(c).add_modifier(Modifier::REVERSED),
    }
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

use serde_json::Value;

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
pub(crate) fn result_text(tool: &ToolExecution) -> Option<String> {
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
/// 否则 result_text）。按扩展名语法高亮（add-tool-card-syntax-highlighting
/// design D1/D2-A——pi formatReadResult 同序：先高亮全文，collapse 再切
/// 前 10 行、提示计数取全文）；None（无扩展名/未知语法）走现状纯文本
/// （降级逐字节等价）。
///
/// 输入: args — 调用参数（取 path/file_path 定语言）；tool — 执行状态
/// 输出: 结果体行序列（高亮或现状纯文本）
fn read_body(args: &Value, tool: &ToolExecution, theme: &crate::app::theme::TuiTheme) -> Vec<Line<'static>> {
    let result = tool.result.as_ref();
    // owned 兑底：structuredContent 缺失时 result_text 的 String 需活过借用。
    let fallback = result_text(tool);
    let content: Option<&str> =
        if let Some(Value::String(c)) = result.and_then(|r| r.get("structuredContent")) {
            Some(c.as_str())
        } else {
            fallback.as_deref()
        };
    let Some(content) = content else {
        return Vec::new();
    };
    let path = args
        .get("path")
        .or_else(|| args.get("file_path"))
        .and_then(|p| p.as_str())
        .unwrap_or("");
    crate::app::markdown::highlight_by_path_themed(path, content, theme)
        .unwrap_or_else(|| text_lines(content))
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
fn edit_body(
    args: &Value,
    tool: &ToolExecution,
    preview: Option<&EditPreviewState>,
) -> Vec<Line<'static>> {
    let details = tool
        .result
        .as_ref()
        .and_then(|r| r.get("details"));

    // 优先级（spec: 结果到达替换）：结果 diff > 预览 Ready/Error > args 降级。
    if let Some(diff) = details.and_then(|d| d.get("diff")).and_then(|d| d.as_str()) {
        // 解析为共享行模型 → 渲染器统一做 1-1 行内分段（task2.1）。
        let rows: Vec<DiffRow> = diff
            .lines()
            .map(|l| match parse_diff_line(l) {
                Some(('-', lead, content)) => DiffRow::Minus {
                    lead: lead.to_string(),
                    content: content.to_string(),
                },
                Some((_, lead, content)) => DiffRow::Plus {
                    lead: lead.to_string(),
                    content: content.to_string(),
                },
                None if l.starts_with("@@") => DiffRow::Hunk(l.to_string()),
                None => DiffRow::Context(l.to_string()),
            })
            .collect();
        return render_diff_rows(&rows);
    }
    match preview {
        Some(EditPreviewState::Ready(rows)) => return render_diff_rows(rows),
        Some(EditPreviewState::Error(msg)) => {
            return vec![Line::from(Span::styled(
                msg.clone(),
                Style::default().fg(Color::Red),
            ))];
        }
        Some(EditPreviewState::Pending) | None => {}
    }

    // 降级：args.oldString/newString 行级对比。
    let old = args.get("oldString").and_then(|v| v.as_str()).unwrap_or("");
    let new = args.get("newString").and_then(|v| v.as_str()).unwrap_or("");
    line_diff(old, new)
}

/// 行内分段单元（词级 diff 内部：文本 + 是否变更段；design D1）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Seg0 {
    text: String,
    changed: bool,
}

/// 词级分词：连续字母数字 = 词、连续空白 = 一段、标点各为一段
/// （对齐 pi `diffWords` 的可读粒度；design D1——不追求逐字符同）。
///
/// 输入: s — 待分词文本；
/// 输出: token 序列（非空）
fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_alphanumeric() {
            let mut w = String::from(c);
            while let Some(&n) = chars.peek()
                && n.is_alphanumeric()
            {
                w.push(n);
                chars.next();
            }
            out.push(w);
        } else if c.is_whitespace() {
            let mut w = String::from(c);
            while let Some(&n) = chars.peek()
                && n.is_whitespace()
            {
                w.push(n);
                chars.next();
            }
            out.push(w);
        } else {
            out.push(c.to_string());
        }
    }
    out
}

/// 词级 LCS 对比 → 两侧分段（未变更 token 归 unchanged，变更归 changed）。
///
/// 输入: a/b — 已分词的旧/新 token 序列；
/// 输出: (旧侧分段, 新侧分段)，按出现顺序合并同态相邻段
fn lcs_words(a: &[String], b: &[String]) -> (Vec<Seg0>, Vec<Seg0>) {
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
    let mut old_out: Vec<Seg0> = Vec::new();
    let mut new_out: Vec<Seg0> = Vec::new();
    let push = |out: &mut Vec<Seg0>, text: &str, changed: bool| {
        match out.last_mut() {
            Some(last) if last.changed == changed => last.text.push_str(text),
            _ => out.push(Seg0 {
                text: text.to_string(),
                changed,
            }),
        }
    };
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if a[i] == b[j] {
            push(&mut old_out, &a[i], false);
            push(&mut new_out, &b[j], false);
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            push(&mut old_out, &a[i], true);
            i += 1;
        } else {
            push(&mut new_out, &b[j], true);
            j += 1;
        }
    }
    for t in &a[i..] {
        push(&mut old_out, t, true);
    }
    for t in &b[j..] {
        push(&mut new_out, t, true);
    }
    (old_out, new_out)
}

/// 词级行内分段（pi `renderIntraLineDiff` 同源；design D1/D2）。
///
/// 功能: ① 剥离 old/new 共同前导空白并原样拼回（防首段插入吞缩进）；
///       ② 词级 LCS 对剩余部分；③ 首个变更段若是纯空白则保留不染
///       （pi 首段前导空白规则）；④ token 乘积超上限退化为不分段
///       （changed=false，等同分段前的整行渲染，防大行卡顿）；
/// 输入: old/new — 单行新旧文本；
/// 输出: (旧侧分段, 新侧分段)
fn intra_line_diff(old: &str, new: &str) -> (Vec<Seg0>, Vec<Seg0>) {
    // 共同前导空白：逐位（字符）相同且均为空白才计入。
    let mut indent_len = 0usize;
    for (oc, nc) in old.chars().zip(new.chars()) {
        if oc == nc && oc.is_whitespace() {
            indent_len += oc.len_utf8();
        } else {
            break;
        }
    }
    let indent = &old[..indent_len];
    let old_rest = &old[indent_len..];
    let new_rest = &new[indent_len..];

    let a = tokenize(old_rest);
    let b = tokenize(new_rest);
    // 上限保护：退化为不分段（整行原样着色，与分段前行为一致）。
    if a.len().saturating_mul(b.len()) > 160_000 {
        return (
            vec![Seg0 {
                text: old.to_string(),
                changed: false,
            }],
            vec![Seg0 {
                text: new.to_string(),
                changed: false,
            }],
        );
    }
    let (mut old_segs, mut new_segs) = lcs_words(&a, &b);

    // pi 首段规则：首个变更段若是纯空白 → 保留不染（两侧各自判定）。
    for segs in [&mut old_segs, &mut new_segs] {
        if let Some(first) = segs.iter_mut().find(|s| s.changed)
            && first.text.chars().all(|c| c.is_whitespace())
        {
            first.changed = false;
        }
    }
    // 缩进原样拼回（两侧都以未变更段开头）。
    if !indent.is_empty() {
        for segs in [&mut old_segs, &mut new_segs] {
            match segs.first_mut() {
                Some(first) if !first.changed => first.text.insert_str(0, indent),
                _ => segs.insert(
                    0,
                    Seg0 {
                        text: indent.to_string(),
                        changed: false,
                    },
                ),
            }
        }
    }
    (old_segs, new_segs)
}

/// diff 行模型（两路径共享；design D3——渲染侧统一做 1-1 行内分段）。
/// 预览回填也以本型携带（design D5 修正：存行模型而非文本，避免上下文
/// 行以 `-` 开头的再解析歧义）。
#[derive(Debug, Clone, PartialEq)]
pub enum DiffRow {
    /// `-` 行：lead 含前缀/行号/分隔空格，content 为正文。
    Minus { lead: String, content: String },
    /// `+` 行（同上）。
    Plus { lead: String, content: String },
    /// `@@` hunk 行（整行青色）。
    Hunk(String),
    /// 上下文/未变行（原样）。
    Context(String),
}

/// 解析 pi diff 行为 (前缀, lead, content)；lead 含前缀 + 可选行号与
/// 分隔空格；无行号时 lead 仅前缀字符；非 -/+ 行返回 None。
///
/// 输入: l — diff 原始行；
/// 输出: Some((前缀, lead, content)) / None
fn parse_diff_line(l: &str) -> Option<(char, &str, &str)> {
    let first = l.chars().next()?;
    if first != '-' && first != '+' {
        return None;
    }
    let rest = &l[first.len_utf8()..];
    let ws_len = rest.len() - rest.trim_start().len();
    let after_ws = &rest[ws_len..];
    let dig_len = after_ws.len() - after_ws.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if dig_len > 0 {
        let after_dig = &after_ws[dig_len..];
        if let Some((sep_idx, sep)) = after_dig.char_indices().find(|(_, c)| c.is_whitespace()) {
            let cut = first.len_utf8() + ws_len + dig_len + sep_idx + sep.len_utf8();
            return Some((first, &l[..cut], &l[cut..]));
        }
    }
    Some((first, &l[..first.len_utf8()], rest))
}

/// 1-1 行对 → 行内分段行（lead 未染 + 变更词反视频；task2.1）。
fn intra_spans(lead: &str, segs: &[Seg0], color: Color) -> Line<'static> {
    let mut spans = vec![Span::styled(lead.to_string(), Style::default().fg(color))];
    for s in segs {
        let mut style = Style::default().fg(color);
        if s.changed {
            style = style.add_modifier(Modifier::REVERSED);
        }
        spans.push(Span::styled(s.text.clone(), style));
    }
    Line::from(spans)
}

/// 行模型 → 上屏行（pi renderDiff 同源规则：连续 - 块与紧随 + 块恰各 1 行
/// → 行内分段；否则整行红/绿；上下文原样、@@ 青）。
///
/// 输入: rows — 行模型序列；
/// 输出: 着色行序列
fn render_diff_rows(rows: &[DiffRow]) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        if matches!(rows[i], DiffRow::Minus { .. }) {
            let mut neg: Vec<(String, String)> = Vec::new();
            let mut j = i;
            while let Some(DiffRow::Minus { lead, content }) = rows.get(j) {
                neg.push((lead.clone(), content.clone()));
                j += 1;
            }
            let mut pos: Vec<(String, String)> = Vec::new();
            while let Some(DiffRow::Plus { lead, content }) = rows.get(j) {
                pos.push((lead.clone(), content.clone()));
                j += 1;
            }
            if neg.len() == 1 && pos.len() == 1 {
                let (old_segs, new_segs) = intra_line_diff(&neg[0].1, &pos[0].1);
                out.push(intra_spans(&neg[0].0, &old_segs, Color::Red));
                out.push(intra_spans(&pos[0].0, &new_segs, Color::Green));
            } else {
                for (lead, content) in neg {
                    out.push(Line::from(Span::styled(
                        format!("{lead}{content}"),
                        Style::default().fg(Color::Red),
                    )));
                }
                for (lead, content) in pos {
                    out.push(Line::from(Span::styled(
                        format!("{lead}{content}"),
                        Style::default().fg(Color::Green),
                    )));
                }
            }
            i = j;
            continue;
        }
        match &rows[i] {
            DiffRow::Plus { lead, content } => out.push(Line::from(Span::styled(
                format!("{lead}{content}"),
                Style::default().fg(Color::Green),
            ))),
            DiffRow::Hunk(t) => out.push(Line::from(Span::styled(
                t.clone(),
                Style::default().fg(Color::Cyan),
            ))),
            DiffRow::Context(t) => out.push(Line::from(Span::raw(t.clone()))),
            DiffRow::Minus { .. } => unreachable!(),
        }
        i += 1;
    }
    out
}

/// 行级 LCS 对比，产出 `-old`/`+new` 风格行（diff 降级路径）。
///
/// 输入: old — 旧文本；new — 新文本（行数超 400 时降级为整块红绿）
/// 输出: 带红绿样式的 diff 行
fn line_diff(old: &str, new: &str) -> Vec<Line<'static>> {
    render_diff_rows(&line_diff_rows(old, new))
}

/// 行级 LCS → 行模型（`line_diff` 与预览计算共用；task3.2 提取）。
///
/// 输入: old/new — 两段完整文本；
/// 输出: 行模型序列（1-1 分段由渲染器统一处理）
pub(crate) fn line_diff_rows(old: &str, new: &str) -> Vec<DiffRow> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();

    // 行数过大时不做 O(n*m)，直接整块对比（渲染器对多行 run 也整行）。
    if a.len().saturating_mul(b.len()) > 160_000 {
        return a
            .iter()
            .map(|l| DiffRow::Minus {
                lead: "-".to_string(),
                content: (*l).to_string(),
            })
            .chain(b.iter().map(|l| DiffRow::Plus {
                lead: "+".to_string(),
                content: (*l).to_string(),
            }))
            .collect();
    }

    // 经典 LCS DP → 行模型。
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
    let mut rows: Vec<DiffRow> = Vec::new();
    while i < n && j < m {
        if a[i] == b[j] {
            rows.push(DiffRow::Context(a[i].to_string()));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            rows.push(DiffRow::Minus {
                lead: "-".to_string(),
                content: a[i].to_string(),
            });
            i += 1;
        } else {
            rows.push(DiffRow::Plus {
                lead: "+".to_string(),
                content: b[j].to_string(),
            });
            j += 1;
        }
    }
    for l in &a[i..] {
        rows.push(DiffRow::Minus {
            lead: "-".to_string(),
            content: (*l).to_string(),
        });
    }
    for l in &b[j..] {
        rows.push(DiffRow::Plus {
            lead: "+".to_string(),
            content: (*l).to_string(),
        });
    }
    rows
}

/// write：优先 args.content 预览（写入内容），退回结果文本；按扩展名
/// 语法高亮（add-tool-card-syntax-highlighting design D4——与 read 同
/// 入口，path/filePath 双键；None 走现状纯文本逐字节等价）。
fn write_body(args: &Value, tool: &ToolExecution, theme: &crate::app::theme::TuiTheme) -> Vec<Line<'static>> {
    // owned 兑底：result_text 的 String 需活过借用。
    let fallback = result_text(tool);
    let content: Option<&str> = if let Some(c) = args.get("content").and_then(|c| c.as_str())
        && !c.is_empty()
    {
        Some(c.trim_end())
    } else {
        fallback.as_deref()
    };
    let Some(content) = content else {
        return Vec::new();
    };
    let path = args
        .get("path")
        .or_else(|| args.get("filePath"))
        .and_then(|p| p.as_str())
        .unwrap_or("");
    crate::app::markdown::highlight_by_path_themed(path, content, theme)
        .unwrap_or_else(|| text_lines(content))
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

// ---------- 中性语义模型（TUI/GPUI 双端共享，design D1） ----------

/// 语义段角色：模型只携带"这是什么"，样式由各渲染端自行映射
/// （TUI → ratatui Style，GPUI → palette）。`Fg` 为前景色透传
/// （语法高亮/diff/exit/警告等共享层既有逻辑产出的颜色，原样带给两端）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegRole {
    /// 状态点 `●` 与 ` · <status>` 状态文字（pending/running → 黄）。
    StatusRun,
    /// 成功状态（done → 绿）。
    StatusOk,
    /// 失败状态（error → 红）。
    StatusErr,
    /// 工具名（粗体）。
    Name,
    /// 头行关键参数（默认色）。
    Arg,
    /// 计时（`Took`/`Elapsed`，暗灰）。
    Timer,
    /// 正文（默认色）。
    Body,
    /// 折叠提示行（暗灰）。
    Hint,
    /// 通知 error（红粗）。
    NoticeError,
    /// 通知 retry（黄）。
    NoticeRetry,
    /// 通知其它（暗灰斜体）。
    Notice,
    /// thinking 正文/占位（暗灰斜体）。
    ThinkingText,
    /// bash 命令行（黄粗）。
    Cmd,
    /// bash 运行中/取消/truncated 提示（黄）。
    Warn,
    /// bash `exit 0`（绿）。
    ExitOk,
    /// bash `exit N`（N≠0，红粗）。
    ExitBad,
    /// bash 输出行/标记/done（暗灰）。
    Dim,
    /// 显式前景色透传（语法高亮 token、diff 行、exit 行、截断警告等）。
    Fg(Color),
    /// 行内反视频变更词（fg=行色；TUI 加 REVERSED、GPUI 与行底对调——
    /// align-edit-diff-rendering design D4：Seg 桥须携带反视频语义）。
    InverseFg(Color),
}

/// 语义段：一段同角色文本（模型的最小着色单元）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seg {
    /// 段文本（可为空串——空行占位，保住行数）。
    pub text: String,
    /// 段角色。
    pub role: SegRole,
}

impl Seg {
    /// 构造语义段。
    ///
    /// 输入: text — 段文本；role — 段角色
    /// 输出: `Seg` 实例
    pub fn new(text: impl Into<String>, role: SegRole) -> Self {
        Self {
            text: text.into(),
            role,
        }
    }
}

/// 工具卡中性模型：头行 + 折叠后正文 + 折叠提示 + 受保护尾部。
///
/// 与渲染框架无关的"该显示什么"唯一真相源（design D1）：TUI 经
/// `tool_lines_themed` 薄适配上屏，GPUI 用内建文本 runs 消费；两端
/// MUST NOT 另存一份卡片语义副本。
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCardModel {
    /// 头行段序列（单行：`● name · 关键参数 · 状态 · 计时`）。
    pub head: Vec<Seg>,
    /// 结果体（元素 = 屏行，行内多段；已按折叠策略截断，展开态为全量）。
    pub body: Vec<Vec<Seg>>,
    /// 折叠提示行（仅折叠且发生截断时 Some；展开态恒 None）。
    pub hint: Option<Seg>,
    /// 受保护尾部（exit 状态行 + 截断警告；不参与折叠计数、始终可见）。
    pub tail: Vec<Vec<Seg>>,
}

/// 构建工具卡中性模型（语义逻辑下沉的唯一入口，design D1）。
///
/// 功能: 解析参数 → 头行段（head_arg/timer_text）→ 按工具取结果体
///       → 折叠（collapse_policy）→ 受保护尾部，组装 [`ToolCardModel`]；
/// 输入: name — 工具名（read/bash/edit/write/grep/find/ls/未知均可）；
///       args_json — 调用参数原文（非法 JSON 降级 Value::Null，不 panic）；
///       tool — 执行状态（status/is_error/result/partial/duration/起点）；
///       expanded — 展开态（跳过折叠与 compact 头行分类）；
///       theme — 注入配色（read/write 语法高亮取 syntax_theme）
/// 输出: 语义模型；无结果时 body 为空、无提示行，不 panic
#[allow(dead_code)] // 测试/oracle 包装（产线走 tool_card_model_ctx；task3.3）
pub fn tool_card_model(
    name: &str,
    args_json: &str,
    tool: &ToolExecution,
    expanded: bool,
    theme: &crate::app::theme::TuiTheme,
) -> ToolCardModel {
    tool_card_model_ctx(name, args_json, tool, expanded, theme, None)
}

/// [`tool_card_model`] 预览版（task3.3）：edit 卡带执行前预览态。
///
/// 输入: 同 [`tool_card_model`]；preview — 该 toolId 的预览态（None=无）；
/// 输出: 同 [`tool_card_model`]
pub fn tool_card_model_ctx(
    name: &str,
    args_json: &str,
    tool: &ToolExecution,
    expanded: bool,
    theme: &crate::app::theme::TuiTheme,
    preview: Option<&EditPreviewState>,
) -> ToolCardModel {
    let args = serde_json::from_str::<Value>(args_json).unwrap_or(Value::Null);
    let head = head_segs(name, &args, tool, expanded);

    let body_lines = match name {
        "read" => read_body(&args, tool, theme),
        "bash" | "powershell" => bash_output(&args, tool),
        "edit" => edit_body(&args, tool, preview),
        "write" => write_body(&args, tool, theme),
        "grep" | "find" | "ls" => text_result_body(tool),
        _ => generic_body(&args, tool, expanded),
    };
    let body = lines_to_segs(body_lines);

    let (body, hint) = if expanded {
        (body, None)
    } else {
        collapse_rows(name, body)
    };
    let tail = protected_tail_rows(name, tool);

    ToolCardModel {
        head,
        body,
        hint,
        tail,
    }
}

/// 头行段序列（镜像 `head_line` 的 span 构成，语义角色化）。
///
/// 输入: name/args/expanded — 同 [`tool_card_model`]；tool — 取状态与计时
/// 输出: 单行段序列（`● ` + 名字 + 关键参数 + ` · 状态` + ` · 计时`，
///       后三段按 head_arg/timer_text 缺省省略）
fn head_segs(name: &str, args: &Value, tool: &ToolExecution, expanded: bool) -> Vec<Seg> {
    let (status_text, status_role) = match tool.status {
        ToolStatus::Pending => ("pending", SegRole::StatusRun),
        ToolStatus::Running => ("running", SegRole::StatusRun),
        ToolStatus::Ended if tool.is_error => ("error", SegRole::StatusErr),
        ToolStatus::Ended => ("done", SegRole::StatusOk),
    };
    let mut segs = vec![
        Seg::new("● ", status_role),
        Seg::new(name, SegRole::Name),
    ];
    if let Some(key) = head_arg(name, args, expanded) {
        segs.push(Seg::new(format!(" {key}"), SegRole::Arg));
    }
    segs.push(Seg::new(format!(" · {status_text}"), status_role));
    if let Some(timer) = timer_text(tool) {
        segs.push(Seg::new(format!(" · {timer}"), SegRole::Timer));
    }
    segs
}

/// ratatui 行 → 中性段行（正文/尾部桥接；行样式仅前景色与无样式两种，
/// 逐 span 透传不丢色）。
///
/// 输入: lines — 共享层既有行构建器产出（语法高亮/diff/纯文本）
/// 输出: 等行数的段行序列（span 有 fg → [`SegRole::Fg`]，无 → `Body`；
///       空 span 行 → 空段行，行数与渲染均与原行等价）
fn lines_to_segs(lines: Vec<Line<'static>>) -> Vec<Vec<Seg>> {
    lines
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|s| {
                    // 反视频修饰符须贯穿 Seg 桥（design D4 修正：否则两端全丢）。
                    let role = if s.style.add_modifier.contains(Modifier::REVERSED)
                        && let Some(c) = s.style.fg
                    {
                        SegRole::InverseFg(c)
                    } else {
                        match s.style.fg {
                            Some(c) => SegRole::Fg(c),
                            None => SegRole::Body,
                        }
                    };
                    Seg::new(s.content.to_string(), role)
                })
                .collect()
        })
        .collect()
}

/// 按折叠策略截断正文行并生成提示行（语义版 `collapse`，design D6）。
///
/// 输入: name — 工具名（选策略：bash/powershell 尾 5、grep 15、
///       find/ls 20、write 与其余头 10）；body — 全量结果体段行
/// 输出: `(保留行, 提示段)`——未超限原样返回且提示为 None；
///       提示文案：bash 尾留 `earlier lines`、write 带总行数、其余 `more lines`
fn collapse_rows(name: &str, body: Vec<Vec<Seg>>) -> (Vec<Vec<Seg>>, Option<Seg>) {
    let (keep, keep_tail) = collapse_policy(name);
    if body.len() <= keep {
        return (body, None);
    }
    let total = body.len();
    let hidden = total - keep;
    let shown = if keep_tail {
        body[total - keep..].to_vec()
    } else {
        body[..keep].to_vec()
    };
    let text = if keep_tail {
        format!("... ({hidden} earlier lines, ctrl+o to expand)")
    } else if name == "write" {
        format!("... ({hidden} more lines, {total} total, ctrl+o to expand)")
    } else {
        format!("... ({hidden} more lines, ctrl+o to expand)")
    };
    (shown, Some(Seg::new(text, SegRole::Hint)))
}

/// 受保护尾部段行（语义版 `protected_tail`）：bash exit 状态行 +
/// 各工具截断警告——不参与折叠计数、始终可见。
///
/// 输入: name — 工具名；tool — 执行状态（读 result/partial 详情）
/// 输出: 行序列（无内容时为空；行内前景色经 [`SegRole::Fg`] 透传）
fn protected_tail_rows(name: &str, tool: &ToolExecution) -> Vec<Vec<Seg>> {
    lines_to_segs(protected_tail(name, tool))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::theme::TuiTheme;
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

    /// 行文本（拼接 span）——定位高亮行用（syntect 会把词拆成多 span）。
    fn line_text(l: &Line<'static>) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect::<Vec<_>>().join("")
    }

    /// 行内首个带前景色 span 的 fg（None = 该行无着色）。
    fn body_fg_of(lines: &[Line<'static>], needle: &str) -> Option<Color> {
        lines
            .iter()
            .find(|l| line_text(l).contains(needle))
            .and_then(|l| l.spans.iter().find(|s| s.style.fg.is_some()).map(|s| s.style.fg))
            .flatten()
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

    /// 模型行文本（行内段拼接）——model 单测断言用。
    fn row_text(row: &[Seg]) -> String {
        row.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join("")
    }

    /// 模型全量文本（头行/正文/提示/尾部逐行拼接）。
    fn model_text(m: &ToolCardModel) -> String {
        let mut out = row_text(&m.head);
        for row in &m.body {
            out.push('\n');
            out.push_str(&row_text(row));
        }
        if let Some(hint) = &m.hint {
            out.push('\n');
            out.push_str(&hint.text);
        }
        for row in &m.tail {
            out.push('\n');
            out.push_str(&row_text(row));
        }
        out
    }

    /// model 单测（task 1.1）：bash 折叠取尾 5 行 + 提示行 + exit 受保护。
    #[test]
    fn model_bash_collapses_tail_with_hint_and_protected_exit() {
        let output = (1..=40).map(|i| format!("line{i}")).collect::<Vec<_>>().join("\n");
        let tool = ToolExecution {
            status: ToolStatus::Ended,
            is_error: false,
            result: Some(serde_json::json!({
                "content": [{"type":"text","text": output}],
                "structuredContent": {"output": output, "exit_code": 0}
            })),
            duration_ms: Some(830),
            ..Default::default()
        };
        let m = tool_card_model(
            "bash",
            r#"{"command":"ls -la","timeout":5}"#,
            &tool,
            false,
            &TuiTheme::default(),
        );

        // 尾 5 行（keep_tail）
        assert_eq!(m.body.len(), 5, "bash 折叠取尾 5 行");
        assert_eq!(row_text(&m.body[0]), "line36");
        assert_eq!(row_text(&m.body[4]), "line40");
        // 提示行（earlier 变体）
        assert_eq!(
            m.hint.as_ref().map(|s| s.text.as_str()),
            Some("... (35 earlier lines, ctrl+o to expand)")
        );
        // exit 状态行在受保护尾部、不参与折叠
        assert_eq!(m.tail.len(), 1);
        assert_eq!(row_text(&m.tail[0]), "exit 0");
        assert_eq!(m.tail[0][0].role, SegRole::Fg(Color::Green));
        // 头行：bash prompt + timeout + 状态 + 计时
        let head = row_text(&m.head);
        assert!(head.contains("$ ls -la (timeout 5s)"), "头行参数: {head}");
        assert!(head.contains("· done"), "头行状态: {head}");
        assert!(head.contains("Took 830ms") || head.contains("Took 0.8s"), "头行计时: {head}");
    }

    /// model 单测（task 1.1）：grep 折叠取头 15 行 + 头行 `/pat/ in path`。
    #[test]
    fn model_grep_collapses_head_15() {
        let output = (1..=30).map(|i| format!("match{i}")).collect::<Vec<_>>().join("\n");
        let tool = ToolExecution {
            status: ToolStatus::Ended,
            is_error: false,
            result: Some(serde_json::json!({"content": [{"type":"text","text": output}]})),
            ..Default::default()
        };
        let m = tool_card_model(
            "grep",
            r#"{"pattern":"foo","path":"src"}"#,
            &tool,
            false,
            &TuiTheme::default(),
        );

        assert_eq!(m.body.len(), 15, "grep 折叠取头 15 行");
        assert_eq!(row_text(&m.body[0]), "match1");
        assert_eq!(
            m.hint.as_ref().map(|s| s.text.as_str()),
            Some("... (15 more lines, ctrl+o to expand)")
        );
        assert!(row_text(&m.head).contains("/foo/ in src"), "grep 头行");
        assert!(m.tail.is_empty(), "grep 无受保护尾部");
    }

    /// model 单测（task 1.1）：read 折叠取头 10 行 + 行范围头行；展开态无提示。
    #[test]
    fn model_read_collapses_head_10_and_expanded_drops_hint() {
        let content = (1..=20).map(|i| format!("row{i}")).collect::<Vec<_>>().join("\n");
        let tool = ToolExecution {
            status: ToolStatus::Ended,
            is_error: false,
            result: Some(serde_json::json!({"content": [{"type":"text","text": content}]})),
            ..Default::default()
        };
        let args = r#"{"path":"src/main.rs","offset":1,"limit":20}"#;

        let collapsed = tool_card_model("read", args, &tool, false, &TuiTheme::default());
        assert_eq!(collapsed.body.len(), 10, "read 折叠取头 10 行");
        assert_eq!(row_text(&collapsed.body[0]), "row1");
        assert_eq!(
            collapsed.hint.as_ref().map(|s| s.text.as_str()),
            Some("... (10 more lines, ctrl+o to expand)")
        );
        assert!(row_text(&collapsed.head).contains("src/main.rs:1-20"), "行范围头行");

        let expanded = tool_card_model("read", args, &tool, true, &TuiTheme::default());
        assert_eq!(expanded.body.len(), 20, "展开态全量");
        assert!(expanded.hint.is_none(), "展开态无提示行");
    }

    /// model 单测（task 1.1）：计时三态经头行 Timer 段呈现（Took/无）。
    #[test]
    fn model_timer_three_states_via_head() {
        let theme = TuiTheme::default();
        let mk = |status: ToolStatus, duration_ms: Option<u64>| ToolExecution {
            status,
            duration_ms,
            ..Default::default()
        };
        let has_timer = |m: &ToolCardModel| m.head.iter().any(|s| s.role == SegRole::Timer);
        let timer_text_of = |m: &ToolCardModel| -> String {
            m.head
                .iter()
                .filter(|s| s.role == SegRole::Timer)
                .map(|s| s.text.as_str())
                .collect()
        };

        let ended = tool_card_model("bash", "{}", &mk(ToolStatus::Ended, Some(90_000)), false, &theme);
        assert!(has_timer(&ended), "Ended+duration 应有计时段");
        assert!(timer_text_of(&ended).contains("Took 1m 30s"), "Took 服务端值");

        let no_timer = tool_card_model("bash", "{}", &mk(ToolStatus::Ended, None), false, &theme);
        assert!(!has_timer(&no_timer), "皆无不显示计时段");

        // Running + 起点 → Elapsed（共享 timer_text 同源）
        let running_tool = ToolExecution {
            status: ToolStatus::Running,
            started_at: Some(std::time::Instant::now() - std::time::Duration::from_secs(5)),
            ..Default::default()
        };
        let running = tool_card_model("bash", "{}", &running_tool, false, &theme);
        assert!(timer_text_of(&running).contains("Elapsed 5."), "运行中 Elapsed");
    }

    /// model 单测（task 1.1）：状态角色映射（done→StatusOk、error→StatusErr、
    /// 运行中→StatusRun），头行分段角色齐全。
    #[test]
    fn model_status_roles_and_head_segments() {
        let theme = TuiTheme::default();
        let mk = |status: ToolStatus, is_error: bool| ToolExecution {
            status,
            is_error,
            ..Default::default()
        };

        let ok = tool_card_model("read", r#"{"path":"/x"}"#, &mk(ToolStatus::Ended, false), false, &theme);
        assert_eq!(ok.head[0].role, SegRole::StatusOk);
        assert_eq!(ok.head[1].role, SegRole::Name);
        assert!(ok.head.iter().any(|s| s.role == SegRole::Arg), "应有参数段");

        let err = tool_card_model("read", r#"{"path":"/x"}"#, &mk(ToolStatus::Ended, true), false, &theme);
        assert_eq!(err.head[0].role, SegRole::StatusErr);

        let run = tool_card_model("read", r#"{"path":"/x"}"#, &mk(ToolStatus::Running, false), false, &theme);
        assert_eq!(run.head[0].role, SegRole::StatusRun);
    }

    /// model 单测（task 1.1）：无结果不 panic、body 为空；未知工具兜底。
    #[test]
    fn model_empty_result_and_unknown_tool() {
        let theme = TuiTheme::default();
        let empty = ToolExecution::default();
        let m = tool_card_model("read", r#"{"path":"/x"}"#, &empty, false, &theme);
        assert!(m.body.is_empty(), "无结果 body 为空");
        assert!(m.hint.is_none(), "未超限无提示");

        let unknown = tool_card_model("mystery", r#"{"foo":1}"#, &empty, true, &theme);
        assert!(!unknown.head.is_empty(), "未知工具仍有头行");
    }

    /// model 单测（task 1.1）：与 `tool_lines_themed` 输出逐行等价（TUI 薄适配
    /// 前提——两端同语义，行文本完全一致）。
    #[test]
    fn model_matches_tool_lines_text() {
        let theme = TuiTheme::default();
        let tool = ToolExecution {
            status: ToolStatus::Ended,
            is_error: false,
            result: Some(serde_json::json!({
                "content": [{"type":"text","text":"a\nb\nc"}],
                "structuredContent": {"output": "a\nb\nc", "exit_code": 2}
            })),
            duration_ms: Some(42),
            ..Default::default()
        };
        for (name, args, expanded) in [
            ("bash", r#"{"command":"echo hi"}"#, false),
            ("bash", r#"{"command":"echo hi"}"#, true),
            ("grep", r#"{"pattern":"p","path":"."}"#, false),
            ("read", r#"{"path":"/x/y.rs","offset":2,"limit":3}"#, false),
            ("mystery", r#"{"a":1}"#, true),
        ] {
            let lines = tool_lines_themed(name, args, &tool, expanded, &theme);
            let m = tool_card_model(name, args, &tool, expanded, &theme);
            let lines_text = render(&lines);
            let model = model_text(&m);
            assert_eq!(lines_text, model, "同语义等价: {name} expanded={expanded}");
        }
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

    /// 任务 1.2：未知扩展降级逐字节等价（add-tool-card-syntax-highlighting
    /// design D1）——None 分支必须走现状纯文本路径：行文本与原文一致、
    /// 原始 tab 保留（高亮链会把 tab 展开为 4 空格——保留即证明未走高亮
    /// 链）、body span 无任何着色；为 2.1 的 None 回退钉住基线。
    /// 任务 2.1：read 卡按扩展名高亮（add-tool-card-syntax-highlighting
    /// design D2/D3）——折叠（前 10 行 + 提示行）与展开全文均有着色、
    /// 同 token 两态 fg 一致；高亮 span 不携带 bg（面板底色保留前提）；
    /// 提示行/行数不变（A 案：先高亮全文、collapse 再切，pi 同序）。
    #[test]
    fn read_card_highlights_collapsed_and_expanded_with_panel_bg() {
        // 12 行 > 折叠上限 10 → 折叠态必现提示行
        let content = concat!(
            "import os\n",
            "def main():\n",
            "    print(\"hi\")\n",
            "    if True:\n",
            "        pass\n",
            "# a comment\n",
            "s = \"string here\"\n",
            "for i in range(3):\n",
            "    print(i)\n",
            "x = os.getcwd()\n",
            "end_one\n",
            "end_two\n",
        );
        let tool = ToolExecution {
            status: ToolStatus::Ended,
            is_error: false,
            result: Some(serde_json::json!({"content":[{"type":"text","text": content}]})),
            duration_ms: Some(1),
            ..Default::default()
        };
        let args = r#"{"path":"/tmp/demo.py"}"#;
        let collapsed = tool_lines("read", args, &tool, false);
        let expanded = tool_lines("read", args, &tool, true);

        // 折叠：提示行保留（行数与折叠行为不变——A 案结构保证）
        let collapsed_text = render(&collapsed);
        assert!(
            collapsed_text.contains("ctrl+o to expand"),
            "提示行保留: {collapsed_text}"
        );

        // 正文行（以 import os 定位，避开头部样式行）两态均有前景色
        // 按行文本定位（syntect 会把 "import os" 拆成多 span，单 span 不含整串）
        let line_text = |l: &Line<'static>| -> String {
            l.spans.iter().map(|s| s.content.as_ref()).collect::<Vec<_>>().join("")
        };
        let body_fg = |lines: &Vec<Line<'static>>| {
            lines
                .iter()
                .find(|l| line_text(l).contains("import os"))
                .and_then(|l| l.spans.iter().find(|s| s.style.fg.is_some()).map(|s| s.style.fg))
                .flatten()
        };
        let cf = body_fg(&collapsed).expect("折叠正文应有着色");
        let ef = body_fg(&expanded).expect("展开正文应有着色");
        assert_eq!(cf, ef, "折叠/展开同 token 同色（着色一致）");

        // 高亮 span 一律无 bg（叠加在 chat 层面板底色上不冲掉，design D3）
        for lines in [&collapsed, &expanded] {
            assert!(
                lines
                    .iter()
                    .all(|l| l.spans.iter().all(|s| s.style.bg.is_none())),
                "高亮 span 不携带 bg"
            );
        }

        // 展开态无提示行（全文可见）
        assert!(
            !render(&expanded).contains("ctrl+o to expand"),
            "展开无提示行"
        );
    }

    #[test]
    fn unknown_extension_falls_back_byte_identical() {
        let content = "plain line one\n\ttabbed line\nlast line";
        let tool = ToolExecution {
            status: ToolStatus::Ended,
            is_error: false,
            result: Some(serde_json::json!({"content":[{"type":"text","text": content}]})),
            duration_ms: Some(1),
            ..Default::default()
        };
        let args = r#"{"path":"/tmp/x.xyzzy"}"#;
        let lines = tool_lines("read", args, &tool, true);
        let text = render(&lines);

        // 行文本与原文逐行一致（含原始 tab）
        assert!(text.contains("plain line one"), "{text}");
        assert!(text.contains("\ttabbed line"), "原始 tab 保留: {text:?}");
        assert!(text.contains("last line"), "{text}");

        // body span 无着色（现状纯文本 = 降级路径）
        let body_line = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.as_ref().contains("plain line one")))
            .expect("正文行存在");
        assert!(
            body_line
                .spans
                .iter()
                .all(|s| s.style.fg.is_none() && s.style.bg.is_none()),
            "降级行无任何着色"
        );
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

    // ===== 行内高亮（align-edit-diff-rendering task 1.1/1.2）=====

    /// 1.1 词级分段：单词替换只染该词、无变化全净、插入/删除对侧不动、
    /// 超长行退化不分段（防卡）。
    #[test]
    fn intra_line_diff_word_segmentation() {
        // 单词替换 → 仅该词 changed，其余合并为未变更段
        let (o, n) = intra_line_diff("const x = 1;", "const y = 1;");
        assert_eq!(
            o.iter().map(|s| (s.text.as_str(), s.changed)).collect::<Vec<_>>(),
            vec![("const ", false), ("x", true), (" = 1;", false)]
        );
        assert_eq!(
            n.iter().map(|s| (s.text.as_str(), s.changed)).collect::<Vec<_>>(),
            vec![("const ", false), ("y", true), (" = 1;", false)]
        );

        // 无变化 → 单段未变更
        let (o, n) = intra_line_diff("same line", "same line");
        assert_eq!(o, vec![Seg0 { text: "same line".into(), changed: false }]);
        assert_eq!(n, vec![Seg0 { text: "same line".into(), changed: false }]);

        // 插入词：旧侧全净、新侧变更段含新词
        let (o, n) = intra_line_diff("a c", "a b c");
        assert!(o.iter().all(|s| !s.changed), "旧侧应全净: {o:?}");
        let new_changed: String = n.iter().filter(|s| s.changed).map(|s| s.text.as_str()).collect();
        assert!(new_changed.contains('b'), "新侧变更含插入词: {n:?}");

        // 删除词：新侧全净、旧侧变更段含被删词
        let (o, n) = intra_line_diff("a b c", "a c");
        assert!(n.iter().all(|s| !s.changed), "新侧应全净: {n:?}");
        let old_changed: String = o.iter().filter(|s| s.changed).map(|s| s.text.as_str()).collect();
        assert!(old_changed.contains('b'), "旧侧变更含删除词: {o:?}");

        // 超长行退化：token 乘积超上限 → 单段不分段（不卡）
        let words = |n: usize| (0..n).map(|i| format!("w{i} ")).collect::<String>();
        let (o, n) = intra_line_diff(&words(450), &words(450));
        assert_eq!(o.len(), 1, "退化为单段");
        assert!(!o[0].changed, "退化不分段");
        assert_eq!(n.len(), 1);
    }

    /// 1.2 pi 缩进与首段规则：共同缩进两侧原样未变更；新增前导空白
    /// 首段不染（缩进插入不被标成变更）。
    #[test]
    fn intra_line_diff_indent_and_first_ws_rules() {
        // 共同缩进：两侧首段 = 缩进且未变更
        let (o, n) = intra_line_diff("    foo()", "    bar()");
        assert_eq!(o[0].text, "    ");
        assert!(!o[0].changed, "旧侧缩进不染");
        assert_eq!(n[0].text, "    ");
        assert!(!n[0].changed, "新侧缩进不染");
        let o_changed: String = o.iter().filter(|s| s.changed).map(|s| s.text.as_str()).collect();
        let n_changed: String = n.iter().filter(|s| s.changed).map(|s| s.text.as_str()).collect();
        assert!(o_changed.contains("foo") && n_changed.contains("bar"), "变更词仍染");

        // 新增前导空白：old 无缩进、new 加缩进 → 新侧首段（空白）不染
        let (o, n) = intra_line_diff("value = 1", "    value = 2");
        let first = n.first().expect("新侧非空");
        assert!(first.text.chars().all(|c| c.is_whitespace()), "首段应为纯空白: {n:?}");
        assert!(!first.changed, "首段空白不染（pi 首段规则）");
        let o_changed: String = o.iter().filter(|s| s.changed).map(|s| s.text.as_str()).collect();
        assert_eq!(o_changed.trim(), "1", "旧侧仅值变更: {o:?}");
        let n_changed: String = n.iter().filter(|s| s.changed).map(|s| s.text.as_str()).collect();
        assert_eq!(n_changed.trim(), "2", "新侧仅值变更: {n:?}");
    }

    /// 2.1 details.diff 的 1-1 行对 → 行内反视频；多行块整行；
    /// 上下文原样、@@ 青（既有语义不回退）。
    #[test]
    fn edit_details_diff_pairs_single_line_with_reversed() {
        let tool = ToolExecution {
            status: ToolStatus::Ended,
            is_error: false,
            result: Some(serde_json::json!({
                "content": [{"type":"text","text":"ok"}],
                "details": {"diff": "-1 alpha beta\n+1 alpha gamma\n 2 keep\n@@ hunk here"}
            })),
            duration_ms: None,
            ..Default::default()
        };
        let lines = tool_lines("edit", r#"{"file_path":"/x"}"#, &tool, true);
        // 输出 = 头行 + diff 正文行
        assert!(lines[0].spans.iter().any(|s| s.content.contains("edit")), "头行在首位");
        let body = &lines[1..];

        // 正文行0：-1 alpha beta —— lead 与未变词红、变更词红 + 反视频
        let l0 = &body[0];
        assert!(
            l0.spans.iter().any(|s| {
                s.content == "beta"
                    && s.style.add_modifier.contains(Modifier::REVERSED)
                    && s.style.fg == Some(Color::Red)
            }),
            "旧侧行内变更词反视频: {l0:?}"
        );
        assert!(
            l0.spans.iter().any(|s| {
                s.content.contains("alpha") && !s.style.add_modifier.contains(Modifier::REVERSED)
            }),
            "未变词不染反视频: {l0:?}"
        );

        // 行1：+1 alpha gamma —— 绿 + 反视频
        let l1 = &body[1];
        assert!(
            l1.spans.iter().any(|s| {
                s.content == "gamma"
                    && s.style.add_modifier.contains(Modifier::REVERSED)
                    && s.style.fg == Some(Color::Green)
            }),
            "新侧行内变更词反视频: {l1:?}"
        );

        // 上下文行原样（无 fg、无反视频）
        let l2 = &body[2];
        assert!(
            l2.spans.iter().all(|s| s.style.fg.is_none()
                && !s.style.add_modifier.contains(Modifier::REVERSED)),
            "上下文行原样: {l2:?}"
        );
        // @@ 青色保留
        let l3 = &body[3];
        assert!(
            l3.spans.iter().any(|s| s.style.fg == Some(Color::Cyan)),
            "@@ 仍青: {l3:?}"
        );

        // 多行块 → 整行红绿、无反视频
        let multi = ToolExecution {
            status: ToolStatus::Ended,
            is_error: false,
            result: Some(serde_json::json!({
                "content": [{"type":"text","text":"ok"}],
                "details": {"diff": "-1 a\n-2 b\n+1 c\n+2 d"}
            })),
            duration_ms: None,
            ..Default::default()
        };
        let lines = tool_lines("edit", r#"{"file_path":"/x"}"#, &multi, true);
        let body = &lines[1..]; // 去头行
        assert_eq!(body.len(), 4);
        assert!(
            body.iter().all(|l| l.spans.iter().all(|s| {
                !s.style.add_modifier.contains(Modifier::REVERSED)
            })),
            "多行块整行不反视频"
        );
        assert!(body[0].spans.iter().any(|s| s.style.fg == Some(Color::Red)));
        assert!(body[2].spans.iter().any(|s| s.style.fg == Some(Color::Green)));
    }

    /// 2.2 降级路径同染：单编辑对行内反视频、多行差异整行。
    #[test]
    fn line_diff_fallback_intra_and_multiline_whole() {
        // 单编辑对（old/new 各一行未变 + 一行变）→ 变更词反视频
        let tool = ToolExecution {
            status: ToolStatus::Ended,
            is_error: false,
            result: Some(serde_json::json!({"content":[{"type":"text","text":"ok"}]})),
            duration_ms: None,
            ..Default::default()
        };
        let args = r#"{"file_path":"/x","oldString":"alpha beta","newString":"alpha gamma"}"#;
        let lines = tool_lines("edit", args, &tool, true);
        let minus = lines.iter().find(|l| l.spans.first().map(|s| s.content.starts_with('-')).unwrap_or(false)).expect("应有 - 行");
        assert!(
            minus.spans.iter().any(|s| {
                s.content.contains("beta")
                    && s.style.add_modifier.contains(Modifier::REVERSED)
                    && s.style.fg == Some(Color::Red)
            }),
            "降级单对同染行内: {minus:?}"
        );

        // 多行差异 → 整行红绿、无反视频
        let args = r#"{"file_path":"/x","oldString":"a\nb","newString":"c\nd"}"#;
        let lines = tool_lines("edit", args, &tool, true);
        assert!(
            lines.iter().all(|l| l.spans.iter().all(|s| {
                !s.style.add_modifier.contains(Modifier::REVERSED)
            })),
            "多行降级整行不反视频"
        );
        assert!(lines.iter().any(|l| l.spans.iter().any(|s| s.style.fg == Some(Color::Red))));
        assert!(lines.iter().any(|l| l.spans.iter().any(|s| s.style.fg == Some(Color::Green))));
    }

    /// 3.3 预览渲染优先级与 Ready/Error 呈现（task3.3；spec: 结果替换、
    /// 先行展示、错误文案）。
    #[test]
    fn edit_preview_priority_and_ready_rendering() {
        // 结果 details 优先于预览（spec: 结果到达替换——渲染期同优先级）
        let (name, args, tool) = fixture_tool_call("tool_edit", "edit");
        let preview_rows = vec![DiffRow::Context("PREVIEW-LINE".to_string())];
        let lines = tool_lines_themed_ctx(
            &name,
            &args,
            &tool,
            true,
            &TuiTheme::default(),
            Some(&EditPreviewState::Ready(preview_rows)),
        );
        let text = render(&lines);
        assert!(text.contains("-1 FIXTURE-MARKER"), "结果 diff 在场: {text}");
        assert!(!text.contains("PREVIEW-LINE"), "结果优先、预览不叠: {text}");

        // 无结果 + Ready → 预览行呈现（1-1 对含行内反视频）
        let running = ToolExecution {
            status: ToolStatus::Running,
            is_error: false,
            result: None,
            duration_ms: None,
            ..Default::default()
        };
        let no_result_args = r#"{"file_path":"/x","oldText":"a","newText":"b"}"#;
        let rows = line_diff_rows("alpha beta", "alpha gamma");
        let lines = tool_lines_themed_ctx(
            "edit",
            no_result_args,
            &running,
            true,
            &TuiTheme::default(),
            Some(&EditPreviewState::Ready(rows)),
        );
        let text = render(&lines);
        assert!(text.contains("alpha"), "预览行呈现: {text}");
        assert!(
            lines
                .iter()
                .flat_map(|l| l.spans.iter())
                .any(|s| s.style.add_modifier.contains(Modifier::REVERSED)),
            "预览 1-1 对含反视频"
        );

        // Error → 红色错误文案行
        let lines = tool_lines_themed_ctx(
            "edit",
            no_result_args,
            &running,
            true,
            &TuiTheme::default(),
            Some(&EditPreviewState::Error("cannot read x".into())),
        );
        let text = render(&lines);
        assert!(text.contains("cannot read x"), "错误文案在场: {text}");
        assert!(
            lines.iter().flat_map(|l| l.spans.iter()).any(|s| {
                s.content.contains("cannot read") && s.style.fg == Some(Color::Red)
            }),
            "错误行红色"
        );
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

    /// 任务 2.2：write 卡按扩展名高亮（add-tool-card-syntax-highlighting
    /// design D4）——path/filePath 双键均着色；未知扩展降级与现状一致
    /// （纯文本无色）；高亮 span 不携带 bg。edit 卡 diff 行红绿青回归由
    /// 既有 edit_card_from_fixture_shows_colored_diff 保持绿。
    #[test]
    fn write_card_highlights_by_extension() {
        let rs = "fn main() {\n    println!(\"hi\");\n}\n";
        let content_json = serde_json::to_string(rs).unwrap();
        let tool = ToolExecution {
            status: ToolStatus::Ended,
            is_error: false,
            result: Some(serde_json::json!({"content":[{"type":"text","text":"wrote ok"}]})),
            duration_ms: Some(1),
            ..Default::default()
        };

        // path 键：.rs 内容着色
        let args = format!(r#"{{"path":"/tmp/demo.rs","content":{content_json}}}"#);
        let lines = tool_lines("write", &args, &tool, true);
        assert!(
            body_fg_of(&lines, "fn main()").is_some(),
            ".rs write 内容应着色: {}",
            render(&lines)
        );
        assert!(
            lines.iter().all(|l| l.spans.iter().all(|s| s.style.bg.is_none())),
            "高亮 span 不携带 bg"
        );

        // filePath 键（与 head 同约定的 camel 变体）：同样着色
        let args2 = format!(r#"{{"filePath":"/tmp/other.rs","content":{content_json}}}"#);
        let lines2 = tool_lines("write", &args2, &tool, true);
        assert!(
            body_fg_of(&lines2, "fn main()").is_some(),
            "filePath 键同样着色"
        );

        // 未知扩展：降级纯文本（行文本保留、无色——与现状一致）
        let args3 = format!(r#"{{"path":"/tmp/x.xyzzy","content":{content_json}}}"#);
        let lines3 = tool_lines("write", &args3, &tool, true);
        assert!(render(&lines3).contains("fn main()"), "降级行文本保留");
        assert!(
            body_fg_of(&lines3, "fn main()").is_none(),
            "降级无色（与现状一致）"
        );
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
