//! 条目级布局缓存（design D1/D2/D7）：把 `chat_lines_themed` 的逐条目
//! 排版产物按条目缓存，每帧只重排"脏范围 + 尾部窗口"，易失尾槽
//! （in-flight 流式 / 运行中 bash）恒常重建。
//!
//! 等价性：条目排版走 [`crate::app::chat::layout_entry`]（与全量 oracle
//! 同一实现），序号以"条目基值 + 局部游标"在构建时组合为全局值，保证
//! 缓存路径与全量路径逐字节一致（parity 单测硬性验收，design D2）。

use ratatui::text::Line;

use pi_shell_rs::session::{Block, SessionState};

use crate::app::chat::{self, RenderFlags};
use crate::app::md_stream::StreamMemo;
use crate::app::theme::TuiTheme;
use crate::app::wrap::{self, HitKind};

/// 尾部常新窗口条数（design D4：tool_end 迟到乱序等尾部兜底）。
const TAIL_WINDOW: usize = 2;

/// 布局键（design D1/D4）：width/theme/flags 任一变化 → 整体重建。
/// 单块/单卡 override 不入键，走 [`LayoutCache::invalidate_*`] 精确失效。
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutKey {
    /// 视口宽度（单元格；构建时按 `max(1)` 归一参与换行计数）。
    width: u16,
    /// 渲染偏好（thinking/工具展开）。
    flags: RenderFlags,
    /// 注入配色（烧入行样式，变化即整体重建）。
    theme: TuiTheme,
}

/// 单条目布局产物（仅在自身内容或渲染键变化时重建）。
#[derive(Debug)]
struct EntryLayout {
    /// 该条目的全部逻辑行（未 wrap；wrap 计数另存）。
    lines: Vec<Line<'static>>,
    /// 逐逻辑行命中种属，携带**全局**序号（构建时以条目基值渲染）。
    meta: Vec<Vec<HitKind>>,
    /// 本条目逻辑行数（行号前缀和原料）。
    line_count: usize,
    /// 本条目消耗的全局 thinking 序号数（含空块；前缀和原料）。
    thinking_count: usize,
    /// 本条目消耗的语义消息序号数（0 或 1；前缀和原料）。
    msg_consumed: usize,
    /// 构建时的全局行号基值（对账/视口定位，design D3 原料）。
    base_row: usize,
    /// 构建时的全局 thinking 序号基值。
    base_thinking: usize,
    /// 构建时的全局语义消息序号基值。
    base_msg: usize,
    /// 逐逻辑行的 wrap 后屏行数（宽度键控；P2 视口化前缀和原料，design D3）。
    wrapped_rows_per_line: Vec<u32>,
    /// 本条目 wrap 后总屏行数（= 计数和；窗口起点按块定位的 O(1) 原料）。
    screen_rows: usize,
    /// 标记跨度 `(first, last, first_offset)`；None = 全块无标记（命中
    /// 扫描 O(1) 跳过，design D3）。
    span: Option<(u32, u32, u32)>,
}

/// 易失尾槽分块（design D1：按 block 分槽，双块交错独立推进）。
#[derive(Debug)]
enum InflightSlot {
    /// Text 块：流式行级 memo（markdown 块上下文 + HighlightState 前缀推进；
    /// Box 装箱压平槽位体积，clippy large_enum_variant）。
    Text(Box<StreamMemo>),
    /// thinking / 工具卡块：每帧重建（行数小；工具输出流 memo 为 follow-up）。
    Other {
        lines: Vec<Line<'static>>,
        meta: Vec<Vec<HitKind>>,
        counts: Vec<u32>,
        total: usize,
        span: Option<(u32, u32, u32)>,
    },
}

impl InflightSlot {
    /// 槽与块形态匹配（Text/其它两分；内容变化由 memo 自防御）。
    ///
    /// 输入: block — in-flight 内容块
    /// 输出: true = 该槽可继续复用
    fn matches(&self, block: &Block) -> bool {
        matches!(self, InflightSlot::Text(_)) == matches!(block, Block::Text { .. })
    }

    /// 新建空槽（宽度键控 memo）。
    ///
    /// 输入: block — in-flight 内容块；width — 视口宽度
    /// 输出: 与块形态匹配的空槽
    fn fresh(block: &Block, width: u16) -> Self {
        match block {
            Block::Text { .. } => InflightSlot::Text(Box::new(StreamMemo::new(width))),
            _ => InflightSlot::Other {
                lines: Vec::new(),
                meta: Vec::new(),
                counts: Vec::new(),
                total: 0,
                span: None,
            },
        }
    }

    /// 刷新单槽：Text 走行级 memo（免重排短路），其它每帧重建。
    ///
    /// 输入: block — in-flight 内容块；flags/overrides/thinking_overrides/
    ///       theme — 同 `chat_lines_themed`；thinking_seq — 全局 thinking
    ///       序号游标（入口 = 前置块合计，出口 = 含本块合计）；width —
    ///       视口宽度
    /// 输出: 无返回值；副作用为重建/推进该槽的行、计数与标记跨度
    #[allow(clippy::too_many_arguments)]
    fn refresh(
        &mut self,
        block: &Block,
        flags: RenderFlags,
        overrides: &std::collections::HashMap<String, bool>,
        thinking_overrides: &std::collections::HashMap<usize, bool>,
        thinking_seq: &mut usize,
        theme: &TuiTheme,
        width: u16,
    ) {
        match self {
            InflightSlot::Text(memo) => {
                if let Block::Text { text } = block {
                    memo.render(text, theme, width);
                }
            }
            InflightSlot::Other {
                lines,
                meta,
                counts,
                total,
                span,
            } => {
                lines.clear();
                meta.clear();
                chat::append_blocks(
                    lines,
                    meta,
                    std::slice::from_ref(block),
                    flags,
                    overrides,
                    thinking_overrides,
                    thinking_seq,
                    theme,
                );
                *counts = lines
                    .iter()
                    .map(|l| wrap::count_wrapped_rows(l, width as usize))
                    .collect();
                *total = counts.iter().map(|c| *c as usize).sum();
                *span = wrap::mark_span(meta, counts);
            }
        }
    }
}

/// 条目级布局缓存（App 持有；渲染与选区文档共用）。
#[derive(Debug)]
pub struct LayoutCache {
    /// 当前产物对应的布局键（width=0 表示未建，强制首建）。
    key: LayoutKey,
    /// 与 `SessionState::entries` 同序的条目布局。
    entries: Vec<EntryLayout>,
    /// 上次消费的 `rev`（draw 对账安全网基准，design D7）。
    last_rev: u64,
    /// App 侧 override 失效的待处理最小下标（`usize::MAX` = 无）。
    pending_from: usize,
    /// 漏标降级全量重建的累计次数（design D7 观测/断言钩子）。
    miss_rebuilds: usize,
    /// 易失尾槽：in-flight 逐块分槽（Text 块行级 memo / 其余每帧重建）。
    inflight: Vec<InflightSlot>,
    /// 易失尾槽：运行中 bash 布局（每帧重建，不入 `entries`）。
    bash_lines: Vec<Line<'static>>,
    bash_meta: Vec<Vec<HitKind>>,
    /// 易失尾槽：bash 逐逻辑行 wrap 计数（宽度键控，design D3）。
    bash_counts: Vec<u32>,
    /// 易失尾槽：bash 标记跨度（每帧重算，design D3）。
    bash_span: Option<(u32, u32, u32)>,
}

impl Default for LayoutCache {
    /// 空缓存：键 width=0（首帧必与真实宽度不等 → 全量冷建）；
    /// `pending_from = usize::MAX`（无待处理失效）。
    ///
    /// 输入: 无；输出: 全字段默认的空缓存
    fn default() -> Self {
        Self {
            key: LayoutKey {
                width: 0,
                flags: RenderFlags::default(),
                theme: TuiTheme::default(),
            },
            entries: Vec::new(),
            last_rev: 0,
            pending_from: usize::MAX,
            miss_rebuilds: 0,
            inflight: Vec::new(),
            bash_lines: Vec::new(),
            bash_meta: Vec::new(),
            bash_counts: Vec::new(),
            bash_span: None,
        }
    }
}

impl LayoutCache {
    /// 按需重建缓存并刷新易失尾槽（每帧调用，design D7）。
    ///
    /// 输入: state — 会话状态（`take_dirty` 消费脏标记）；flags/overrides/
    ///       thinking_overrides/theme — 同 `chat_lines_themed`；width —
    ///       视口宽度（`max(1)` 归一）
    /// 输出: 无返回值；副作用为增量重排 `[脏下标..]` 与尾部窗口 K≥2、
    ///       整键变化全量重建、易失尾槽常新；错误情形：漏标（rev 变而无脏）
    ///       时降级全量重建并累计 `miss_rebuilds`（不 panic，保降级可测）
    pub fn refresh(
        &mut self,
        state: &mut SessionState,
        flags: RenderFlags,
        overrides: &std::collections::HashMap<String, bool>,
        thinking_overrides: &std::collections::HashMap<usize, bool>,
        theme: &TuiTheme,
        width: u16,
    ) {
        let width = width.max(1);
        let key = LayoutKey {
            width,
            flags,
            theme: theme.clone(),
        };
        let (rev, dirty) = state.take_dirty();
        // 对账（design D7 安全网）：rev 变而无脏 = 漏标 → 降级全量重建。
        let mut from = if dirty == usize::MAX && rev != self.last_rev {
            self.miss_rebuilds += 1;
            0
        } else {
            dirty
        };
        self.last_rev = rev;
        // App 侧 override 精确失效（点击切换单卡/thinking 单块）。
        from = from.min(self.pending_from);
        self.pending_from = usize::MAX;
        // 键变化（width/theme/flags）→ 整体重建。
        if key != self.key {
            from = 0;
        }
        let n = state.entries().len();
        if from != usize::MAX && from > n {
            from = 0; // 收缩防御（整体替换类应已标 0；此处双保险）
        }
        // 尾部窗口 K≥2（design D4）：常新兜底，同时把空闲帧的重建范围
        // 收窄到尾窗（from 恒为实际下标，不再有空档）。
        from = from.min(n.saturating_sub(TAIL_WINDOW));

        // 同步保留段（truncate 保前段；from==0 全清）。
        if from == 0 {
            self.entries.clear();
        } else {
            self.entries.truncate(from);
        }
        // 前缀合计（对保留段全量整数重算，design D7——O(n) 加法）。
        let mut row_base = 0usize;
        let mut thinking_base = 0usize;
        let mut msg_base = 0usize;
        for e in &self.entries {
            // 保留段基值与前缀和一致（缓存内部不变量，design D2/D3；
            // 同时消费 base_row/base_msg/wrap 计数字段供 P2 视口化对账）。
            debug_assert_eq!(e.base_row, row_base, "行基应与前缀和一致");
            debug_assert_eq!(e.base_msg, msg_base, "消息基应与前缀和一致");
            debug_assert_eq!(
                e.wrapped_rows_per_line.len(),
                e.line_count,
                "wrap 计数必须与逻辑行等长"
            );
            row_base += e.line_count;
            thinking_base += e.thinking_count;
            msg_base += e.msg_consumed;
        }
        // 起点之后的条目全部重排版（含尾窗）。
        let start = self.entries.len();
        for i in start..n {
            let entry = &state.entries()[i];
            let (t0, m0, r0) = (thinking_base, msg_base, row_base);
            let mut tcur = thinking_base;
            let mut mcur = msg_base;
            let (lines, meta) = chat::layout_entry(
                entry,
                flags,
                overrides,
                thinking_overrides,
                &mut tcur,
                &mut mcur,
                theme,
            );
            let wrapped_rows_per_line: Vec<u32> = lines
                .iter()
                .map(|l| wrap::count_wrapped_rows(l, width as usize))
                .collect();
            let screen_rows: usize = wrapped_rows_per_line.iter().map(|c| *c as usize).sum();
            let span = wrap::mark_span(&meta, &wrapped_rows_per_line);
            row_base = r0 + lines.len();
            thinking_base = tcur;
            msg_base = mcur;
            self.entries.push(EntryLayout {
                line_count: lines.len(),
                thinking_count: tcur - t0,
                msg_consumed: mcur - m0,
                base_row: r0,
                base_thinking: t0,
                base_msg: m0,
                lines,
                meta,
                wrapped_rows_per_line,
                screen_rows,
                span,
            });
        }
        self.key = key;
        // 易失尾槽：按 block 分槽推进（design D1）——Text 块走行级 memo
        // （markdown 块上下文 + HighlightState 按完结行前缀推进），
        // thinking/工具块每帧重建；in-flight 的 thinking 序号接在条目
        // 合计之后（与全量 oracle 的游标穿透一致）。
        {
            let blocks = state.in_flight();
            if self.inflight.len() != blocks.len()
                || std::iter::zip(&self.inflight, blocks).any(|(s, b)| !s.matches(b))
            {
                self.inflight = blocks.iter().map(|b| InflightSlot::fresh(b, width)).collect();
            }
            for (slot, block) in std::iter::zip(&mut self.inflight, blocks) {
                slot.refresh(
                    block,
                    flags,
                    overrides,
                    thinking_overrides,
                    &mut thinking_base,
                    theme,
                    width,
                );
            }
        }
        let (bl, bm) = chat::layout_bash_tail(state);
        self.bash_lines = bl;
        self.bash_meta = bm;
        self.bash_counts = self
            .bash_lines
            .iter()
            .map(|l| wrap::count_wrapped_rows(l, width as usize))
            .collect();
        self.bash_span = wrap::mark_span(&self.bash_meta, &self.bash_counts);
    }

    /// 借出全部布局分块（条目 + 易失尾槽），供视口窗口化 wrap 消费
    /// （design D3：[`wrap::Chunk`] 含逻辑行、命中 meta、wrap 计数、块
    /// 屏行总数与标记跨度）。
    ///
    /// 输入: 无（&self 借用）
    /// 输出: `Chunk` 序列——顺序与全量 `chat_lines_themed` 的拼装顺序一致
    ///       （条目序 → in-flight → bash），前三者逐行等长；空尾槽不产块
    pub fn chunks(&self) -> Vec<wrap::Chunk<'_>> {
        let mut v: Vec<wrap::Chunk<'_>> = self
            .entries
            .iter()
            .map(|e| wrap::Chunk {
                lines: &e.lines,
                meta: &e.meta,
                counts: &e.wrapped_rows_per_line,
                total: e.screen_rows,
                span: e.span,
            })
            .collect();
        // 易失尾槽逐块出借（design D1：Text 槽行级 memo，其余每帧重建）。
        for slot in &self.inflight {
            match slot {
                InflightSlot::Text(memo) => {
                    let (lines, counts, meta) = memo.parts();
                    if !lines.is_empty() {
                        v.push(wrap::Chunk {
                            lines,
                            meta,
                            counts,
                            total: memo.total(),
                            span: None,
                        });
                    }
                }
                InflightSlot::Other {
                    lines,
                    meta,
                    counts,
                    total,
                    span,
                } => {
                    if !lines.is_empty() {
                        v.push(wrap::Chunk {
                            lines,
                            meta,
                            counts,
                            total: *total,
                            span: *span,
                        });
                    }
                }
            }
        }
        if !self.bash_lines.is_empty() {
            v.push(wrap::Chunk {
                lines: &self.bash_lines,
                meta: &self.bash_meta,
                counts: &self.bash_counts,
                total: self.bash_counts.iter().map(|c| *c as usize).sum(),
                span: self.bash_span,
            });
        }
        v
    }

    /// 标记含指定工具卡的条目待重排（点击切换单卡展开，design D4）。
    ///
    /// 输入: id — toolCallId（来自命中表，故其 Tool 标记必在 meta 中）
    /// 输出: 无返回值；副作用为 `pending_from` 收紧到命中条目下标；
    ///       未找到（理论不可达）时为无操作
    pub fn invalidate_tool(&mut self, id: &str) {
        if let Some(i) = self.entries.iter().position(|e| {
            e.meta
                .iter()
                .any(|m| m.iter().any(|k| matches!(k, HitKind::Tool(t) if t == id)))
        }) {
            self.pending_from = self.pending_from.min(i);
        }
    }

    /// 标记含指定全局 thinking 序号的条目待重排（点击单块展开，design D4）。
    ///
    /// 输入: seq — 全局 thinking 渲染序号（来自命中表，必落在某条目
    ///       `[base_thinking, base_thinking + thinking_count)` 区间内）
    /// 输出: 无返回值；副作用为 `pending_from` 收紧到命中条目下标；
    ///       未找到时为无操作
    pub fn invalidate_thinking(&mut self, seq: usize) {
        if let Some(i) = self
            .entries
            .iter()
            .position(|e| seq >= e.base_thinking && seq < e.base_thinking + e.thinking_count)
        {
            self.pending_from = self.pending_from.min(i);
        }
    }

    /// 漏标降级全量重建的累计次数（design D7 观测钩子；draw 侧 debug_assert）。
    ///
    /// 输入: 无；输出: 自缓存创建以来的漏标次数
    pub fn miss_rebuilds(&self) -> usize {
        self.miss_rebuilds
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::scroll::ChatScroll;
    use pi_shell_rs::session::{replay_jsonl, Entry, ToolExecution, ToolStatus};
    use serde_json::json;
    use std::collections::HashMap;
    use std::time::Instant;

    /// 默认参数三件套（测试用空覆盖表与默认主题）。
    fn defaults() -> (HashMap<String, bool>, HashMap<usize, bool>, TuiTheme) {
        (HashMap::new(), HashMap::new(), TuiTheme::default())
    }

    /// 把缓存分块摊平为全量 `(lines, meta)`（仅测试比较用）。
    fn flatten(cache: &LayoutCache) -> (Vec<Line<'static>>, Vec<Vec<HitKind>>) {
        let mut lines = Vec::new();
        let mut meta = Vec::new();
        for c in cache.chunks() {
            lines.extend_from_slice(c.lines);
            meta.extend_from_slice(c.meta);
        }
        (lines, meta)
    }

    /// 缓存路径 vs 全量 oracle：摊平后逐字节相等（design D2 等价性）。
    fn assert_parity(
        cache: &mut LayoutCache,
        state: &mut SessionState,
        flags: RenderFlags,
        overrides: &HashMap<String, bool>,
        thinking_overrides: &HashMap<usize, bool>,
        theme: &TuiTheme,
        width: u16,
    ) {
        cache.refresh(state, flags, overrides, thinking_overrides, theme, width);
        let (cl, cm) = flatten(cache);
        let (ol, om) =
            chat::chat_lines_themed(state, flags, overrides, thinking_overrides, theme);
        assert_eq!(cl, ol, "缓存行与全量路径必须逐字节相等");
        assert_eq!(cm, om, "缓存 meta（含全局序号）与全量路径必须逐号相等");
    }

    /// 构造含 user / assistant（text+thinking）/ notice 的样例会话。
    fn sample_state() -> SessionState {
        let mut s = SessionState::new();
        s.handle(&json!({"type":"message_start","message":{"role":"user","content":[{"type":"text","text":"hello"}]}}));
        s.handle(&json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        s.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":0}}));
        s.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"hi **there**"}}));
        s.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_start","contentIndex":1}}));
        s.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","contentIndex":1,"delta":"hmm"}}));
        s.handle(&json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"hi **there**"},{"type":"thinking","thinking":"hmm"}]}}));
        s.push_notice("compaction", "压缩完成".into());
        s
    }

    /// 流式逐 delta parity（design D2/D3）：source 按步长切成 delta 逐个
    /// 喂入 in-flight，每 delta 断言缓存摊平与全量 oracle 逐字节相等，
    /// 落盘（message_end）后复验一次。
    fn assert_stream_parity_chunks(source: &str, step: usize) {
        let mut cache = LayoutCache::default();
        let mut state = SessionState::new();
        let (o, t, theme) = defaults();
        let flags = RenderFlags::default();
        assert_parity(&mut cache, &mut state, flags, &o, &t, &theme, 80);
        state.handle(&json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        let chars: Vec<char> = source.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let end = (i + step).min(chars.len());
            let delta: String = chars[i..end].iter().collect();
            state.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":delta}}));
            assert_parity(&mut cache, &mut state, flags, &o, &t, &theme, 80);
            i = end;
        }
        state.handle(&json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":source}]}}));
        assert_parity(&mut cache, &mut state, flags, &o, &t, &theme, 80);
    }

    /// 取 in-flight Text 槽的 memo（fallback 观测用）。
    fn inflight_text_memo(cache: &LayoutCache) -> Option<&StreamMemo> {
        cache.inflight.iter().find_map(|s| match s {
            InflightSlot::Text(m) => Some(m.as_ref()),
            _ => None,
        })
    }

    /// 2.3 parity：互异代码块逐 delta 等价（行级 HighlightState memo 的
    /// 核心场景），且快路径真实生效（未转全量回退）。
    #[test]
    fn stream_parity_distinct_code_blocks() {
        let mut src = String::new();
        for k in 0..6 {
            src.push_str(&format!(
                "段落 {k}：**流式** 正文与 `行内代码`。\n\n```rust\nfn demo_{k}(x: usize) -> usize {{\n    // 块 {k}\n    (0..x).map(|i| i * {k}).sum()\n}}\n```\n\n"
            ));
        }
        assert_stream_parity_chunks(&src, 1);
        assert_stream_parity_chunks(&src, 7);
    }

    /// 2.3 parity：嵌套列表/引用逐 delta 等价（含松紧表 retro、懒续行、
    /// 任务列表与有序序号重排）。
    #[test]
    fn stream_parity_nested_lists_and_quotes() {
        let src = "- 一级 a\n  - 二级 b\n    - 三级 c\n  续行文本\n- 一级 d\n\n1. 第一\n2. 第二\n   嵌套续\n\n> 引用一\n> 引用二\n>\n> - 引用内列表\n>   - 深层\n\n- 松散\n\n- 表项\n\n- [ ] 待办\n- [x] 完成\n";
        assert_stream_parity_chunks(src, 1);
        assert_stream_parity_chunks(src, 5);
    }

    /// 2.3 parity：畸形/未闭合 fence 逐 delta 等价（fence 内 fence、
    /// 闭合行带垃圾、波浪/反引号混用、空 fence）。
    #[test]
    fn stream_parity_malformed_fences() {
        let src = "```rust\nfn a() {}\n``` junk\n\n````\n```\n嵌套外观\n````\n\n~~~\nx\n```\n~~~\n\n```\n```\n\n```notalang-xyz\n纯文本降级\n```\n\n```a`b\n信息串含反引号不成 fence\n";
        assert_stream_parity_chunks(src, 1);
        assert_stream_parity_chunks(src, 4);
    }

    /// 2.3 parity：CJK 混排逐 delta 等价（双格宽度、行内标记、硬换行）。
    #[test]
    fn stream_parity_cjk_mix() {
        let src = "中文**加粗**与 *斜体* 混排 wrap 压力测试。行内 `code_x` 与中文。\n第二行：全角标点（，。！）与 half-width mix。  \n硬换行后第三行 with english words and 中文词。\n\n- 列表：中文项 `a`\n- 列表：中文项 **b**\n";
        assert_stream_parity_chunks(src, 1);
        assert_stream_parity_chunks(src, 3);
    }

    /// 2.3 parity：thinking + text 双块交错逐 delta 等价（design D1 风险
    /// 条目：并发流按 block 分槽独立推进）。
    #[test]
    fn stream_parity_dual_block_interleave() {
        let mut cache = LayoutCache::default();
        let mut state = SessionState::new();
        let (o, t, theme) = defaults();
        let flags = RenderFlags::default();
        state.handle(&json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        state.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":0}}));
        state.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_start","contentIndex":1}}));
        let text_parts = ["正文 **第一段**\n", "继续正文\n", "```rust\nfn x() {}\n", "```\n"];
        let think_parts = ["思考一 ", "思考二\n", "思考三", " 收尾"];
        for i in 0..4 {
            state.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":text_parts[i]}}));
            assert_parity(&mut cache, &mut state, flags, &o, &t, &theme, 80);
            state.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","contentIndex":1,"delta":think_parts[i]}}));
            assert_parity(&mut cache, &mut state, flags, &o, &t, &theme, 80);
        }
    }

    /// 2.3 parity：setext 回溯/行内标记跨行/尾随反斜杠/HTML/缩进代码等
    /// 不确定构造逐 delta 等价（整区重排吸收，失败安全）。
    #[test]
    fn stream_parity_uncertain_constructs() {
        let src = "标题下划线\n===\n\n跨行 *强调\n继续* 收尾。\n\n尾随反斜杠\\\n下一行。\n\n缩进代码候选\n    四空格行\n\n等号行\n--\n\n<div>\nhtml 块跨区泄漏\n</div>\n";
        assert_stream_parity_chunks(src, 1);
        assert_stream_parity_chunks(src, 6);
    }

    /// 2.3 失败安全：reference 定义/空列表项（parse_to_lines 跨区泄漏
    /// 怪癖）触发整槽全量回退，回退后仍逐字节等价且可观测。
    #[test]
    fn stream_fallback_is_fail_safe_and_observable() {
        for src in [
            "引用行 [1] 在此。\n\n[1]: https://example.com\n",
            "-\n泄漏怪癖\n",
            "正文\n\n1.\n空序号项\n",
            "#\n空标题怪癖\n",
        ] {
            let mut cache = LayoutCache::default();
            let mut state = SessionState::new();
            let (o, t, theme) = defaults();
            let flags = RenderFlags::default();
            state.handle(&json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
            for ch in src.chars() {
                let delta = ch.to_string();
                state.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":delta}}));
                assert_parity(&mut cache, &mut state, flags, &o, &t, &theme, 80);
            }
            assert!(
                inflight_text_memo(&cache).is_some_and(|m| m.is_fallback()),
                "应触发失败安全回退: {src:?}"
            );
        }
    }

    /// 2.3 快路径观测：常规流式内容（段落 + 代码块）不触发全量回退
    /// （防止 memo 退化成每帧全量重排而不自知）。
    #[test]
    fn stream_fast_path_engages_for_common_content() {
        let mut cache = LayoutCache::default();
        let mut state = SessionState::new();
        let (o, t, theme) = defaults();
        let flags = RenderFlags::default();
        state.handle(&json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        let src = "普通段落。\n\n```rust\nfn a() {}\n```\n\n- 列表项\n- 列表项二\n";
        for ch in src.chars() {
            let delta = ch.to_string();
            state.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":delta}}));
            cache.refresh(&mut state, flags, &o, &t, &theme, 80);
        }
        assert!(
            !inflight_text_memo(&cache).is_some_and(|m| m.is_fallback()),
            "常规内容不应触发全量回退"
        );
    }

    /// 2.1 编号等价（局部游标 + 条目基值 → 全局）：跨空/单条/流式/落盘/
    /// 通知各态，缓存 meta 的 thinking/msg 序号与全量路径逐号相等。
    /// 输入: 无；输出: 各态 refresh 后 parity 断言通过
    #[test]
    fn cache_numbering_matches_oracle_across_states() {
        let mut cache = LayoutCache::default();
        let mut state = SessionState::new();
        let (o, t, theme) = defaults();
        let flags = RenderFlags::default();

        // 空会话。
        assert_parity(&mut cache, &mut state, flags, &o, &t, &theme, 80);
        // 单条目。
        state.handle(&json!({"type":"message_start","message":{"role":"user","content":[{"type":"text","text":"one"}]}}));
        assert_parity(&mut cache, &mut state, flags, &o, &t, &theme, 80);
        // 流式中途（in-flight 易失尾槽）。
        state.handle(&json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        state.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"str**eam**"}}));
        assert_parity(&mut cache, &mut state, flags, &o, &t, &theme, 80);
        // 落盘 + 通知（尾部 push）。
        state.handle(&json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"str**eam**"}]}}));
        state.push_notice("retry", "重试 1/3: x".into());
        assert_parity(&mut cache, &mut state, flags, &o, &t, &theme, 80);
    }

    /// 2.3 中部变异（apply_tool 乱序命中倒数第 2 条）：精确脏下标 →
    /// 重建后仍与全量路径逐字节相等。
    /// 输入: 无；输出: 乱序 tool 事件后 parity 断言通过
    #[test]
    fn cache_parity_after_mid_list_tool_mutation() {
        let mut cache = LayoutCache::default();
        let mut state = sample_state();
        let (o, t, theme) = defaults();
        let flags = RenderFlags::default();
        assert_parity(&mut cache, &mut state, flags, &o, &t, &theme, 80);

        // 追加含 ToolCall（已有结果）的 assistant，再跟两条通知——
        // tool 事件将命中**倒数第 3/中部**条目（乱序场景，design D4）。
        state.transcript.push(Entry::Assistant {
            turn: 9,
            blocks: vec![pi_shell_rs::session::Block::ToolCall {
                id: "c1".into(),
                name: "bash".into(),
                args_json: "{}".into(),
                tool: ToolExecution {
                    status: ToolStatus::Ended,
                    is_error: false,
                    result: Some(json!({"content":"ok"})),
                    ..Default::default()
                },
            }],
            usage: None,
        });
        state.push_notice("compaction", "尾随通知 1".into());
        state.push_notice("compaction", "尾随通知 2".into());
        // 直接 push 不带标记：下一次 refresh 由尾窗补齐（见下一次 parity
        // 断言），随后的 tool 事件才是带精确脏下标的中部变异。
        assert_parity(&mut cache, &mut state, flags, &o, &t, &theme, 80);

        // 乱序 tool_execution_end：apply_tool 命中已提交条目（非尾窗首
        // 位——倒数第 3 条），带精确脏下标重建。
        state.handle(&json!({"type":"tool_execution_end","toolCallId":"c1","isError":false,"result":{"content":"done"}}));
        assert_parity(&mut cache, &mut state, flags, &o, &t, &theme, 80);
    }

    /// 5.2 wrap 层 parity：同一状态下，窗口化路径与全文单片路径的窗口行
    /// （含 hits 区间）与 ChatScroll 回写逐项相等（钉住/贴底两态都比）。
    /// 输入: 无；输出: wrapped/next/hits 三元组逐项断言相等
    #[test]
    fn wrap_parity_between_cache_chunks_and_full_path() {
        let mut cache = LayoutCache::default();
        let mut state = sample_state();
        state.transcript.push(Entry::Assistant {
            turn: 9,
            blocks: vec![pi_shell_rs::session::Block::ToolCall {
                id: "cw".into(),
                name: "bash".into(),
                args_json: "{}".into(),
                tool: ToolExecution {
                    status: ToolStatus::Ended,
                    is_error: false,
                    result: Some(json!({"content":"ok"})),
                    ..Default::default()
                },
            }],
            usage: None,
        });
        let (o, t, theme) = defaults();
        let flags = RenderFlags::default();
        cache.refresh(&mut state, flags, &o, &t, &theme, 64);
        let (ol, om) = chat::chat_lines_themed(&state, flags, &o, &t, &theme);
        let (cl, cm) = flatten(&cache);
        assert_eq!(cl, ol, "入 wrap 前两路径必须逐字节相等");
        assert_eq!(cm, om);

        for (top, following) in [(0usize, false), (3, false), (0, true)] {
            let scroll = ChatScroll {
                top,
                following,
                ..Default::default()
            };
            let overlay = Some(wrap::ScrollbarOverlay { hovered: false });
            let (fw, fs, fh) =
                wrap::wrap_for_viewport(&ol, 64, 10, &scroll, &om, overlay, &theme);
            let chunks = cache.chunks();
            let (cw, cs, ch) =
                wrap::wrap_for_viewport_windowed(&chunks, 64, 10, &scroll, overlay, &theme);
            assert_eq!(fw, cw, "窗口行必须逐字节相等 (top={top}, following={following})");
            assert_eq!(fs, cs, "ChatScroll 回写必须相等");
            assert_eq!(fh, ch, "hits 绝对屏行区间必须相等");
        }
    }

    /// 5.1/5.2 窗口边界回归：窗口化路径与全文路径在空行段/超宽硬切/行内
    /// 偏移/越界 clamp/贴底/空内容各边界逐字节相等（含 hits 与 ChatScroll）。
    /// 输入: 无（窄宽 20 列强制硬切）；输出: 各滚动位置三元组逐项断言相等
    #[test]
    fn windowed_matches_full_on_boundaries() {
        let mut state = SessionState::new();
        // 混合内容：空行、超长 ASCII（硬切）、中文双格行、多段落。
        state.transcript.push(Entry::User {
            text: "首行\n\n空行后\n一段超长 ASCII 需要硬切 aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 结束\n中文超宽行：测试双格字符的硬切与换行行为测试双格字符的硬切与换行行为测试双格字符的硬切与换行行为".into(),
        });
        state.push_notice("compaction", "通知行\n第二行".into());
        state.handle(&json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        let delta = "流式内容 **加粗** 与 `代码`\n\n- 列表项1\n- 列表项2";
        state.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":delta}}));
        state.handle(&json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":delta}]}}));

        let (o, t, theme) = defaults();
        let flags = RenderFlags::default();
        let mut cache = LayoutCache::default();
        cache.refresh(&mut state, flags, &o, &t, &theme, 20);
        let (ol, om) = chat::chat_lines_themed(&state, flags, &o, &t, &theme);

        let positions = [
            (0usize, false), // 钉住顶部
            (1, false),      // 行内偏移（首行被 wrap 成多行）
            (3, false),      // 中部（跨行中段）
            (999, false),    // 越界 → clamp 到底
            (0, true),       // 贴底跟随
        ];
        for (top, following) in positions {
            let scroll = ChatScroll {
                top,
                following,
                ..Default::default()
            };
            let overlay = Some(wrap::ScrollbarOverlay { hovered: true });
            let (fw, fs, fh) =
                wrap::wrap_for_viewport(&ol, 20, 5, &scroll, &om, overlay, &theme);
            let chunks = cache.chunks();
            let (cw, cs, ch) =
                wrap::wrap_for_viewport_windowed(&chunks, 20, 5, &scroll, overlay, &theme);
            assert_eq!(fw, cw, "窗口行逐字节 (top={top}, following={following})");
            assert_eq!(fs, cs, "ChatScroll 回写 (top={top}, following={following})");
            assert_eq!(fh, ch, "hits (top={top}, following={following})");
        }

        // 空内容：两路径皆空、回写一致。
        let mut empty_state = SessionState::new();
        let mut empty_cache = LayoutCache::default();
        empty_cache.refresh(&mut empty_state, flags, &o, &t, &theme, 20);
        let (el, em) = chat::chat_lines_themed(&empty_state, flags, &o, &t, &theme);
        let scroll = ChatScroll::default();
        let (fw, fs, fh) = wrap::wrap_for_viewport(&el, 20, 5, &scroll, &em, None, &theme);
        let chunks = empty_cache.chunks();
        let (cw, cs, ch) =
            wrap::wrap_for_viewport_windowed(&chunks, 20, 5, &scroll, None, &theme);
        assert_eq!(fw, cw, "空内容窗口行");
        assert_eq!(fs, cs, "空内容回写");
        assert_eq!(fh, ch, "空内容 hits");
    }

    /// 2.3 宽度变化（LayoutKey）→ 整体重建后仍等价。
    /// 输入: 无；输出: 80→40 切换后 parity 断言通过
    #[test]
    fn cache_parity_after_width_change() {
        let mut cache = LayoutCache::default();
        let mut state = sample_state();
        let (o, t, theme) = defaults();
        let flags = RenderFlags::default();
        assert_parity(&mut cache, &mut state, flags, &o, &t, &theme, 80);
        assert_parity(&mut cache, &mut state, flags, &o, &t, &theme, 40);
        assert_parity(&mut cache, &mut state, flags, &o, &t, &theme, 80);
    }

    /// 2.3 override 切换：单块 thinking / 单卡 tool 失效目标条目后等价。
    /// 输入: 无；输出: 注入覆盖表 + invalidate 后 parity 断言通过
    #[test]
    fn cache_parity_after_override_invalidation() {
        let mut cache = LayoutCache::default();
        let mut state = sample_state();
        let (mut o, mut t, theme) = defaults();
        let flags = RenderFlags::default();
        assert_parity(&mut cache, &mut state, flags, &o, &t, &theme, 80);

        // thinking 单块展开：全局 seq 0（样例首个非空 thinking 块）。
        t.insert(0, true);
        cache.invalidate_thinking(0);
        assert_parity(&mut cache, &mut state, flags, &o, &t, &theme, 80);

        // 单卡展开（带结果的工具卡才入命中表 → invalidate 可定位）。
        state.transcript.push(Entry::Assistant {
            turn: 9,
            blocks: vec![pi_shell_rs::session::Block::ToolCall {
                id: "cx".into(),
                name: "bash".into(),
                args_json: "{}".into(),
                tool: ToolExecution {
                    status: ToolStatus::Ended,
                    is_error: false,
                    result: Some(json!({"content":"ok"})),
                    ..Default::default()
                },
            }],
            usage: None,
        });
        assert_parity(&mut cache, &mut state, flags, &o, &t, &theme, 80);
        o.insert("cx".into(), true);
        cache.invalidate_tool("cx");
        assert_parity(&mut cache, &mut state, flags, &o, &t, &theme, 80);
    }

    /// 1.3 漏标安全网：只 bump rev 不落脏 → 对账降级全量重建，输出不 stale
    /// 且 `miss_rebuilds` 计数可观测。
    /// 输入: 无；输出: 直改早条目 + 漏标模拟后 parity 通过、计数 == 1
    #[test]
    fn missed_mark_falls_back_to_full_rebuild() {
        let mut cache = LayoutCache::default();
        let mut state = sample_state();
        let (o, t, theme) = defaults();
        let flags = RenderFlags::default();
        assert_parity(&mut cache, &mut state, flags, &o, &t, &theme, 80);
        assert_eq!(cache.miss_rebuilds(), 0);

        // 直改早条目（下标 0，尾窗覆盖不到）+ 模拟漏标（只 bump rev）。
        if let Entry::User { text } = &mut state.transcript[0] {
            text.push_str(" MODIFIED");
        }
        state.simulate_missed_mark();
        assert_parity(&mut cache, &mut state, flags, &o, &t, &theme, 80);
        assert_eq!(cache.miss_rebuilds(), 1, "漏标应触发降级全量重建");
        let (cl, _) = flatten(&cache);
        assert!(
            cl.iter()
                .any(|l| l.spans.iter().any(|s| s.content.contains("MODIFIED"))),
            "降级全量重建后不得渲染 stale 内容"
        );
    }

    /// 2.1 尾窗常新（design D4）：无任何标记的空闲帧也会重建最后 K=2 条，
    /// 使未走标记路径的尾部直改不 stale。
    /// 输入: 无；输出: 直改尾部条目（不 bump rev）后 refresh 仍等价
    #[test]
    fn tail_window_keeps_last_entries_fresh() {
        let mut cache = LayoutCache::default();
        let mut state = sample_state();
        let (o, t, theme) = defaults();
        let flags = RenderFlags::default();
        assert_parity(&mut cache, &mut state, flags, &o, &t, &theme, 80);

        // 直改倒数第 2 条（尾窗 K=2 覆盖范围内），不落任何标记。
        let idx = state.transcript.len() - 2;
        if let Entry::Assistant { blocks, .. } = &mut state.transcript[idx]
            && let pi_shell_rs::session::Block::Text { text } = &mut blocks[0]
        {
            text.push_str(" TAIL");
        }
        assert_parity(&mut cache, &mut state, flags, &o, &t, &theme, 80);
        let (cl, _) = flatten(&cache);
        assert!(
            cl.iter().any(|l| l.spans.iter().any(|s| s.content.contains("TAIL"))),
            "尾窗应覆盖倒数第 2 条的直改"
        );
    }

    /// 合成单条回复正文（≥ 7.8KB，含标题/粗体/行内代码/列表/段落；
    /// 按序号唯一，避免内容哈希缓存全命中失真）。
    fn assistant_blob(i: usize) -> String {
        let mut s = String::with_capacity(8192);
        s.push_str(&format!(
            "# 合成回复 {i}\n\n这是**第 {i} 条**合成内容，含 `行内代码`、列表与普通段落。\n\n"
        ));
        for k in 0..10 {
            s.push_str(&format!("- 条目 {i}.{k}：`let v_{i}_{k} = {};`\n", k * 7 + i));
        }
        s.push('\n');
        let mut k = 0usize;
        while s.len() < 7800 {
            s.push_str(&format!(
                "段落 {i}.{k}：合成文本用于撑大体积，The quick brown fox {i} jumps over the lazy dog，第 {k} 段。\n\n"
            ));
            k += 1;
        }
        s
    }

    /// 合成基准会话 JSONL（750 轮 × 2 条目 = 1500 条目、≥5MB；纯合成
    /// 文本、无真实会话内容），形如探针归档 `{seq,dir,raw}` 逐行。
    fn synthetic_jsonl() -> String {
        let mut out = String::with_capacity(8 << 20);
        let mut seq = 0u64;
        for i in 0..750 {
            let user_text = format!("问题 {i}：请说明合成用例 {i} 的行为。");
            let blob = assistant_blob(i);
            let events = [
                json!({"type":"message_start","message":{"role":"user","content":[{"type":"text","text":user_text}]}}),
                json!({"type":"message_start","message":{"role":"assistant","content":[]}}),
                json!({"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":0}}),
                json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":blob.clone()}}),
                json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":blob}]}}),
            ];
            for raw in events {
                out.push_str(&format!("{{\"seq\":{seq},\"dir\":\"in\",\"raw\":{}}}\n", raw));
                seq += 1;
            }
        }
        out
    }

    /// 实测缓存内存足迹（结构头 + 行/meta/计数内容字节近似和；含 String
    /// 堆内容长度，不含分配器开销）。
    fn cache_memory_footprint(cache: &LayoutCache) -> usize {
        fn line_bytes(l: &Line<'static>) -> usize {
            std::mem::size_of_val(l)
                + l.spans
                    .iter()
                    .map(|s| std::mem::size_of_val(s) + s.content.as_ref().len())
                    .sum::<usize>()
        }
        fn meta_bytes(m: &[Vec<HitKind>]) -> usize {
            std::mem::size_of_val(m)
                + m.iter()
                    .map(|row| {
                        std::mem::size_of_val(row)
                            + row.iter()
                                .map(|k| {
                                    std::mem::size_of_val(k)
                                        + if let HitKind::Tool(id) = k { id.len() } else { 0 }
                                })
                                .sum::<usize>()
                    })
                    .sum::<usize>()
        }
        let mut total = std::mem::size_of_val(cache);
        for e in &cache.entries {
            total += std::mem::size_of_val(e);
            total += e.lines.iter().map(line_bytes).sum::<usize>();
            total += meta_bytes(&e.meta);
            total += std::mem::size_of_val(e.wrapped_rows_per_line.as_slice());
        }
        for slot in &cache.inflight {
            match slot {
                InflightSlot::Text(memo) => total += memo.memory_bytes(),
                InflightSlot::Other {
                    lines,
                    meta,
                    counts,
                    ..
                } => {
                    total += lines.iter().map(line_bytes).sum::<usize>();
                    total += meta_bytes(meta);
                    total += std::mem::size_of_val(counts.as_slice());
                }
            }
        }
        total += cache.bash_lines.iter().map(line_bytes).sum::<usize>();
        total += meta_bytes(&cache.bash_meta);
        total
    }

    /// 4.1 基准：合成 ~1500 条目 / ≥5MB 会话（replay_jsonl 回放，无真实
    /// 内容），计时暖帧（refresh 无新事件 + 分块 + 全文 wrap）断言 < 5ms
    /// （best-of-10 宽松阈值防 CI 抖动）；同步实测冷建耗时、旧全量路径
    /// 对照与缓存内存足迹（超 D6 预算 2 倍 = 触发 P3 信号）。
    /// 输入: 无；输出: 计时/内存 eprintln 注记 + 预算断言
    #[test]
    fn bench_warm_frame_under_budget() {
        let jsonl = synthetic_jsonl();
        assert!(
            jsonl.len() >= 5 * 1024 * 1024,
            "合成 JSONL 应 ≥5MB，实得 {}KB",
            jsonl.len() / 1024
        );
        let mut state =
            replay_jsonl(std::io::Cursor::new(jsonl.as_bytes())).expect("合成会话回放");
        let n_entries = state.entries().len();
        assert!(n_entries >= 1400, "条目数应约 1500，实得 {n_entries}");

        let (o, t, theme) = defaults();
        let flags = RenderFlags::default();
        let mut cache = LayoutCache::default();
        // 冷建（允许 O(全文) 一次，design §3）。
        let cold0 = Instant::now();
        cache.refresh(&mut state, flags, &o, &t, &theme, 100);
        let cold_ms = cold0.elapsed().as_secs_f64() * 1e3;

        // 暖帧（滚动/打字稳态）：refresh（无新事件）+ 分块 + 窗口化 wrap。
        // 分段计时：refresh（尾窗重排）与 wrap（命中扫描 + 窗口排版）。
        let mut warm_best = f64::MAX;
        let mut refresh_best = f64::MAX;
        let mut wrap_best = f64::MAX;
        for _ in 0..10 {
            let t0 = Instant::now();
            cache.refresh(&mut state, flags, &o, &t, &theme, 100);
            let chunks = cache.chunks();
            let t1 = Instant::now();
            let _ = wrap::wrap_for_viewport_windowed(
                &chunks,
                100,
                36,
                &ChatScroll::default(),
                None,
                &theme,
            );
            let t2 = Instant::now();
            warm_best = warm_best.min((t2 - t0).as_secs_f64() * 1e3);
            refresh_best = refresh_best.min((t1 - t0).as_secs_f64() * 1e3);
            wrap_best = wrap_best.min((t2 - t1).as_secs_f64() * 1e3);
        }

        // 旧全量路径对照（chat_lines_themed + 单片 wrap）。
        let mut oracle_best = f64::MAX;
        for _ in 0..5 {
            let t0 = Instant::now();
            let (lines, meta) = chat::chat_lines_themed(&state, flags, &o, &t, &theme);
            let _ = wrap::wrap_for_viewport(
                &lines,
                100,
                36,
                &ChatScroll::default(),
                &meta,
                None,
                &theme,
            );
            oracle_best = oracle_best.min(t0.elapsed().as_secs_f64() * 1e3);
        }

        // 完整 draw 计时（TestBackend 帧渲染；回显预算依据，design D0/
        // 5.3 本地计时注记——真实宿主链路回显并入 4.2 PTY 冒烟观察）。
        let backend = ratatui::backend::TestBackend::new(100, 40);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let ui = crate::app::view::ViewUi::default();
        let mut draw_best = f64::MAX;
        for _ in 0..5 {
            let t0 = Instant::now();
            terminal
                .draw(|frame| {
                    let _ = crate::app::view::render(frame, &mut state, &ui, &mut cache);
                })
                .unwrap();
            draw_best = draw_best.min(t0.elapsed().as_secs_f64() * 1e3);
        }

        // LayoutKey 变化（flags 翻转）→ 全量重建计时（暖 MD，5.3/4.2 对照）。
        let t0 = Instant::now();
        cache.refresh(
            &mut state,
            RenderFlags {
                show_thinking: !flags.show_thinking,
                ..flags
            },
            &o,
            &t,
            &theme,
            100,
        );
        let full_rebuild_ms = t0.elapsed().as_secs_f64() * 1e3;
        cache.refresh(&mut state, flags, &o, &t, &theme, 100); // 恢复键

        let mem = cache_memory_footprint(&cache);
        eprintln!(
            "[bench] entries={n_entries} cold={cold_ms:.1}ms warm_best={warm_best:.2}ms \
             (refresh={refresh_best:.2}ms wrap={wrap_best:.2}ms) draw_best={draw_best:.2}ms \
             full_rebuild={full_rebuild_ms:.1}ms oracle_best={oracle_best:.2}ms \
             cache_mem={:.1}MB",
            mem as f64 / (1024.0 * 1024.0)
        );
        assert!(
            warm_best < 5.0,
            "暖帧 {warm_best:.2}ms 应 < 5ms（best-of-10 宽松阈值）"
        );
        assert!(
            draw_best < 50.0,
            "完整 draw {draw_best:.2}ms 应 < 50ms（回显预算，5.3 注记）"
        );
        assert!(
            mem < 36 * 1024 * 1024,
            "缓存内存 {}MB 超 D6 预算 2 倍 → 应触发 P3 档 2",
            mem / (1024 * 1024)
        );
    }

    /// 4.2 常驻基准：长会话 + 流式增长尾槽的逐帧成本（design D3/D4：
    /// 流式尾条场景补原 bench 暖帧口径盲区）。背景 = 合成 1500 条/5.8MB
    /// bench 同款；尾条 = 40KB 互异代码块（~200B/delta，每 delta 一帧），
    /// 分段计时基线/流式/落盘，并每 25 delta 记 memo/oracle 等价计数注记
    /// （组 1 占位计数，组 2 接真断言）。
    /// 输入: 无（全部合成内容，无真实会话数据）
    /// 输出: [bench] 分段计时 + parity 计数 + memo/缓存内存注记 eprintln；
    ///       预算断言（design D3）：整帧 best-of-2 max < 5ms、memo/oracle
    ///       逐字节等价、memo 内存 < 2MB、缓存内存 < D6 预算 2 倍
    #[test]
    fn bench_streaming_tail_under_budget() {
        // 背景 = bench 同款合成 (约 1500 条/5.8MB, 与真实长会话 1496/5.9MB
        // 同量级)。真实会话日志是 agent 日志格式 (非 probe 包装), replay
        // 不认; 背景只需同量级条目/字节, 内容真实度由尾条负责。
        let mut state =
            replay_jsonl(std::io::Cursor::new(synthetic_jsonl().as_bytes())).expect("合成回放");
        let n_entries = state.entries().len();
        assert!(n_entries >= 1400, "背景应约 1500 条, 实得 {n_entries}");
        let (o, t, theme) = defaults();
        let flags = RenderFlags::default();

        // ---- 基线对照: 冷建 / 暖帧 / 完整 draw (镜像 bench) ----
        let mut cache = LayoutCache::default();
        let cold0 = Instant::now();
        cache.refresh(&mut state, flags, &o, &t, &theme, 100);
        let cold_ms = cold0.elapsed().as_secs_f64() * 1e3;
        let mut warm_best = f64::MAX;
        for _ in 0..10 {
            let t0 = Instant::now();
            cache.refresh(&mut state, flags, &o, &t, &theme, 100);
            let chunks = cache.chunks();
            let _ = wrap::wrap_for_viewport_windowed(&chunks, 100, 36, &ChatScroll::default(), None, &theme);
            warm_best = warm_best.min(t0.elapsed().as_secs_f64() * 1e3);
        }
        let backend = ratatui::backend::TestBackend::new(100, 40);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let ui = crate::app::view::ViewUi::default();
        let mut draw_warm_best = f64::MAX;
        for _ in 0..5 {
            let t0 = Instant::now();
            terminal
                .draw(|frame| {
                    let _ = crate::app::view::render(frame, &mut state, &ui, &mut cache);
                })
                .unwrap();
            draw_warm_best = draw_warm_best.min(t0.elapsed().as_secs_f64() * 1e3);
        }
        eprintln!(
            "[bench] base: entries={n_entries} cold={cold_ms:.1}ms warm_best={warm_best:.2}ms draw_warm_best={draw_warm_best:.2}ms"
        );

        // ---- 流式场景: 40KB 混合 markdown/代码块响应, ~200B/delta, 每 delta 一帧 ----
        state.handle(&json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        let mut source = String::new();
        let mut blk = 0usize;
        while source.len() < 40 * 1024 {
            source.push_str("这是一段**流式**正文，用于模拟真实长回答的排版成本。行内 `code` 与中文混排 wrap 压力。\n\n");
            blk += 1;
            // 各块内容互异: 真实流式里代码块持续变化, 高亮缓存逐帧 miss。
            source.push_str(&format!(
                "```rust\nfn demo_{blk}(x: usize) -> usize {{\n    // 块 {blk}: 内容互异\n    (0..x).map(|i| i * {blk}).sum()\n}}\n```\n\n"
            ));
        }
        let mut deltas: Vec<String> = Vec::new();
        let mut cur = String::new();
        for ch in source.chars() {
            cur.push(ch);
            if cur.len() >= 200 {
                deltas.push(std::mem::take(&mut cur));
            }
        }
        if !cur.is_empty() {
            deltas.push(cur);
        }

        // 惰性静态预热（syntect 语法集/主题集进程级一次性加载，约几十 ms，
        // 不属每帧成本；真实宿主在首次代码块前即已加载）。
        crate::app::markdown::render_markdown("```rust\nfn warm() {}\n```");

        // 单缓存探针 (dirty_from 是单缓存协议 — 双缓存会互相消费脏标记,
        // 误触"漏标降级全量重建", 首版量测教训)。计时口径（组 2 修正）:
        // 每 delta: handle -> [draw#1 计时 = 真实整帧（app 口径：单次 draw，
        // 内部 refresh 承担 memo 推进 + 尾窗）] -> [draw#2 计时 = 尾窗 +
        // wrap + paint，分段归因]，refresh(推进) = 整帧 - draw#2。旧口径
        // "refresh + draw 两段相加" 把尾窗重建双计（内部 refresh 并非空
        // 操作），故不再作整帧度量；两遍取逐 delta 最小值（best-of-2 宽松
        // 阈值防 CI 抖动，对齐 warm bench best-of-10 纪律）。
        let mut best_frame = vec![f64::MAX; deltas.len()];
        let (mut sum_refresh, mut max_refresh) = (0f64, 0f64);
        let (mut sum_draw, mut max_draw) = (0f64, 0f64);
        let (mut parity_ok, mut parity_checked) = (0usize, 0usize);
        let (mut memo_mem, mut mem_total, mut land_ms) = (0usize, 0usize, 0f64);
        for pass in 0..2 {
            let mut state = replay_jsonl(std::io::Cursor::new(synthetic_jsonl().as_bytes()))
                .expect("合成回放");
            state.handle(&json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
            let mut cache = LayoutCache::default();
            cache.refresh(&mut state, flags, &o, &t, &theme, 100);
            terminal
                .draw(|frame| {
                    let _ = crate::app::view::render(frame, &mut state, &ui, &mut cache);
                })
                .unwrap();
            for (i, d) in deltas.iter().enumerate() {
                let th = Instant::now();
                state.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":d}}));
                let handle_ms = th.elapsed().as_secs_f64() * 1e3;

                // 真实整帧（app 口径）。
                let t1 = Instant::now();
                terminal
                    .draw(|frame| {
                        let _ = crate::app::view::render(frame, &mut state, &ui, &mut cache);
                    })
                    .unwrap();
                let frame_ms = t1.elapsed().as_secs_f64() * 1e3;
                best_frame[i] = best_frame[i].min(frame_ms);

                // 分段摸底：尾窗 + wrap + paint（重复 draw，仅供归因注记）。
                let t2 = Instant::now();
                terminal
                    .draw(|frame| {
                        let _ = crate::app::view::render(frame, &mut state, &ui, &mut cache);
                    })
                    .unwrap();
                let draw_ms = t2.elapsed().as_secs_f64() * 1e3;
                let refresh_ms = (frame_ms - draw_ms).max(0.0);

                if pass == 0 {
                    sum_refresh += refresh_ms;
                    max_refresh = max_refresh.max(refresh_ms);
                    sum_draw += draw_ms;
                    max_draw = max_draw.max(draw_ms);
                }
                if pass == 0 && (i + 1) % 25 == 0 {
                    // 流式中间态等价计数（memo/oracle 逐字节，真断言）。
                    let (cl, cm) = flatten(&cache);
                    let (ol, om) =
                        chat::chat_lines_themed(&state, flags, &o, &t, &theme);
                    parity_checked += 1;
                    if cl == ol && cm == om {
                        parity_ok += 1;
                    }
                    eprintln!(
                        "[bench] delta#{:4} tail≈{:>6}B | handle={:.2} refresh(推进)={:.2} draw(尾窗+wrap+paint)={:.2} frame={:.2}ms | parity=memo/oracle {}/{}",
                        i + 1,
                        (i + 1) * 200,
                        handle_ms,
                        refresh_ms,
                        draw_ms,
                        frame_ms,
                        parity_ok,
                        parity_checked
                    );
                }
            }
            if pass == 1 {
                // 末遍收尾：memo/缓存内存注记（落盘前，含行级 memo 增量）
                // 与落盘整帧（message_end → 尾槽迁入正式条目的一次性重排）。
                memo_mem = cache
                    .inflight
                    .iter()
                    .map(|s| match s {
                        InflightSlot::Text(m) => m.memory_bytes(),
                        InflightSlot::Other { .. } => 0,
                    })
                    .sum();
                mem_total = cache_memory_footprint(&cache);
                state.handle(&json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":source}]}}));
                let t0 = Instant::now();
                terminal
                    .draw(|frame| {
                        let _ = crate::app::view::render(frame, &mut state, &ui, &mut cache);
                    })
                    .unwrap();
                land_ms = t0.elapsed().as_secs_f64() * 1e3;
            }
        }
        let best_max = best_frame.iter().cloned().fold(0f64, f64::max);
        let best_avg = best_frame.iter().sum::<f64>() / deltas.len() as f64;
        // 行级 memo 缓存内存注记（design D3：预期 <2MB；总量超 D6 预算
        // 2 倍 = P3 信号，与暖帧基准同口径）。
        let mem = mem_total;
        eprintln!(
            "[bench] stream: deltas={} tail={}B | frame(best-of-2) avg={:.2} max={:.2}ms | refresh(推进) avg={:.2} max={:.2}ms | draw(尾窗+wrap+paint) avg={:.2} max={:.2}ms | parity=memo/oracle {}/{} | memo_mem={:.1}KB cache_mem={:.1}MB",
            deltas.len(),
            source.len(),
            best_avg,
            best_max,
            sum_refresh / deltas.len() as f64,
            max_refresh,
            sum_draw / deltas.len() as f64,
            max_draw,
            parity_ok,
            parity_checked,
            memo_mem as f64 / 1024.0,
            mem as f64 / (1024.0 * 1024.0)
        );
        // 预算断言（design D3/D4）：流式尾条整帧 max < 5ms（现状 ~110ms
        // 红 → 转绿）；parity 计数接真断言（memo 路径逐字节等价）。
        assert_eq!(
            parity_ok, parity_checked,
            "流式中间态 memo/oracle 必须逐字节等价"
        );
        assert!(
            best_max < 5.0,
            "流式整帧 {best_max:.2}ms 应 < 5ms（best-of-2 宽松阈值防 CI 抖动）"
        );
        assert!(
            memo_mem < 2 * 1024 * 1024,
            "行级 memo 缓存 {:.1}KB 应 < 2MB（design D3 预期）",
            memo_mem as f64 / 1024.0
        );
        assert!(
            mem < 36 * 1024 * 1024,
            "缓存内存 {}MB 超 D6 预算 2 倍 → 应触发 P3 档 2",
            mem / (1024 * 1024)
        );

        // ---- 收尾: message_end 落盘 (尾槽 -> 正式条目) 后一帧（末遍内测） ----
        eprintln!("[bench] land: message_end 后整帧={:.2}ms", land_ms);
    }
}
