# Design

## Context

动机与范围见 proposal.md（Why/What Changes）。本文是
`Docs/transcript-incremental-rendering.md`（方案 B）的**修订版设计**：原方案的
D1–D6 骨架成立，但探索期核查发现三处事实错误与一个设计缺口，本文以其结论为准，
归档时同步回改原文档。

现状热路径（引用已核实，行号为核查基线）：

```
App::draw (src/app/mod.rs:3984)
 ├─ *state_slot = self.state.clone()      mod.rs:3989  ← 每帧深拷贝全文(含锁), 待删
 └─ view::render → render_chat            view.rs:63/357
      ├─ chat_lines_themed                chat.rs:56   ← 全量 markdown 排版 O(全文)
      └─ wrap_for_viewport                wrap.rs:154  ← 全文三遍: wrap / collect_hits / bg-pad
                                                        最后才 drain+truncate 切视口
```

核查钉死的变异模型（设计前提）：

- `transcript` **基本 append-only**：push 恒在尾部；`load_messages` 全量替换；
  **就地变异存在但低频**——`apply_tool`（`tool_end` 可晚于 `message_end` 乱序到达，
  reverse 扫 id 改已提交条目）、`apply_tool_result`（`turn_end` 回填）。原方案
  "中部插入（compaction Notice）"例子错误：Notice 一律尾部 push。
- **流式内容不在 `entries()` 内**：`message_update` 写 `state.inflight`
  （`as_assistant_mut` 全仓零调用，系死代码），连同 `bash_running` 尾字段在
  entries 循环**之外**渲染，是两个"易失尾槽"。
- `wrap_one(&sub, width, &mut out)` 逐行无上下文（已核实）——可视段 wrap 成立。
- `content_len` = **wrap 后**全文屏行数（wrap.rs:252 `next.content_len = wrapped.len()`），
  选区按绝对屏行寻址（`row = scroll.top + y`）——原方案 §6 遗留待核项就此关闭。
- `event_loop`（mod.rs:392，native/embed 共用）每轮 drain 全部积压、整批单次
  draw——队列天然有界，"事件积压死亡螺旋"机制不存在；真实病灶是单帧 O(全文)
  随内容增长（149→275ms）。

## Goals / Non-Goals

**Goals:**

- 每帧布局成本 O(新/变更条目 + 视口行)，冷首帧允许一次 O(全文)。
- **逐字节等价**：`lines`/`meta`/`logical_start`/`thinking_seq`/`msg_seq`/
  `wrap_for_viewport` 输出/`collect_hits` 区间/`ChatScroll` 回写与现全量路径
  相同（parity 单测为硬性验收）。
- 变更检测机制可证明不漏标（有安全网）。
- 层约束不破：session 不依赖终端，app 不做状态归约。

**Non-Goals:**

- 不做宿主侧滚轮事件合并（方案 A，归 balabala-agent-gui）。
- 不改 RPC 协议、JSONL 存储格式、`virtual_terminal`/ANSI 管线。
- 不做冷首帧分帧与 D6 内存上限第 2/3 档（P3 可选，按实测触发）。
- 不追查"卡死不自恢复"根因（另行立项）。

## Decisions

**D0 归因修正（验收指标随之改变）**
每帧 O(全文) 是病灶；合批机制已有。原方案"30 事件/s 稳态不积压"验收本来就被
现有合批满足，改为帧率/延迟指标：视口 36 行基准会话上 `chat_lines + wrap`
合计 < 5ms，单键回显 < 20ms（含宿主链路）。

**D1 条目级布局缓存（沿用原方案，补易失尾槽）**
`EntryLayout { lines, meta, row_base, thinking_base, msg_base, rev, wrapped_rows_per_line }`
按序与 `SessionState::entries` 对齐存于 app 层 `LayoutCache`；
`LayoutKey = (width, theme, flags)`。**新增**：`inflight` 与 `bash_running` 两个
易失尾槽不入缓存 Vec，每帧直接重建（单条消息成本，流式常态 O(1) 级）。
条目追加只排新增条目；流式变更恒在尾部。

**D2 序号/行号前缀语义（沿用原方案）**
全局计数改"条目内局部计数 + 基数组合"。`msg_seq` 的条件消耗（空条目不耗号）
是条目局部判定，与基数组合兼容。命中种属存**局部序号、读取时重定基**，
不做变异时绝对号重写（避免触碰未变条目）。

**D3 视口化 wrap（沿用原方案，补 bg-pad）**
wrap 计数前缀和（`wrapped_rows_per_line`）定位视口逻辑行区间 `[l0,l1)`，
只对区间调 `split_on_newlines` + `wrap_one`；bg 补齐逐行局部，只做窗口内；
`collect_hits` 保守对缓存 meta 全量扫（无分配无排版），P2 后按需再收窄。
`refresh_selection_doc` 的全文窗口需求保留全文 wrap 路径（见 D5）。

**D4 失效矩阵（补两行、修一行）**

| 变化 | 失效粒度 |
|---|---|
| 流式 token（message_update → inflight） | 易失尾槽常新，无需失效 |
| `tool_execution_*`（apply_tool 命中已提交条目） | 命中条目（通常尾部，偶发中部）→ `dirty_from = min(命中下标)` |
| `turn_end` 工具结果回填（apply_tool_result） | 同上 |
| 尾部 push（user/notice/bash/message_end） | 新增条目排版；**尾部窗口 K≥2 常新**（覆盖 tool_end 迟到把倒数第 2 条当目标的乱序） |
| `load_messages` / 会话重建 | 整缓存丢弃 |
| `show_thinking`/`expand_tools`/theme/width | 全部（入 `LayoutKey`） |
| `thinking_overrides`/`user_tool_expanded` | 受影响条目（条目级失效） |

**D5 selection_doc 投影（关闭原方案待核项）**
`content_len` 与 `selection_doc` 寻址均为 **wrap 后屏行**口径；D5 投影从缓存
`lines` 出发仍需一次 wrap 展开（便宜：split/wrap 无 markdown），行数与现
`wrapped.len()` 同口径，`lines.len() == content_len` 复用判据含义不变。
收益：省掉第二份 `chat_lines_themed`（markdown 排版），不省 wrap——频率低
（每手势一次），不追。

**D6 内存预算（沿用原方案档 1 先行）**
5.9MB 会话全量 `EntryLayout` 估算 6–18MB。P1 基准实测校准；超预算 2 倍
触发 P3 档 2（近窗口 ±500 条 + LRU）。
- **实测校准（4.1，合成 1500 条目 / 5.8MB 会话）**：**19.2MB**（含行、
  meta、wrap 计数与易失槽的近似足迹，不含分配器开销）——略超原估上限
  18MB、远低于 2 倍触发线 36MB → **档 2 不触发**；同基准暖帧 P1 实测
  465ms（旧全量 601ms）→ 剩余成本 76% 在全文 wrap，由 P2 视口化消化。

**D7 变更检测：`dirty_from` + `rev` 埋点（新增核心决策）**
`SessionState` 变异分类学五类（尾部 push / 全量替换 / 就地改条目 / inflight /
bash 尾字段）在 session 层以纯 usize 标记：

```
SessionState 新增字段（无新依赖, 不违层约束）:
  rev: u64              // 每次变异 ++
  dirty_from: usize     // 变异触及的最小条目下标 (usize::MAX=无; 易失/良性=len)
take_dirty(&mut self) -> (u64, usize)   // 读取即复位
```

- 埋点约 12 处（`transcript.rs` 的 push 系 / `apply_tool` / `apply_tool_result` /
  `flush_inflight` / `load_messages` / bash 系）。
- 每帧策略：前缀**全量重算**（纯整数加法，O(n)≈1500 次，可忽略）+ **重排版
  只做 `dirty_from..` 与尾部窗口 K=2 与易失尾槽**。
- **标记分级（选项 A，实现确认）**：条目变异 → 精确 `dirty=idx`；全量替换 →
  0；**易失尾（inflight/bash）与良性事件（phase/队列/名字等）→ 尾部脏
  `dirty=len`**（重排范围为空、对账非空）。由此“rev 变了 + 无脏”只剩真漏标
  （未来变异路径只 bump 不标）→ 对账降级全量：安全网不被良性事件误触，
  也不被流式帧打穿（流式帧 = O(1)）。
- **安全网**：`handle()` 出口统一 `rev++`；draw 帧尾对账 `rev` 与缓存记录不
  一致 → 全量重建（防漏标，debug_assert 提示）。
- 弃用备选：每帧内容指纹（O(全文) 哈希 3–6ms，吃掉 5ms 预算）；事件驱动
  失效表（app 层查表，表格漂移产生无声 stale 渲染）——后者仅可作精确化补充，
  不作主方案。

**D8 删除 `state_slot` 发布与 `session_state()`（BREAKING）**
三个零：宿主仓全源码零引用、本仓零调用、Docs/specs 零契约。成本：每帧持写锁
深拷贝整个 `SessionState`（5.9MB ≈ 5–15ms + 上万次分配），纯滚动帧照付，
P1 后将成单帧最大成本项。弃用备选：rev 门控（救不了流式帧，rev 恒变）；
Arc 快照（保一致仍须变更即克隆，只转移付费方）；惰性拉取/摘要类型（无消费
者无法定型，留作将来真需求出现时的重加路径，成本 ~10 行）。

**D9 层约束落位**
`EntryLayout`/`LayoutCache`（含 ratatui `Line<'static>`）放 **app 层**
（`chat.rs` 旁，建议 `src/app/layout_cache.rs` 新模块）；`transcript.rs` 只加
纯 usize 标记与 `take_dirty`。`app → session → rpc → pi` 单向依赖不变。

## Risks / Trade-offs

- [漏标变异 → stale 渲染] → `handle()` rev++ 全量重建安全网 + 变异谱全覆盖的
  parity 单测 + debug_assert 对账。
- [tool_end 迟到乱序命中倒数第 2 条] → 尾部窗口 K≥2 常新 + `apply_tool` 命中
  点精确埋 `dirty_from`。
- [缓存内存 6–18MB] → 档 1 先实测，超预算 2 倍触发 P3 档 2；不留隐性增长。
- [parity 测试与实现漂移] → 缓存路径以全量路径为 oracle 的对比测试随 P1 提交，
  覆盖：空会话/单条目/流式中途/override toggle/宽度变化/中部变异。
- [基准断言 CI 抖动] → 宽松阈值 + 本地基准脚本化，阈值不进硬性 CI 门。
- [删除 `session_state()` 波及未知第三方宿主] → git 依赖已核实零调用；归档
  注记 + CHANGELOG 一句话。
- [流式期间 dirty 恒为尾部] → 预期行为（恰是设计目标）；若实测出现非尾部高频
  变异，重估尾部窗口与埋点粒度。

## Migration Plan

1. **P1**：D7 变更检测 + D1/D2 缓存 + D8 state_slot 删除 + parity 单测与基准
   ——独立可回归，性能收益即达（viewport wrap 仍是全文，但只算一次）。
2. **P2**：D3 视口化 wrap（`wrap.rs` 新增"按前缀和切逻辑行区间"入口，全文路径
   保留供选区）。
3. **P3（可选）**：冷首帧分帧 + D6 档 2/3。

每阶段 `cargo test` + `cargo clippy --all-targets` 全绿（AGENTS.md 强制）；
按阶段独立 commit，回滚 = 剥离对应 commit。归档时同步修订
`Docs/transcript-incremental-rendering.md`（机制归因 + 验收指标 + 本文新增决策）。

## Open Questions

- "卡死不自恢复"根因（机制上非本仓队列积压；另行立项复现，不影响本设计）。
- 5.9MB 基准会话的实际缓存占用（P1 基准出数后校准 D6 档位，不改设计）。
