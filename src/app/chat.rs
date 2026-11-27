//! 会话状态 → 可渲染 ratatui 行的纯映射（markdown/工具卡片/thinking/通知）。
//!
//! 渲染偏好通过 [`RenderFlags`] 传入（不进 SessionState——它们是 UI 偏好，
//! 不是会话事实），保证相同状态 + 相同 flags 产出相同内容，可离线单测。

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use pi_shell_rs::session::{Block, Entry, SessionState, ToolExecution, ToolStatus};

use crate::app::theme::TuiTheme;
use crate::app::wrap::HitKind;

use super::{EditPreviewState, PreviewCtx};

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
///       的工具卡行追加 Tool(id)、非空 thinking 块整段追加
///       Thinking(seq)（design D1/D2）、用户/assistant 条目首逻辑行追加
///       MessageStart(msg_seq)（add-prompt-jump design D1/D3——每行
///       Vec 多标记共存，不覆盖内容标记）；其余（通知/`!` bash 等）
///       为空 Vec；thinking_overrides — 单块展开覆盖（seq → 是否展开，
///       无覆盖跟随 flags.show_thinking，design D3）
///
/// 默认配色薄包装（design D1 方案 A）：转发 [`chat_lines_themed`] 传
/// [`TuiTheme::default()`]——存量 28 处调用点（含测试）签名零改动。
/// 渲染主路径（view/选区文档）走 themed 入口；本包装服务存量测试与
/// 默认外观调用方。
#[allow(dead_code)]
pub fn chat_lines(
    state: &SessionState,
    flags: RenderFlags,
    overrides: &std::collections::HashMap<String, bool>,
    thinking_overrides: &std::collections::HashMap<usize, bool>,
) -> (Vec<Line<'static>>, Vec<Vec<HitKind>>) {
    chat_lines_themed(state, flags, overrides, thinking_overrides, &TuiTheme::default())
}

/// 构建全部聊天行（带注入配色，[`chat_lines`] 的 themed 实现体）。
///
/// 输入: state/flags/overrides/thinking_overrides — 同 [`chat_lines`]；
///       theme — 注入配色（工具卡三态面板色等，宿主配色注入 design D1）
/// 输出: 同 [`chat_lines`]；相同状态 + 相同 flags + 相同 theme 产出相同
///       内容（纯函数，可离线单测）
pub fn chat_lines_themed(
    state: &SessionState,
    flags: RenderFlags,
    overrides: &std::collections::HashMap<String, bool>,
    thinking_overrides: &std::collections::HashMap<usize, bool>,
    theme: &TuiTheme,
) -> (Vec<Line<'static>>, Vec<Vec<HitKind>>) {
    let mut lines = Vec::new();
    let mut meta: Vec<Vec<HitKind>> = Vec::new();
    // thinking 全局渲染序号（design D2：无条件计数，跨流式/落盘稳定）。
    let mut thinking_seq = 0usize;
    // 语义消息序号（add-prompt-jump design D3：仅用户/assistant 消耗）。
    let mut msg_seq = 0usize;
    // 逐条目布局（与条目缓存同源，design D2——保证缓存路径逐字节等价）。
    for entry in state.entries() {
        let (el, em) = layout_entry(
            entry,
            flags,
            overrides,
            thinking_overrides,
            &mut thinking_seq,
            &mut msg_seq,
            theme,
            None,
        );
        lines.extend(el);
        meta.extend(em);
    }
    // 流式中尚未落盘的内容。
    let (il, im) = layout_inflight(
        state,
        flags,
        overrides,
        thinking_overrides,
        &mut thinking_seq,
        theme,
        None,
    );
    lines.extend(il);
    meta.extend(im);
    // 运行中的用户 bash（分片实时增长）。
    let (bl, bm) = layout_bash_tail(state);
    lines.extend(bl);
    meta.extend(bm);
    meta.resize(lines.len(), Vec::new());
    debug_assert_eq!(lines.len(), meta.len(), "meta 必须与行等长");
    (lines, meta)
}

/// 单条目布局（oracle 与条目缓存共用的唯一实现，design D2）。
///
/// 输入: entry — transcript 条目；flags/overrides/thinking_overrides/theme —
///       同 [`chat_lines_themed`]；thinking_seq/msg_seq — 全局序号游标
///       （入口值 = 该条目基值，函数内推进，出口 = 下一条目基值）；
///       previews — edit 预览上下文（task3.3；None=无预览）
/// 输出: `(该条目的逻辑行, 逐行命中 meta)`——两者等长，命中种属携带
///       **全局**序号（以入口游标为基渲染）；错误情形：无
#[allow(clippy::too_many_arguments)] // previews 穿参（task3.3）+ append_blocks 同款
pub(crate) fn layout_entry(
    entry: &Entry,
    flags: RenderFlags,
    overrides: &std::collections::HashMap<String, bool>,
    thinking_overrides: &std::collections::HashMap<usize, bool>,
    thinking_seq: &mut usize,
    msg_seq: &mut usize,
    theme: &TuiTheme,
    previews: Option<&PreviewCtx<'_>>,
) -> (Vec<Line<'static>>, Vec<Vec<HitKind>>) {
    let mut lines = Vec::new();
    let mut meta: Vec<Vec<HitKind>> = Vec::new();
    let is_message = matches!(entry, Entry::User { .. } | Entry::Assistant { .. });
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
                thinking_seq,
                theme,
                previews,
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
    // 非卡片路径补齐空标记组（与行等长；append_blocks 已自同步）。
    meta.resize(lines.len(), Vec::new());
    // 语义消息起点回填首逻辑行（design D3/D1）：push 与内容标记
    // 共存不覆盖；通知/`!`bash 不标不耗 seq；空条目（无行）不标；
    // seq 仅标记时递增（transcript append-only → 序号稳定）。
    if is_message && !lines.is_empty() {
        meta[0].push(HitKind::MessageStart(*msg_seq));
        *msg_seq += 1;
    }
    (lines, meta)
}

/// 流式 in-flight 尾槽布局（design D1 易失槽；与 oracle 同源）。
///
/// 输入: state — 会话状态（读 `is_in_flight`/`in_flight`）；其余同
///       [`layout_entry`]；thinking_seq — 全局游标（入口 = 已有条目合计，
///       出口 = 含本槽合计）
/// 输出: `(行, meta)`——两者等长；无进行中流式时皆为空
pub(crate) fn layout_inflight(
    state: &SessionState,
    flags: RenderFlags,
    overrides: &std::collections::HashMap<String, bool>,
    thinking_overrides: &std::collections::HashMap<usize, bool>,
    thinking_seq: &mut usize,
    theme: &TuiTheme,
    previews: Option<&PreviewCtx<'_>>,
) -> (Vec<Line<'static>>, Vec<Vec<HitKind>>) {
    let mut lines = Vec::new();
    let mut meta: Vec<Vec<HitKind>> = Vec::new();
    if state.is_in_flight() {
        append_blocks(
            &mut lines,
            &mut meta,
            state.in_flight(),
            flags,
            overrides,
            thinking_overrides,
            thinking_seq,
            theme,
            previews,
        );
    }
    meta.resize(lines.len(), Vec::new());
    (lines, meta)
}

/// 运行中 bash 尾槽布局（design D1 易失槽；与 oracle 同源）。
///
/// 输入: state — 会话状态（读 `bash_running`/`bash_command`/`bash_output`/
///       `bash_excluded`）
/// 输出: `(行, meta)`——两者等长；bash 未运行时皆为空；输出行数上限与
///       [`append_bash`] 一致（500 行）
pub(crate) fn layout_bash_tail(
    state: &SessionState,
) -> (Vec<Line<'static>>, Vec<Vec<HitKind>>) {
    let mut lines = Vec::new();
    let mut meta: Vec<Vec<HitKind>> = Vec::new();
    if state.bash_running {
        let excluded = state.bash_excluded;
        let command = state.bash_command.clone();
        let output = state.bash_output.clone();
        append_bash(&mut lines, &command, &output, None, false, false, None, excluded, true);
    }
    meta.resize(lines.len(), Vec::new());
    (lines, meta)
}

/// bash 输出渲染行数上限（超出保留末尾并提示；pi 侧另有整体截断策略）。
const BASH_OUTPUT_MAX_LINES: usize = 500;

/// 追加一条 bash 渲染：命令行 + 输出（弱化、行数上限）+ 状态行。
/// TUI 薄适配——语义在 [`bash_entry_model`]（design D1）。
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
    for row in bash_entry_model(
        command,
        output,
        exit_code,
        cancelled,
        truncated,
        full_output_path,
        excluded,
        live,
    ) {
        out.push(crate::app::tools::segs_to_line(&row));
    }
}

/// bash 条目中性模型（design D1）：命令行 + 输出（尾部截断）+ 状态行。
///
/// 输入: command/output — 命令与输出；exit_code/cancelled/truncated/
///       full_output_path — 结果态；excluded — `!!` 不进上下文；
///       live — 运行中（状态行替换为运行标记）
/// 输出: 行序列（每行 = 段序列）——命令行（黄粗 + 可选不进上下文标记）、
///       截断标记行、输出行（仅保留末尾 `BASH_OUTPUT_MAX_LINES` 行、
///       跳过结尾换行空元素）、状态行（运行中/取消/exit/done + 可选
///       truncated 后缀）；不 panic
#[allow(clippy::too_many_arguments)]
pub(crate) fn bash_entry_model(
    command: &str,
    output: &str,
    exit_code: Option<i64>,
    cancelled: bool,
    truncated: bool,
    full_output_path: Option<&str>,
    excluded: bool,
    live: bool,
) -> Vec<Vec<crate::app::tools::Seg>> {
    use crate::app::tools::{Seg, SegRole};
    let mut rows: Vec<Vec<Seg>> = Vec::new();

    let mut cmd = vec![Seg::new(format!("$ {command}"), SegRole::Cmd)];
    if excluded {
        cmd.push(Seg::new("（不进上下文）", SegRole::Dim));
    }
    rows.push(cmd);

    // 输出行（超上限保留末尾）
    let all: Vec<&str> = output.split('\n').collect();
    let total = all.len();
    let start = total.saturating_sub(BASH_OUTPUT_MAX_LINES);
    if start > 0 {
        rows.push(vec![Seg::new(
            format!("…（已截断 {start} 行）"),
            SegRole::Dim,
        )]);
    }
    for (i, row) in all[start..].iter().enumerate() {
        if row.is_empty() && start + i == total - 1 {
            continue; // 仅跳过结尾换行产生的最后一个空元素（保留中间空行）
        }
        rows.push(vec![Seg::new((*row).to_string(), SegRole::Dim)]);
    }

    // 状态行
    let status = if live {
        Seg::new("⟳ 运行中…（esc 中止）", SegRole::Warn)
    } else if cancelled {
        Seg::new("cancelled", SegRole::Warn)
    } else {
        match exit_code {
            Some(0) => Seg::new("exit 0", SegRole::ExitOk),
            Some(code) => Seg::new(format!("exit {code}"), SegRole::ExitBad),
            None => Seg::new("done", SegRole::Dim),
        }
    };
    let mut status_row = vec![status];
    if truncated {
        status_row.push(Seg::new(
            match full_output_path {
                Some(p) => format!(" · truncated → {p}"),
                None => " · truncated".to_string(),
            },
            SegRole::Warn,
        ));
    }
    rows.push(status_row);
    rows
}

/// thinking 块可见行中性模型（design D1；空块 → None 不占行）。
///
/// 输入: text — thinking 正文；expanded — 展开态（折叠为占位行）
/// 输出: None = 空块（不渲染任何行）；Some(rows) = 折叠单行占位
///       `<thinking N chars>` 或展开的逐行正文（均暗灰斜体）
pub(crate) fn thinking_visible(
    text: &str,
    expanded: bool,
) -> Option<Vec<Vec<crate::app::tools::Seg>>> {
    use crate::app::tools::{Seg, SegRole};
    if text.is_empty() {
        return None;
    }
    if expanded {
        Some(
            text.split('\n')
                .map(|line| vec![Seg::new(line.to_string(), SegRole::ThinkingText)])
                .collect(),
        )
    } else {
        let n = text.chars().count();
        Some(vec![vec![Seg::new(
            format!("<thinking {n} chars>"),
            SegRole::ThinkingText,
        )]])
    }
}

/// 为一段内容块分配 thinking 全局序号（design D2 规则的共享实现，
/// TUI 命中表与 GPUI 单块覆盖表同源消费）。
///
/// 输入: blocks — 一段内容块序列；base — 入口全局序号游标（上一段出口）
/// 输出: `(标记, 出口游标)`——标记与 blocks 等长，仅 Thinking 块为
///       Some(seq)（**含空块**——空块也消耗序号，后接 delta 变非空时
///       既有序号不重排），其余块为 None；出口 = 下一段的入口基值
pub(crate) fn thinking_seqs(blocks: &[Block], base: usize) -> (Vec<Option<usize>>, usize) {
    let mut marks = Vec::with_capacity(blocks.len());
    let mut next = base;
    for block in blocks {
        if matches!(block, Block::Thinking { .. }) {
            marks.push(Some(next));
            next += 1;
        } else {
            marks.push(None);
        }
    }
    (marks, next)
}

/// 按通知类型生成样式行：error 红色加粗、retry 黄色、其余弱化斜体灰。
///
/// 输入: kind — 通知类别（"error"/"retry"/其它）；text — 通知正文
/// 输出: 单行样式化通知
fn notice_line(kind: &str, text: &str) -> Line<'static> {
    crate::app::tools::segs_to_line(&notice_model(kind, text))
}

/// 通知行中性模型（design D1；`[kind]` 前缀并入文本，单一语义段）。
///
/// 输入: kind — 通知类别（"error" → 红粗、"retry" → 黄、其余 → 暗灰斜体）；
///       text — 通知正文
/// 输出: 单行段序列（`[kind] text`）
pub(crate) fn notice_model(kind: &str, text: &str) -> Vec<crate::app::tools::Seg> {
    use crate::app::tools::{Seg, SegRole};
    let role = match kind {
        "error" => SegRole::NoticeError,
        "retry" => SegRole::NoticeRetry,
        _ => SegRole::Notice,
    };
    vec![Seg::new(format!("[{kind}] {text}"), role)]
}

/// 把一组内容块追加到输出行（markdown 文本/thinking 两态/工具卡片）。
///
/// 输入: out/meta — 行与逐行命中标识收集器（design D1）；blocks — 内容块；
///       flags — 渲染偏好；overrides — 单卡工具覆盖；thinking_overrides —
///       thinking 单块覆盖；thinking_seq — 全局 thinking 序号游标
///       （无条件递增、跨调用穿透，design D2）；previews — edit 预览
///       上下文（task3.3；None=无预览，测试/oracle 路径）
/// 输出: 无返回值；副作用为向 out/meta 追加（流式尾槽分槽构建也走
///       本入口，与全量 oracle 同源，design D1）
#[allow(clippy::too_many_arguments)]
pub(crate) fn append_blocks(
    out: &mut Vec<Line<'static>>,
    meta: &mut Vec<Vec<HitKind>>,
    blocks: &[Block],
    flags: RenderFlags,
    overrides: &std::collections::HashMap<String, bool>,
    thinking_overrides: &std::collections::HashMap<usize, bool>,
    thinking_seq: &mut usize,
    theme: &TuiTheme,
    previews: Option<&PreviewCtx<'_>>,
) {
    // 序号分配先行（共享规则 thinking_seqs，design D2：无条件递增、
    // 含空块、跨调用穿透——出口游标回写由本函数完成）。
    let (seqs, exit_seq) = thinking_seqs(blocks, *thinking_seq);
    *thinking_seq = exit_seq;
    for (idx, block) in blocks.iter().enumerate() {
        let start = out.len();
        // 本块命中标识（None = 不入命中表，design D1）。
        let mut mark: Option<HitKind> = None;
        match block {
            Block::Text { text } => {
                if !text.is_empty() {
                    // assistant 文本按 markdown 渲染（流式未闭合语法安全降级）。
                    out.extend(crate::app::markdown::render_markdown_themed(text, theme));
                }
            }
            Block::Thinking { text } => {
                let seq = seqs[idx].expect("Thinking 块必有序号");
                // 生效值：单块覆盖优先、否则全局（design D3，与工具 D1 同构）。
                let expanded = flags.show_thinking
                    || thinking_overrides.get(&seq).copied().unwrap_or(false);
                // 语义在 thinking_visible（空块 → None 不占行，design D1）。
                let Some(rows) = thinking_visible(text, expanded) else {
                    continue;
                };
                mark = Some(HitKind::Thinking(seq));
                for row in &rows {
                    out.push(crate::app::tools::segs_to_line(row));
                }
            }
            Block::ToolCall { id, name, args_json, tool } => {
                // 工具调用渲染为结构化卡片（展开态 = 单卡覆盖优先、否则
                // 跟随全局 flags.expand_tools，design D1）；背景面板按状态
                // 着色（design D5），状态文字/圆点保留（spec #16）。
                let expanded = *overrides.get(id).unwrap_or(&flags.expand_tools);
                // 预览态（task3.3）：Error → 红底头行（spec）；Ready → 卡体
                // 换预览行（edit_body 内优先级：结果 diff 仍最高）。
                let preview = previews.and_then(|p| p.map.get(id));
                let bg = if matches!(preview, Some(EditPreviewState::Error(_))) {
                    theme.tool_error_bg
                } else {
                    tool_panel_bg(tool, theme)
                };
                for mut line in crate::app::tools::tool_lines_themed_ctx(
                    name,
                    args_json,
                    tool,
                    expanded,
                    theme,
                    preview,
                ) {
                    line.style = Style::default().bg(bg);
                    out.push(line);
                }
                // 门控前移到建表：仅已有最终结果的卡入表（运行中不入，D1）。
                if tool.result.is_some() {
                    mark = Some(HitKind::Tool(id.clone()));
                }
            }
        }
        // 逐块同步 meta（design D1/D2）：命中块的全部行整段 push 内容
        // 标记（门控前移到建表，分发无需回查 state）。
        meta.resize(out.len(), Vec::new());
        if let Some(kind) = mark {
            for m in &mut meta[start..] {
                m.push(kind.clone());
            }
        }
    }
}

/// 工具卡片背景面板色（design D5）：由 pi dark.json 的 okhsl 转 RGB——
/// `toolPendingBg` okhsl(229 5% 24%) 运行灰、`toolSuccessBg` okhsl(158 46% 25%)
/// 成功墨绿、`toolErrorBg` okhsl(19 54% 25%) 失败暗红（转换经 pi oklab.ts
/// 权威算法核算）；宿主注入配色时以注入值为准（宿主配色注入 design D1）。
///
/// 输入: tool — 执行状态（读 status/is_error）；theme — 注入配色
///       （默认深色调为上述三色）
/// 输出: 面板背景色（spec #16：背景叠加、不替代状态文字）
fn tool_panel_bg(tool: &ToolExecution, theme: &TuiTheme) -> Color {
    match tool.status {
        ToolStatus::Pending | ToolStatus::Running => theme.tool_running_bg,
        ToolStatus::Ended if tool.is_error => theme.tool_error_bg,
        ToolStatus::Ended => theme.tool_success_bg,
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
        // 只看内容标记（首行同时携带 MessageStart——多标记共存，design D1）
        let marks: Vec<HitKind> = meta
            .iter()
            .flatten()
            .filter(|&k| matches!(k, HitKind::Thinking(_)))
            .cloned()
            .collect();
        assert_eq!(
            marks,
            vec![HitKind::Thinking(1), HitKind::Thinking(2)],
            "空块（序号 0）不打标、非空块整段打标"
        );
        // assistant 首个 thinking 行与 MessageStart 共存（不覆盖）
        let first_thinking = meta
            .iter()
            .find(|v| v.contains(&HitKind::Thinking(1)))
            .expect("Thinking(1) 行存在");
        assert!(
            first_thinking
                .iter()
                .any(|k| matches!(k, HitKind::MessageStart(_))),
            "首行多标记共存: {first_thinking:?}"
        );

        // 两次渲染产出完全一致（序号稳定，design D2）
        let (lines2, meta2) = chat_lines(&s, RenderFlags::default(), &empty, &empty_t);
        assert_eq!(lines, lines2, "行产出确定性");
        assert_eq!(meta, meta2, "meta 产出确定性");
    }

    /// 任务 1.1：语义消息起点打标（add-prompt-jump design D3）——用户/
    /// assistant 首逻辑行标 MessageStart(seq) 且 seq 递增；通知/`!`bash
    /// 不标不耗 seq；meta 与行等长。
    #[test]
    fn message_starts_marked_on_user_and_assistant_only() {
        let mut s = SessionState::new();
        s.transcript.push(Entry::User { text: "q1".into() });
        s.transcript.push(Entry::Assistant {
            turn: 1,
            blocks: vec![Block::Text { text: "a1".into() }],
            usage: None,
        });
        s.transcript.push(Entry::Notice {
            kind: "info".into(),
            text: "n-note".into(),
        });
        s.transcript.push(Entry::Assistant {
            turn: 2,
            blocks: vec![Block::Text { text: "a2".into() }],
            usage: None,
        });
        s.transcript.push(Entry::Bash {
            command: "!ls".into(),
            output: String::new(),
            exit_code: Some(0),
            cancelled: false,
            truncated: false,
            full_output_path: None,
            excluded: false,
        });

        let empty = std::collections::HashMap::new();
        let empty_t = std::collections::HashMap::new();
        let (lines, meta) = chat_lines(&s, RenderFlags::default(), &empty, &empty_t);
        assert_eq!(lines.len(), meta.len(), "meta 必须与行等长");

        let marks: Vec<(usize, HitKind)> = meta
            .iter()
            .enumerate()
            .flat_map(|(i, v)| v.iter().map(move |k| (i, k.clone())))
            .collect();
        assert_eq!(marks.len(), 3, "恰三个消息起点（通知/bash 不标）: {marks:?}");
        assert_eq!(marks[0], (0, HitKind::MessageStart(0)), "用户首行 seq0");
        assert!(matches!(marks[1].1, HitKind::MessageStart(1)), "a1 seq1");
        assert!(matches!(marks[2].1, HitKind::MessageStart(2)), "a2 seq2");

        // 打标行确实落在对应条目内容上
        let text_of = |i: usize| -> String {
            lines[i].spans.iter().map(|s| s.content.as_ref()).collect()
        };
        assert!(text_of(marks[0].0).contains("q1"), "seq0 在用户行");
        assert!(text_of(marks[1].0).contains("a1"), "seq1 在 a1 行");
        assert!(text_of(marks[2].0).contains("a2"), "seq2 在 a2 行");

        // 通知行 None
        let notice_row = lines
            .iter()
            .position(|l| l.spans.iter().any(|s| s.content.as_ref().contains("n-note")))
            .expect("通知行存在");
        assert!(meta[notice_row].is_empty(), "通知行不打标");
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

        // 只看工具类标记（用户/assistant 首行可携带 MessageStart，design D1）
        let marked: Vec<HitKind> = meta
            .iter()
            .flatten()
            .filter(|&k| matches!(k, HitKind::Tool(_)))
            .cloned()
            .collect();
        assert!(!marked.is_empty(), "结果卡 c1 应入表");
        assert!(
            marked.iter().all(|m| *m == HitKind::Tool("c1".into())),
            "只标 c1——运行中 c2 与普通行不入工具表: {marked:?}"
        );

        // `!` bash 行明确无标记
        let bash_idx = lines
            .iter()
            .position(|l| l.spans.iter().any(|sp| sp.content.as_ref().contains("echo hi")))
            .expect("bash 行应存在");
        assert!(meta[bash_idx].is_empty(), "`!` bash 行不入表");
        // 用户行不入工具表（可携带 MessageStart，design D1）
        assert!(
            !meta[0].iter().any(|k| matches!(k, HitKind::Tool(_))),
            "用户行不入工具表"
        );
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

    /// bash 命令行必须黄粗（回归防护：task1.2 段化重构曾把 `$ cmd` 的
    /// 黄色丢成纯粗体；HEAD 与 spec「命令行黄色加粗」均为黄+粗）。
    ///
    /// 功能: 仅含落盘 bash 条目的会话 → chat_lines_themed → 断言首行
    ///       `$ cmd` 前景色 Yellow + BOLD；
    /// 输入: 无（纯函数）；
    /// 输出: 断言通过。
    #[test]
    fn bash_command_line_is_yellow_bold() {
        let mut st = SessionState::new();
        st.transcript.push(Entry::Bash {
            command: "echo hi".into(),
            output: "hi\n".into(),
            exit_code: Some(0),
            cancelled: false,
            truncated: false,
            full_output_path: None,
            excluded: false,
        });
        let empty_s: std::collections::HashMap<String, bool> = Default::default();
        let empty_t: std::collections::HashMap<usize, bool> = Default::default();
        let (lines, _) = chat_lines_themed(
            &st,
            RenderFlags::default(),
            &empty_s,
            &empty_t,
            &TuiTheme::default(),
        );
        let spans = &lines[0].spans;
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.starts_with("$ echo hi"), "首行应为命令行: {text}");
        assert_eq!(spans[0].style.fg, Some(Color::Yellow), "命令行应为黄色");
        assert!(
            spans[0]
                .style
                .add_modifier
                .contains(ratatui::style::Modifier::BOLD),
            "命令行应粗体"
        );
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

    /// 中性通知模型（task 1.3）：`[kind]` 前缀并入、角色按 kind 分派。
    #[test]
    fn notice_model_prefix_and_roles() {
        use crate::app::tools::SegRole;
        // 3.3 预览错误态 → 红底头行（task3.3；spec: 错误态红底）。
        {
            use std::collections::HashMap;
            let theme = TuiTheme::default();
            let block = Block::ToolCall {
                id: "prev-err".into(),
                name: "edit".into(),
                args_json: r#"{"file_path":"/x","oldText":"a","newText":"b"}"#.into(),
                tool: ToolExecution {
                    status: ToolStatus::Running,
                    ..Default::default()
                },
            };
            let mut map = HashMap::new();
            map.insert(
                "prev-err".to_string(),
                EditPreviewState::Error("boom".into()),
            );
            let ctx = PreviewCtx {
                map: &map,
                rev: 1,
            };
            let mut out = Vec::new();
            let mut meta = Vec::new();
            append_blocks(
                &mut out,
                &mut meta,
                std::slice::from_ref(&block),
                RenderFlags::default(),
                &HashMap::new(),
                &HashMap::new(),
                &mut 0usize,
                &theme,
                Some(&ctx),
            );
            let text = out
                .iter()
                .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n");
            assert!(text.contains("boom"), "错误文案在场: {text}");
            assert!(
                out.iter().all(|l| l.style.bg == Some(theme.tool_error_bg)),
                "错误态红底头行: {:?}",
                out.iter().map(|l| l.style.bg).collect::<Vec<_>>()
            );
        }

        let err = notice_model("error", "400 bad");
        assert_eq!(err.len(), 1);
        assert_eq!(err[0].text, "[error] 400 bad");
        assert_eq!(err[0].role, SegRole::NoticeError);

        let retry = notice_model("retry", "重试 1/3");
        assert_eq!(retry[0].text, "[retry] 重试 1/3");
        assert_eq!(retry[0].role, SegRole::NoticeRetry);

        let info = notice_model("info", "note");
        assert_eq!(info[0].text, "[info] note");
        assert_eq!(info[0].role, SegRole::Notice);
    }

    /// 中性 bash 模型（task 1.3）：600 行 → 尾 500 + 截断标记；状态行四态。
    #[test]
    fn bash_entry_model_tail_cap_and_status_variants() {
        use crate::app::tools::SegRole;
        let big: String = (0..600).map(|i| format!("row{i}\n")).collect();
        let rows = bash_entry_model("big", &big, Some(0), false, false, None, false, false);
        // 命令行 + 截断标记 + 500 输出行 + 状态行
        let joined: String = rows
            .iter()
            .map(|r| r.iter().map(|s| s.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("…（已截断 101 行）"), "截断标记: {joined}");
        assert!(joined.contains("row599"), "保留末尾");
        assert!(!joined.contains("row0\n"), "丢弃开头");
        let out_rows = rows.len() - 1; // 去掉命令行
        assert!(out_rows <= 1 + 1 + 500, "输出不超限: {out_rows}");

        // 状态行四态
        let st = |rows: &Vec<Vec<crate::app::tools::Seg>>| rows.last().unwrap().clone();
        assert_eq!(st(&rows)[0].text, "exit 0");
        assert_eq!(st(&rows)[0].role, SegRole::ExitOk);

        let cancelled = bash_entry_model("x", "", None, true, false, None, false, false);
        assert_eq!(st(&cancelled)[0].text, "cancelled");
        assert_eq!(st(&cancelled)[0].role, SegRole::Warn);

        let live = bash_entry_model("x", "", None, false, false, None, false, true);
        assert!(st(&live)[0].text.contains("运行中"), "运行中文案");
        assert_eq!(st(&live)[0].role, SegRole::Warn);

        let failed = bash_entry_model("x", "", Some(127), false, false, None, false, false);
        assert_eq!(st(&failed)[0].text, "exit 127");
        assert_eq!(st(&failed)[0].role, SegRole::ExitBad);

        // truncated 后缀 + 不进上下文标记
        let tr = bash_entry_model("x", "", None, false, true, Some("/tmp/t.log"), true, false);
        assert_eq!(st(&tr)[1].text, " · truncated → /tmp/t.log");
        assert_eq!(tr[0][1].text, "（不进上下文）");
    }

    /// 中性 thinking 模型（task 1.3）：空块 → None；两态文案与行数。
    #[test]
    fn thinking_visible_empty_none_and_two_states() {
        use crate::app::tools::SegRole;
        assert!(thinking_visible("", false).is_none(), "折叠空块不占行");
        assert!(thinking_visible("", true).is_none(), "展开空块不占行");

        let collapsed = thinking_visible("secret thoughts", false).unwrap();
        assert_eq!(collapsed.len(), 1, "折叠单行占位");
        assert_eq!(collapsed[0][0].text, "<thinking 15 chars>");
        assert_eq!(collapsed[0][0].role, SegRole::ThinkingText);

        let expanded = thinking_visible("l1\nl2", true).unwrap();
        assert_eq!(expanded.len(), 2, "展开逐行");
        assert_eq!(expanded[0][0].text, "l1");
        assert_eq!(expanded[1][0].text, "l2");
        assert_eq!(expanded[0][0].role, SegRole::ThinkingText);
    }

    /// seq 共享规则（task 1.5）：空块计数、非 thinking 不耗序、
    /// 跨段（跨条目）递增——出口游标作下一段基值。
    #[test]
    fn thinking_seqs_counts_empty_and_chains_across_segments() {
        // 段 1：空 thinking（计数不打标）+ 非空 + Text（不耗序）+ 非空
        let seg1 = vec![
            Block::Thinking { text: String::new() }, // seq 0
            Block::Thinking { text: "a".into() },    // seq 1
            Block::Text { text: "t".into() },        // 不耗序
            Block::Thinking { text: "b".into() },    // seq 2
        ];
        let (marks1, exit1) = thinking_seqs(&seg1, 0);
        assert_eq!(marks1, vec![Some(0), Some(1), None, Some(2)]);
        assert_eq!(exit1, 3, "出口 = 下段基值（含空块消耗）");

        // 段 2（跨条目）：从段 1 出口继续
        let seg2 = vec![
            Block::Text { text: "x".into() },
            Block::Thinking { text: "c".into() }, // seq 3
        ];
        let (marks2, exit2) = thinking_seqs(&seg2, exit1);
        assert_eq!(marks2, vec![None, Some(3)]);
        assert_eq!(exit2, 4, "跨段递增");
    }

    /// seq 双端同源（task 1.5）：同 fixture 下共享规则分配的序号 ==
    /// TUI 命中表 `HitKind::Thinking(seq)` 序列（GPUI 单块覆盖表消费
    /// 同一 `thinking_seqs`，两端序号必然一致）。
    #[test]
    fn thinking_seqs_match_tui_marks_on_fixture() {
        use pi_shell_rs::session::replay_file;
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/probe/text_only/snapshot/transcript.jsonl");
        let state = replay_file(&path).expect("replay fixture");

        // 共享规则端：按 transcript 顺序逐条目分配（跨条目穿透）
        let mut base = 0usize;
        let mut rule: Vec<usize> = Vec::new();
        for entry in state.entries() {
            if let Entry::Assistant { blocks, .. } = entry {
                let (marks, exit) = thinking_seqs(blocks, base);
                base = exit;
                // 与打标口径一致：仅非空 thinking 打标（空块无行无标）
                for (b, m) in blocks.iter().zip(&marks) {
                    if let (Block::Thinking { text }, Some(seq)) = (b, m)
                        && !text.is_empty()
                    {
                        rule.push(*seq);
                    }
                }
            }
        }
        assert!(!rule.is_empty(), "fixture 应含非空 thinking 块");

        // TUI 端：chat_lines 命中表
        let empty = std::collections::HashMap::new();
        let empty_t = std::collections::HashMap::new();
        let (_, meta) = chat_lines(&state, RenderFlags::default(), &empty, &empty_t);
        let tui: Vec<usize> = meta
            .iter()
            .flatten()
            .filter_map(|k| match k {
                HitKind::Thinking(seq) => Some(*seq),
                _ => None,
            })
            .collect();

        assert_eq!(rule, tui, "双端同 fixture 同序号");
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

    /// 宿主配色注入：`chat_lines_themed` 传非默认 `TuiTheme` 时三态面板
    /// 背景等于注入值、状态文字仍可见；传默认时等于现值（design D1）。
    ///
    /// 输入: 无（构造三态工具卡会话）
    /// 输出: 断言通过；注入值/默认值分别与面板 bg 比对
    #[test]
    fn chat_lines_themed_uses_injected_panel_colors() {
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
        s.handle(&json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        s.transcript.push(Entry::Assistant {
            turn: 1,
            blocks: vec![
                mk(ToolStatus::Running, false),
                mk(ToolStatus::Ended, false),
                mk(ToolStatus::Ended, true),
            ],
            usage: None,
        });

        // 注入非默认配色：三态面板 bg 必须等于注入值
        let injected = TuiTheme {
            tool_running_bg: Color::Rgb(1, 2, 3),
            tool_success_bg: Color::Rgb(4, 5, 6),
            tool_error_bg: Color::Rgb(7, 8, 9),
            ..TuiTheme::default()
        };
        let expect = [
            injected.tool_running_bg,
            injected.tool_success_bg,
            injected.tool_error_bg,
        ];
        let status_texts = ["running", "done", "error"];
        let (lines, _) = chat_lines_themed(
            &s,
            RenderFlags::default(),
            &std::collections::HashMap::new(),
            &std::collections::HashMap::new(),
            &injected,
        );
        for (i, (&bg, status)) in expect.iter().zip(status_texts.iter()).enumerate() {
            let start = lines
                .iter()
                .position(|l| {
                    l.spans.iter().any(|sp| sp.content.as_ref() == "bash")
                        && l.spans.iter().any(|sp| sp.content.as_ref().contains(status))
                })
                .unwrap_or_else(|| panic!("卡 {i} 头行未找到"));
            assert_eq!(
                lines[start].style.bg,
                Some(bg),
                "卡 {i} 应取注入面板色"
            );
            // 状态文字仍可见（背景不替代文字）
            let head_text: String = lines[start]
                .spans
                .iter()
                .map(|sp| sp.content.as_ref())
                .collect();
            assert!(head_text.contains(status), "状态文字保留: {head_text}");
        }

        // 传默认 theme = 现值（与存量精确 RGB 断言一致）
        let (default_lines, _) = chat_lines_themed(
            &s,
            RenderFlags::default(),
            &std::collections::HashMap::new(),
            &std::collections::HashMap::new(),
            &TuiTheme::default(),
        );
        let default_expect = [
            Color::Rgb(52, 56, 58),
            Color::Rgb(37, 65, 49),
            Color::Rgb(91, 40, 42),
        ];
        for (i, (&bg, status)) in default_expect.iter().zip(status_texts.iter()).enumerate() {
            let start = default_lines
                .iter()
                .position(|l| {
                    l.spans.iter().any(|sp| sp.content.as_ref() == "bash")
                        && l.spans.iter().any(|sp| sp.content.as_ref().contains(status))
                })
                .unwrap_or_else(|| panic!("卡 {i} 头行未找到"));
            assert_eq!(
                default_lines[start].style.bg,
                Some(bg),
                "默认 theme 应等于现值"
            );
        }
    }
}
