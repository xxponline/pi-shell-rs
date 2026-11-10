# Design

## Context

动机见 proposal.md。现状约束（决定方案形态的实测事实）：

- 命中管道（`add-tool-card-click` 刚落地）：`chat_lines` 产出
  `meta: Vec<Option<String>>`（工具行按 `result.is_some()` 打 toolCallId）
  → `wrap_for_viewport` 扫出 `Vec<CardHit{start,end,id}>`（绝对屏行）→
  `handle_click`/`card_at` 线性命中后相对翻转 `user_tool_expanded`。
- thinking 渲染（`chat.rs` append_blocks）：非空块两态——`show_thinking`
  全局展开（斜体全文）或单行占位 `<thinking N chars>`；空块跳过。
- 全局开关 `KeyIntent::ToggleThinking` 只翻 `show_thinking`，无单块状态。
- **键稳定性相关**：`entries()` 只含已落盘条目，流式块在独立
  `in_flight()`，由 `chat.rs` 在条目循环**之后**作为尾段渲染；
  `message_end → flush_inflight → ensure_assistant_entry()` 把尾段并入
  （可能落在**早先已存在**的 assistant 条目位）；`Notice` 条目可在流式中
  追加；**transcript 整体重写点唯一** = `load_messages`
  （compaction 不重写本地 transcript）。
- pi 参照：`hidden = overrides.get(runIndex) ?? hideThinkingBlock`；
  左键 `set(runIndex, !hidden)`；`setHideThinkingBlock()` 先
  `overrides.clear()` 再重绘（interactive-mode/assistant-message 源码实测）。

## Goals / Non-Goals

**Goals:**

- 逐块左键展开/收起，与工具卡片同一分发链路（`UiEvent::Click` →
  `handle_click`），事件层零改动。
- Ctrl+T 保持既有全局语义并补"清单块状态"（pi parity）。
- 单块键在 流式→落盘→后续追加 全程稳定；会话切换不串态。

**Non-Goals:**

- settings 持久化/可配置（proposal 已撤回）；连续 thinking 块合并 run；
  折叠标签文案、展开态 markdown、空行（needsSpacer）规则、状态提示语。
- 不改 session 层（`Block::Thinking` 结构、transcript 归约均不动）；
  不改 `event.rs`；不动工具卡片既有语义与覆盖表。

## Decisions

### D1 命中表强类型泛化（一张表、带 kind）

`wrap.rs`：

```rust
pub enum HitKind {
    Tool(String),     // toolCallId（既有语义）
    Thinking(usize),  // thinking 全局渲染序号（D2）
}
pub struct Hit { pub start: usize, pub end: usize, pub kind: HitKind }
```

`CardHit/card_hits/card_at/collect_card_hits` 相应更名
`Hit/hits/hit_at/collect_hits`（机械替换，名副其实）。

- **替代 (b) 字符串键前缀**（`"t:3"` vs uuid 共用 `String`）：否——
  stringly-typed，分发时靠约定判别；项目规范要求"弱类型边界集中判别、
  输出用强类型"。
- **替代 (c) 工具/思考两张并行表**：否——双字段、双扫描；一表一线性扫、
  分发放到 `match kind` 处最简，且两域共享行号/视口/浮层规则。

### D2 thinking 键 = 全局渲染序号（`usize`，空块也计数）

`chat_lines` 建 `let mut thinking_seq = 0usize`，以 `&mut` 穿透所有
`append_blocks` 调用（已落盘逐条目 + `in_flight` 尾段）；每个
`Block::Thinking` **无条件 `+= 1`（含空块）**，仅非空块的行打
`HitKind::Thinking(seq)` 标记。

- **替代 (a) `(条目序, contentIndex)` 复合键**：否——`in_flight` 尾段不是
  条目，渲染期只能以"将来的条目位"估键；`flush_inflight` 经
  `ensure_assistant_entry` 可能把内容并回**早先**条目位，流式中若追加过
  `Notice` 等条目则条目序号漂移 → 覆盖悬空/错块。全局 thinking 序号
  不受条目插入影响（Notice 等非 thinking 不消耗序号、flush 不改 thinking
  相对序）。
- **空块也计数**：`thinking_start` 先建空块、后接 delta 变非空——若"只数
  非空"，空→非空瞬间其后所有键整体 +1 错位；无条件计数免疫。
- **替代 (c) 内容哈希键**：否——流式 delta 持续改内容，键每帧变。
- 稳定性全链：流式只追加 → 既有序号不变；落盘不改相对序；唯一重写点
  `load_messages` 清表（D6）。

### D3 生效值与 Ctrl+T 清覆盖

`App` 增 `thinking_overrides: HashMap<usize, bool>`；生效值 =
`override ?? show_thinking`（与工具卡片 D1 完全同构）。`ToggleThinking`
在翻转 `show_thinking` 后 **`thinking_overrides.clear()`** + log——对应 pi
`setHideThinkingBlock()` 的 clear 语义；无覆盖时行为与现状不可区分
（向后兼容），有覆盖时由 spec「全局切换覆盖单块状态」钉住。

### D4 分发：`handle_click` 单点 match

浮层早退（既有）→ `hit_at(y)`（原 `card_at` 行号逻辑原样）→ `match kind`：
`Tool(id)` 走既有工具覆盖分支；`Thinking(seq)` 相对翻转写
`thinking_overrides`（log 注明块序号）。两分支同为"相对当前生效值取反"，
返回值语义不变（true=命中需重绘）。

### D5 渲染链：并行参数而非打包结构

`ViewUi` 增 `thinking_overrides: Option<&HashMap<usize, bool>>`
（`derive(Default)` 不破）；`render_chat` `unwrap_or(&empty)` 传入；
`chat_lines` +1 参、`append_blocks` +2 参
（`thinking_overrides`、`thinking_seq`）。

- **替代：打包 `Overrides<'a>{tools,thinking}` 结构**：否——ViewUi 已是
  分离字段风格（`tool_expanded` 先例），并行参数 diff 更小、逐参语义直白。
  参数数到达 7 个是已知代价，可接受。

### D6 会话切换清 thinking_overrides（`AsyncKind::Messages` 臂）

`load_messages` 完成处（现有 `scroll = ChatScroll::default()` 同臂）追加
`thinking_overrides.clear()`。理由：序号键**跨会话必撞**（与 uuid 工具 id
不同，旧条目残留无害的论证不成立）；且 `load_messages` 是 transcript
唯一整体重写点，单点覆盖 boot/resume/切换全部路径。工具覆盖表不清
（uuid 不撞、既有行为，非本 change 范围）。

### D7 流式与点击的帧一致性（沿用工具卡论证）

点击永远用"上帧命中表 + 上帧 `scroll.top`"：事件循环串行，表与画面同帧
更新；thinking delta 只向既有块追加 → 既有绝对行不移位；`in_flight` 尾段
（`chat.rs:70`）与已落盘条目同表同扫描。流式中点击（spec 场景 3）由此
自然成立，由单测 + PTY 钉住。

## Risks / Trade-offs

- [序号键依赖"thinking 渲染序 append-only"] → 当前 transcript 无中段
  插入 thinking 的路径（唯一重写点已清表）；设计记录该假设，任务含
  `load_messages` 清表测试防回归。
- [meta 类型改动波及 chat/wrap/view/mod 四处签名] → 纯机械连锁，
  既有命中测试同步更名适配，全量测试护航。
- [Ctrl+T 清覆盖是既有行为的新增副作用] → 无覆盖时不可观察；有覆盖时
  spec 场景 + 单测双钉。
- [流式中点击的时序边界] → PTY 场景 3（点击后继续 delta 仍展开）验证。

## Migration Plan

纯新增行为，无数据/协议迁移；随归档同步 tui-app 主规格（ADDED +1）。
回滚 = 还原本 change 提交，无状态遗留（覆盖表为内存态）。

## Open Questions

（无——PTY 坐标与命中细节在 tasks 阶段定。）
