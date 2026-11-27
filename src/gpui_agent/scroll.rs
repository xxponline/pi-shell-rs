//! 消息区滚动的条目级命令实现（design D1/D2：滚动真相归 `MessageScrollerState`，
//! `App.scroll` 保持 TUI 专属）。
//!
//! 本模块承载 task 1.1/1.2 spike 的纯逻辑与 task 3.x 的目标条目计算：
//! 条目高度内容估算（S1 页语义）——误差源只有 user/assistant 文本的
//! markdown/TextView 折行，卡片/思考/bash/通知行均为固定网格行高。

use crate::app::theme::TuiTheme;
use crate::app::{
    App, bash_entry_model, notice_model, thinking_seqs, thinking_visible,
    tool_card_model_ctx,
};
use crate::session::{Block, Entry};

/// 文本平均字符推进宽（px）。经验常数：spike 探针 D1/D2/D3/WRAP/E1/C00
/// 实测折行容量在 760px 文本宽下收敛于 64 字符（token 59-68、带空格
/// ≥67），取 760/64 = 11.875；随窗口宽度线性换算容量（容量 = text_w/
/// 本常数）。全宽字符内容（连续 w）与带空格内容各有 ±1 行偏差（已知差异）。
pub(crate) const CHAR_ADVANCE_PX: f32 = 760.0 / 64.0;

/// 文本区左右内边距合计（px）。spike 标定：视口 800 → 正文容器 760
/// （含条目 `px_2` 与滚动容器内衬；浅色主题固定，theme 变更时重标）。
const TEXT_PAD_X: f32 = 40.0;

impl ScrollMetrics {
    /// 生产度量（浅色主题固定字体；常数来自 spike 标定，见 tasks 1.1 结论）。
    ///
    /// 功能: 从实时视口构造滚动估算度量；
    /// 输入: viewport_w — 消息区视口宽（观测 bounds）；grid_line — 网格行高；
    /// 输出: `text_w = 视口宽 − TEXT_PAD_X`；text_line/user_folded/base_pad 为
    ///       主题钉死常数（26 / 62.5 / 40，标定于 spike：条目 88.5 = 40+22.5+26）。
    pub(crate) fn production(viewport_w: f32, grid_line: f32) -> Self {
        Self {
            grid_line,
            text_line: 26.0,
            user_folded: 62.5,
            base_pad: 40.0,
            text_w: (viewport_w - TEXT_PAD_X).max(100.0),
            char_w: CHAR_ADVANCE_PX,
        }
    }
}

/// 消息起点条目下标（镜像 TUI `msg_seq` 规则：User 恒是；Assistant 有任一
/// 可见内容行才是——空文本/空 thinking 不算；通知与 `!` bash 不是）。
///
/// 功能: 枚举可跳转的消息边界；
/// 输入: entries — 全部会话条目；
/// 输出: 升序的条目下标列表（可空）。
pub(crate) fn message_start_indices(entries: &[Entry]) -> Vec<usize> {
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| match e {
            Entry::User { .. } => true,
            Entry::Assistant { blocks, .. } => blocks.iter().any(|b| match b {
                Block::Text { text } => !text.is_empty(),
                Block::Thinking { text } => !text.is_empty(),
                Block::ToolCall { .. } => true,
            }),
            _ => false,
        })
        .map(|(i, _)| i)
        .collect()
}

/// prompt 跳转目标（镜像 TUI `prompt_jump_target`，条目粒度）。
///
/// 功能: 以当前视口顶 `(top_ix, top_offset_px)` 为基准找目标消息起点——
///       prev = 严格低于顶的最后一个起点（顶在消息中部 → 该消息起点），
///       无 → `Some(0)` 归顶；next = 严格高于顶的第一个起点，
///       无 → `None`（调用方贴底并恢复跟随）；
/// 输入: entries — 会话条目；top_ix/top_offset_px — `logical_scroll_top`；
///       dir — -1 上一条 / +1 下一条；
/// 输出: `Some(目标条目下标)` / `None`（仅 next 端点）。
pub(crate) fn jump_target(
    entries: &[Entry],
    top_ix: usize,
    top_offset_px: f32,
    dir: i8,
) -> Option<usize> {
    let starts = message_start_indices(entries);
    if dir < 0 {
        Some(
            starts
                .iter()
                .rev()
                .copied()
                .find(|&s| s < top_ix || (s == top_ix && top_offset_px > 0.0))
                .unwrap_or(0),
        )
    } else {
        starts.iter().copied().find(|&s| s > top_ix)
    }
}

/// 条目高度估算度量（px；由调用方从窗口实测校准后传入）。
///
/// 功能: 六参描述条目几何，供 [`estimated_entry_px`] 换算像素；
/// 输入: 无（纯数据）；
/// 输出: `grid_line` 固定网格行高、`text_line` 消息文本单行高、
///       `user_folded` user 条目正文外高度（内边距+标签行+间隙）、
///       `base_pad` 非 user 条目外层内边距合计（含工具卡自带）、
///       `text_w` 文本可折行宽度、`char_w` 文本平均字符宽（折行估算）。
#[derive(Clone, Copy, Debug)]
pub(crate) struct ScrollMetrics {
    /// 固定网格行高（卡片/思考/bash/通知等 `h(px(line_h))` 行）。
    pub(crate) grid_line: f32,
    /// 消息文本单视觉行高（TextView/markdown；折行为唯一系统误差源）。
    pub(crate) text_line: f32,
    /// user 条目正文外高度（`py_1` 上下 + 标签行 + 间隙）。
    pub(crate) user_folded: f32,
    /// 非 user 条目（assistant/notice/bash/工具卡）外层内边距合计。
    pub(crate) base_pad: f32,
    /// 文本可折行宽度（视口宽 − 左右内边距）。
    pub(crate) text_w: f32,
    /// 文本平均字符推进宽（[`CHAR_ADVANCE_PX`]；折行行数 = ceil(字数×本值/文本宽)）。
    pub(crate) char_w: f32,
}

/// markdown 段落折行行数估算（软换行拼接 + 按宽折行）。
///
/// 功能: 按空行分段（段内 `\n` 软换行拼空格，与内建 markdown 同语义），
///       每段 ceil(字符数×char_w/text_w) 行（至少 1 行）；
/// 输入: text — 原文；m — 度量；
/// 输出: 视觉行数估算（>0）。
pub(crate) fn wrapped_rows(text: &str, m: &ScrollMetrics) -> f32 {
    let mut rows = 0.0;
    for para in text.split("\n\n") {
        let joined: String = para.split('\n').collect::<Vec<_>>().join(" ");
        let chars = joined.chars().count() as f32;
        rows += (chars * m.char_w / m.text_w).ceil().max(1.0);
    }
    rows.max(1.0)
}

/// 估算单条目渲染高度（px）。
///
/// 功能: 按 `entry_element` 的实际 chrome 结构逐块累加——user = 折叠高度 +
///       段落折行行数×文本行高；assistant = 外层内边距 + 各块（段落折行 /
///       思考可见行×网格行高 / 工具卡模型行×网格行高+卡内边距）；
///       notice/bash = 内边距 + 模型行×网格行高；
/// 输入: entry — 会话条目；app — 共享状态（thinking/工具卡生效展开态查询）；
///       seq_base — 本条目 thinking 全局序号基值（与渲染侧前缀扫描同源）；
///       m — 度量六参；
/// 输出: 估算高度 px（非负；markdown 软换行与按宽折行为主要误差源）。
pub(crate) fn estimated_entry_px(
    entry: &Entry,
    app: &App,
    seq_base: usize,
    m: &ScrollMetrics,
) -> f32 {
    match entry {
        Entry::User { text } => m.user_folded + wrapped_rows(text, m) * m.text_line,
        Entry::Assistant { blocks, .. } => {
            let (seqs, _) = thinking_seqs(blocks, seq_base);
            let mut px = m.base_pad;
            for (bi, block) in blocks.iter().enumerate() {
                px += match block {
                    Block::Text { text } if !text.is_empty() => wrapped_rows(text, m) * m.text_line,
                    Block::Text { .. } => 0.0,
                    Block::Thinking { text } => {
                        let seq = seqs[bi].expect("Thinking 块必有序号");
                        let expanded = app.thinking_expanded_effective(seq);
                        thinking_visible(text, expanded)
                            .map(|rows| rows.len() as f32 * m.grid_line)
                            .unwrap_or(0.0)
                    }
                    Block::ToolCall {
                        id,
                        name,
                        args_json,
                        tool,
                    } => {
                        // 卡 chrome：自带 `py_1` + 头行 + 折叠体(+提示) / 展开体 + 受保护尾。
                        let expanded = app.tool_expanded_effective(id);
                        // 预览行计入高度估算（task3.3：与渲染同源，防滚动比例偏差）。
                        let pctx = app.preview_ctx();
                        let model = tool_card_model_ctx(
                            name,
                            args_json,
                            tool,
                            expanded,
                            &TuiTheme::default(),
                            pctx.map.get(id),
                        );
                        let rows = 1
                            + model.body.len()
                            + usize::from(model.hint.is_some())
                            + model.tail.len();
                        m.base_pad + rows as f32 * m.grid_line
                    }
                };
            }
            px
        }
        Entry::Notice { kind, text } => {
            m.base_pad + notice_model(kind, text).len() as f32 * m.grid_line
        }
        Entry::Bash {
            command,
            output,
            exit_code,
            cancelled,
            truncated,
            full_output_path,
            excluded,
        } => {
            let rows = bash_entry_model(
                command,
                output,
                *exit_code,
                *cancelled,
                *truncated,
                full_output_path.as_deref(),
                *excluded,
                false,
            )
            .len();
            m.base_pad + rows as f32 * m.grid_line
        }
    }
}

/// 条目 → thinking 全局序号基值前缀表（与渲染侧 seq_bases 扫描同源）。
///
/// 功能: 逐条目累积 thinking 序号出口值；
/// 输入: entries — 全部会话条目；
/// 输出: 与 entries 等长的基值表（第 i 项 = 第 i 条目的入口基值）。
fn seq_bases(entries: &[Entry]) -> Vec<usize> {
    let mut bases = Vec::with_capacity(entries.len());
    let mut cursor = 0usize;
    for entry in entries {
        bases.push(cursor);
        if let Entry::Assistant { blocks, .. } = entry {
            cursor = thinking_seqs(blocks, cursor).1;
        }
    }
    bases
}

/// 整页向下：取位移最接近 `page_px` 的条目顶落点（S1 内容估算步进）。
///
/// 功能: 从当前视口顶 `(from_ix, from_offset_px)` 出发，累计估算高度，
///       在候选落点中选 |位移 − page_px| 最小者（单调累计、过靶即停）；
/// 输入: entries/app — 内容与生效展开态；from_ix/from_offset_px — 当前
///       视口顶（条目下标 + 条目内偏移 px，`logical_scroll_top` 语义）；
///       page_px — 目标位移（正=向下）；m — 度量；
/// 输出: 落点条目下标（≥ from_ix；末条饱和不越界；内容不足一页时停在末条）。
pub(crate) fn page_down_target(
    entries: &[Entry],
    app: &App,
    from_ix: usize,
    from_offset_px: f32,
    page_px: f32,
    m: &ScrollMetrics,
) -> usize {
    let bases = seq_bases(entries);
    let from = from_ix.min(entries.len().saturating_sub(1));
    let target = page_px.max(0.0);
    let mut acc = -from_offset_px.max(0.0); // 候选 from 的位移（对齐条目顶 = 上移 offset）
    let mut best = from;
    let mut best_err = (acc - target).abs();
    let mut ix = from;
    while ix + 1 < entries.len() {
        acc += estimated_entry_px(&entries[ix], app, bases[ix], m);
        ix += 1;
        let err = (acc - target).abs();
        if err < best_err {
            best_err = err;
            best = ix;
        } else if acc > target {
            break; // 已过靶且更差：后续只会更差
        }
    }
    best
}

/// 整页向上：[`page_down_target`] 的镜像（位移为负，取最接近 −page_px 者）。
///
/// 功能: 从当前视口顶向上累计估算高度选落点；
/// 输入: 同 [`page_down_target`]，page_px 传正值（语义=向上一页）；
/// 输出: 落点条目下标（≤ from_ix；顶条饱和停在 0；offset 归零也算合法位移）。
pub(crate) fn page_up_target(
    entries: &[Entry],
    app: &App,
    from_ix: usize,
    from_offset_px: f32,
    page_px: f32,
    m: &ScrollMetrics,
) -> usize {
    let bases = seq_bases(entries);
    let from = from_ix.min(entries.len().saturating_sub(1));
    let target = -page_px.max(0.0);
    let mut acc = -from_offset_px.max(0.0); // 候选 from：向下为负位移语义的镜像起点
    let mut best = from;
    let mut best_err = (acc - target).abs();
    let mut ix = from;
    while ix > 0 {
        acc -= estimated_entry_px(&entries[ix - 1], app, bases[ix - 1], m);
        ix -= 1;
        let err = (acc - target).abs();
        if err < best_err {
            best_err = err;
            best = ix;
        } else if acc < target {
            break; // 已过靶（更负）且更差
        }
    }
    best
}

#[cfg(test)]
mod tests {
    //! 纯算术单测（无窗口）：估算公式与步进夹取。

    use super::*;
    use crate::session::{Block, ToolExecution, ToolStatus};

    /// 度量六参（测试固定值，量纲自洽即可）。
    fn m() -> ScrollMetrics {
        ScrollMetrics {
            grid_line: 10.0,
            text_line: 20.0,
            user_folded: 30.0,
            base_pad: 8.0,
            text_w: 1000.0,
            char_w: 10.0,
        }
    }

    /// user 条目 = 折叠高度 + 段落折行行数×文本行高（软换行拼接）。
    #[test]
    fn estimator_user_paragraph_wrap() {
        let app = App::new();
        // 5 字符 → 1 行（50/1000 ceil=1）。
        let e = Entry::User {
            text: "a\nb\nc".into(),
        };
        let px = estimated_entry_px(&e, &app, 0, &m());
        assert!((px - (30.0 + 20.0)).abs() < 1e-6, "实际 {px}");

        // 200 字符 → ceil(2000/1000)=2 行。
        let e = Entry::User {
            text: "x".repeat(200),
        };
        let px = estimated_entry_px(&e, &app, 0, &m());
        assert!((px - (30.0 + 2.0 * 20.0)).abs() < 1e-6, "折行 {px}");

        // 空行分段：两段各 1 行 → 2 行。
        let e = Entry::User {
            text: "hi\n\nthere".into(),
        };
        let px = estimated_entry_px(&e, &app, 0, &m());
        assert!((px - (30.0 + 2.0 * 20.0)).abs() < 1e-6, "分段 {px}");
    }

    /// 空 thinking 不占行；折叠占位 1 网格行；展开按可见行数。
    #[test]
    fn estimator_thinking_empty_zero_and_visible_rows() {
        let mut app = App::new();
        let mk = |text: &str| Block::Thinking {
            text: text.to_string(),
        };
        let empty = Entry::Assistant {
            turn: 1,
            blocks: vec![mk("")],
            usage: None,
        };
        let px = estimated_entry_px(&empty, &app, 0, &m());
        assert!((px - 8.0).abs() < 1e-6, "空块仅内边距: {px}");

        let one = Entry::Assistant {
            turn: 1,
            blocks: vec![mk("hi")],
            usage: None,
        };
        let px = estimated_entry_px(&one, &app, 0, &m());
        assert!((px - (8.0 + 10.0)).abs() < 1e-6, "折叠 1 行: {px}");

        // 单块覆盖翻转为展开（全局位私有，经共享切换入口）→ 两行正文两网格行。
        app.toggle_thinking_block(0);
        let two = Entry::Assistant {
            turn: 1,
            blocks: vec![mk("l1\nl2")],
            usage: None,
        };
        let px = estimated_entry_px(&two, &app, 0, &m());
        assert!((px - (8.0 + 2.0 * 10.0)).abs() < 1e-6, "展开 2 行: {px}");
    }

    /// 工具卡 = pad +（头 + 折叠体 + 提示? + 受保护尾）网格行。
    #[test]
    fn estimator_tool_card_rows_follow_model() {
        use crate::app::tools::tool_card_model;
        let app = App::new(); // 默认折叠
        let tool = ToolExecution {
            status: ToolStatus::Ended,
            is_error: false,
            result: Some(serde_json::json!({"content":[{"type":"text","text":"a\nb\nc"}]})),
            ..Default::default()
        };
        let block = Block::ToolCall {
            id: "c1".into(),
            name: "read".to_string(),
            args_json: r#"{"path":"/x"}"#.into(),
            tool,
        };
        let e = Entry::Assistant {
            turn: 1,
            blocks: vec![block],
            usage: None,
        };
        // 与渲染同源：模型行数 × 网格行 + 卡自带 pad + 条目 pad。
        let model = tool_card_model(
            "read",
            r#"{"path":"/x"}"#,
            &ToolExecution {
                status: ToolStatus::Ended,
                is_error: false,
                result: Some(serde_json::json!({"content":[{"type":"text","text":"a\nb\nc"}]})),
                ..Default::default()
            },
            false,
            &TuiTheme::default(),
        );
        let rows = 1 + model.body.len() + usize::from(model.hint.is_some()) + model.tail.len();
        let expect = 8.0 + 8.0 + rows as f32 * 10.0;
        let px = estimated_entry_px(&e, &app, 0, &m());
        assert!((px - expect).abs() < 1e-6, "卡 {px} vs {expect}");
    }

    /// bash 条目 = pad + bash_entry_model 行数 × 网格行。
    #[test]
    fn estimator_bash_uses_model_row_count() {
        let app = App::new();
        let out: String = (0..600).map(|i| format!("r{i}\n")).collect();
        let e = Entry::Bash {
            command: "seq".into(),
            output: out.clone(),
            exit_code: Some(0),
            cancelled: false,
            truncated: false,
            full_output_path: None,
            excluded: false,
        };
        let expect_rows =
            bash_entry_model("seq", &out, Some(0), false, false, None, false, false).len();
        let px = estimated_entry_px(&e, &app, 0, &m());
        assert!((px - (8.0 + expect_rows as f32 * 10.0)).abs() < 1e-6);
    }

    /// 消息起点规则（镜像 TUI msg_seq）：User 恒是；Assistant 有可见内容行
    /// 才是（空 thinking/空块不算）；通知与 `!` bash 不是。
    #[test]
    fn message_start_indices_follow_tui_msg_seq() {
        let entries = vec![
            Entry::User { text: String::new() },
            Entry::Notice {
                kind: "info".into(),
                text: "n".into(),
            },
            Entry::Assistant {
                turn: 1,
                blocks: vec![Block::Thinking {
                    text: String::new(),
                }],
                usage: None,
            },
            Entry::Assistant {
                turn: 1,
                blocks: vec![Block::Text { text: "hi".into() }],
                usage: None,
            },
            Entry::Bash {
                command: "ls".into(),
                output: String::new(),
                exit_code: Some(0),
                cancelled: false,
                truncated: false,
                full_output_path: None,
                excluded: false,
            },
            Entry::User { text: "q".into() },
        ];
        assert_eq!(message_start_indices(&entries), vec![0, 3, 5]);
    }

    /// 跳转向量（镜像 TUI `prompt_jump_target`）：prev 严格低于顶、
    /// 顶在消息中部取该消息起点、端点归顶/贴底（None）；next 跳过非消息条目。
    #[test]
    fn jump_target_vectors() {
        // starts = [0, 2, 3]（0 user / 1 notice / 2 assistant / 3 user）。
        let entries = vec![
            Entry::User { text: "a".into() },
            Entry::Notice {
                kind: "info".into(),
                text: "n".into(),
            },
            Entry::Assistant {
                turn: 1,
                blocks: vec![Block::Text { text: "b".into() }],
                usage: None,
            },
            Entry::User { text: "c".into() },
        ];
        // prev：顶在首条 → 归顶；顶在中部（offset>0）→ 该消息起点；
        // 顶恰在消息起点 → 前一条。
        assert_eq!(jump_target(&entries, 0, 0.0, -1), Some(0));
        assert_eq!(jump_target(&entries, 1, 0.0, -1), Some(0));
        assert_eq!(jump_target(&entries, 2, 5.0, -1), Some(2));
        assert_eq!(jump_target(&entries, 2, 0.0, -1), Some(0));
        assert_eq!(jump_target(&entries, 3, 0.0, -1), Some(2));
        // next：跳过 notice；末条之后 → None（贴底恢复跟随）。
        assert_eq!(jump_target(&entries, 0, 0.0, 1), Some(2));
        assert_eq!(jump_target(&entries, 2, 0.0, 1), Some(3));
        assert_eq!(jump_target(&entries, 3, 0.0, 1), None);
    }

    /// 生产度量：text_w = 视口宽 − 40；主题钉死常数就位。
    #[test]
    fn production_metrics_derive_text_width() {
        let m = ScrollMetrics::production(800.0, 18.0);
        assert!((m.text_w - 760.0).abs() < 1e-6);
        assert!((m.char_w - CHAR_ADVANCE_PX).abs() < 1e-6);
        assert!(m.text_line > 0.0 && m.user_folded > 0.0 && m.base_pad > 0.0);
        // 极窄视口不下溢。
        let m = ScrollMetrics::production(50.0, 18.0);
        assert!(m.text_w >= 100.0);
    }

    /// 步进：均匀条目下整页 ≈ 页高/条高；末条饱和；顶条向上停 0。
    #[test]
    fn page_walk_clamps_and_directions() {
        let app = App::new();
        let entries: Vec<Entry> = (0..30)
            .map(|_| Entry::User {
                text: "a\nb".into(), // 段落拼接 1 行：30+20=50px
            })
            .collect();
        let m = m();
        // 从 0 起向下 2.5 条高（170px）→ 最近落点 2 或 3（136/204 → 170-136=34 < 204-170=34并列取先者）。
        let t = page_down_target(&entries, &app, 0, 0.0, 170.0, &m);
        assert!((2..=3).contains(&t), "向下落点 {t}");

        // 末条饱和：从 29 起仍返回 29。
        let t = page_down_target(&entries, &app, 29, 0.0, 500.0, &m);
        assert_eq!(t, 29, "末条饱和");

        // 顶条向上停 0；带 offset 时 from=0 也停 0。
        let t = page_up_target(&entries, &app, 0, 0.0, 500.0, &m);
        assert_eq!(t, 0, "顶条饱和");
        let t = page_up_target(&entries, &app, 0, 10.0, 500.0, &m);
        assert_eq!(t, 0, "顶条带 offset 仍 0");

        // 从 10 起向上 170px（≈2.5 条）→ 落点 7 或 8。
        let t = page_up_target(&entries, &app, 10, 0.0, 170.0, &m);
        assert!((7..=8).contains(&t), "向上落点 {t}");
    }
}
