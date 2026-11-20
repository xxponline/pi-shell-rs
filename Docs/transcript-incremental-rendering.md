# 长会话渲染优化方案：transcript 增量排版（方案 B）

> 状态：**已实施**（change `optimize-long-session-rendering`，2026-11-20 归档；
> 暖帧实测 1.06ms vs 旧全量 600ms = **566×**，验收数据见 §3 修订）。
> 原始归因（2026-11-20 由宿主仓 `balabala-agent-gui` 性能验收产出；宿主侧管线
> 分段实测已排除，证据见 §1）保留，其中“死亡螺旋”机制经实施期核查**修正**为
> 每帧 O(全文)（见 §1 修订注）。
> 关联：`Docs/rust-tui-architecture.md`、`Docs/library-and-replay.md`（离线回放
> 可作本方案的基准数据源）、`openspec/specs/`（tui-app 滚动/渲染既有规格不因本
> 方案改变——本方案是等价重构，可见行为不变）
> 参照本仓 `AGENTS.md` 约定：中文函数注释（功能/输入/输出三要素）、
> `cargo test` + `cargo clippy --all-targets` 全绿后提交。

---

## 1. 问题与证据

**触发场景**：宿主 GUI 中打开一个长会话后（滚动/打字）严重卡顿，滚轮可复现
"完全卡死、不自恢复"。

**长会话实测**（宿主机器）：

```
~/.pi/agent/sessions/--Users-b.f.peng-Documents-Projects-balabala-agent-gui--/
  2026-11-19T09-02-49-512Z_01a1f30b-58a7-714a-a875-a6608c59673b.jsonl
  5.9 MB / 1496 条记录 / 平均单条 4 KB / 最大单条 51 KB
```

**宿主分段实测**（term-rate 全链日志，同目录会话）：

| 段 | 实测 | 归属 |
|---|---|---|
| 输入→帧（含本仓渲染） | 149 → 166 → 202 → **275ms**（重帧爬升） | **本仓每帧全量排版** |
| 帧大小 | ≤10 KB（视口增量 ANSI，无洪水） | 健康 |
| 帧→画（宿主 paint） | 4–15ms | 宿主已排除 |
| 喂帧解析 | ≤0.5ms | 宿主已排除 |
| 链路丢失 | pushed≈fed≈fed，dropped=0 | 宿主已排除 |

**机制归因（实施期修订）**：原归因“每事件一次全量重排版 → 队列积压死亡
螺旋”**不成立**——`event_loop`（`src/app/mod.rs:392`，native/embed 共用，
2026-11-05 首版即有）每轮把通道 **drain 合批、整批只画一次**，队列稳态有界
（≈ 到达率 × 帧时长）；宿主侧帧通道亦有界且丢帧计数，两侧均无螺旋。
真实病灶是**每帧 O(全文) 随内容增长**：

```
每帧 = chat_lines_themed O(全文) + wrap_for_viewport 全文三遍 + 每帧深拷贝 SessionState
  → 输入→帧 149→275ms 随会话增长（上表实测）
  → 稳态 ~3.6fps + ~275ms 回显 = 体感“卡死”（非队列积压）
  → 原“滚 10 秒积压 50 秒”推算作废；“不自恢复”未在本仓机制内找到对应，
    另行立项（见 change 的 Non-Goals）
```

---

## 2. 现状热路径：每帧 O(全文) 三遍（+ 每帧深拷贝）

```
App::draw (src/app/mod.rs ~3984)
  ├─ *state_slot = self.state.clone()   ← 补漏：每帧深拷贝全文给宿主（本变更已删除）
  └─ view::render(frame, &self.state, &ui)          src/app/view.rs:63
       └─ render_chat                                src/app/view.rs:357
            ├─ chat_lines_themed(state, ...)         src/app/view.rs:372 -> src/app/chat.rs:55
            │    for entry in state.entries()        ← 全部 1496 条逐一
            │    ├── markdown 块解析/换行             (src/app/markdown.rs)
            │    │    (syntect 高亮有块级缓存 ✓)
            │    └── 组装全文 Vec<Line> + meta
            └─ wrap_for_viewport(全文 lines, ...)    src/app/view.rs:375 -> src/app/wrap.rs:154
                 for line in lines.iter()            ← 第二遍全文 (拆行/换行)
                 collect_hits(全文 meta)             ← 第三遍全文扫描
                 bg 补齐(全文)                         ← 补漏：面板底色 pad 也扫全文
                 ... 最后才 drain(..top)/truncate(height) 切视口 36 行
```

另有**第二个全文构建器**：`refresh_selection_doc`（`src/app/mod.rs:802`，选区
文本提取用）——同样调 `chat_lines_themed` + `wrap_for_viewport`，按
`content_len` 等长缓存复用（append-only 同长即同文），`clear_selection` 手势
起始清空后重建一次。两个构建器应共享同一份缓存（§4 D6）。

关键事实（方案成立的前提，均已核实）：

- `entries()` 返回 `&[Entry]`（`src/session/transcript.rs:366`），条目基本
  **append-only**；**修正**：流式内容不在 entries 内——`message_update` 写
  `state.inflight`（在 entries 循环**之外**渲染；`as_assistant_mut` 全仓零
  调用，系死代码），另有 `bash_running` 易失尾字段；就地变异仅 `apply_tool` /
  `apply_tool_result`（tool_end 乱序晚于 message_end，通常尾条、偶发中部）；
- `wrap_one(&sub, width, ...)` **逐行无上下文**（不依赖前一行状态）→ 换行可
  只算可见段，前提是先知道"哪些逻辑行落进视口"（需要 wrap 计数前缀和，D3）；
- `wrap_for_viewport` 的窗口化（design D3 行号=绝对行）发生在**末尾**，
  导致前两遍无法省略——这是要改的结构。

---

## 3. 目标与验收指标

- 每帧成本 = **O(新/变更条目 + 视口行)**；`content_len`/滚动锚点/命中区间
  的语义（绝对行号、design D2/D3）**逐字保持**（等价重构，无可见行为变化）。
- 用 §1 的 5.9MB 会话作基准（可用 `Docs/library-and-replay.md` 的
  `replay_jsonl` 离线重建 state，免开真终端）：
  - 单帧 `chat_lines + wrap` 两段合计 **< 5ms**（视口 36 行量级；实施后
    实测暖帧 **1.06ms**，旧全量路径 600ms → **566×**）；
  - **帧率/回显口径（替代原“30 事件/s 不积压”——合批已有，该指标恒真）**：
    完整 draw **< 20ms**（实测 2.60ms @TestBackend）；单键回显 < 20ms
    （PTY 实测 PageUp/打字 3.3–4.6ms）；
  - 冷首帧允许 O(全文) 一次（实测 585ms @5.8MB 合成；P3 分帧未触发）。
- `cargo test` + `cargo clippy --all-targets` 全绿（AGENTS.md 强制）。

---

## 4. 设计

### D1 条目级布局缓存（核心）

```rust
/// 单条目的一次布局产物 (仅在自身内容或渲染键变化时重建)。
struct EntryLayout {
    lines: Vec<Line<'static>>,        // 该条目全部逻辑行 (未 wrap)
    meta: Vec<Vec<HitKind>>,          // 逐逻辑行命中种属
    row_base: usize,                  // 前缀: 该条目首逻辑行的全局行号
    thinking_base: usize,             // 前缀: thinking 全局序号基数
    msg_base: usize,                  // 前缀: 语义消息序号基数
    rev: u64,                         // 条目内容版本 (streaming 每次变更 ++)
}

struct LayoutCache {
    key: LayoutKey,                   // (width, theme, flags) — 见 D4
    entries: Vec<EntryLayout>,        // 与 SessionState::entries 同序
    total_rows: usize,                // wrap 前逻辑行总数 (前缀和末项)
}
```

- 变更点之后的条目**顺移重编号**：`row_base/seq 基数` 从上一条目的末项推出，
  增量维护 O(变更点之后的条目数)。**流式变更永远在尾部 → O(1)**；
  中部插入（compaction Notice 等）只重算其后的前缀，不重排版。
- 命中即免：稳态滚动/打字期间缓存命中率 = 全部，仅新 token 所在的
  Assistant 条目 `rev` 变化 → 单条目重建。

### D2 序号与行号的前缀语义

`chat_lines_themed` 内的 `thinking_seq`/`msg_seq` 是跨条目全局计数、
`logical_start` 是全局行号——缓存后改为**条目内局部计数 + 条目基数组合**
（D1 的 `*_base` 字段），查询时对 `entries` 求前缀（可 O(log n) 二分）。
约束：`Entry::Assistant { turn }` 单调不变、条目同序 ⇒ 基数组合与现全量
逐字节等价（parity 单测保证，§6）。

### D3 视口化 wrap（第二遍 O(全文) → O(视口)）

1. 由缓存前缀和求出**视口窗口覆盖的逻辑行区间** `[l0, l1)`（屏幕行 top →
   逻辑行的映射用每逻辑行 wrap 计数前缀和：`EntryLayout` 内再存
   `wrapped_rows_per_line: Vec<u32>` 或按条目粒度粗定位后逐行精定位）；
2. 只对 `[l0, l1)` 调 `split_on_newlines` + `wrap_one`（逐行无上下文已核实 ✓）；
3. `collect_hits` 改为对**窗口内 + 命中区间扫描所需范围**计算——保守做法：
   hits 仍可对缓存的 meta 全量扫（O(条目数) 但无分配/无排版，远小于行构建），
   或滚动停止时异步重建，P2 再优化；
4. `refresh_selection_doc` 的"全文窗口"需求（选区绝对行号）继续走**全文
   wrap**，但复用 D1 缓存的 lines——它有自己的 content_len 缓存，频率低
   （每手势一次），不在热路径。

### D4 失效矩阵

| 变化 | 失效粒度 |
|---|---|
| Assistant 流式 token（message_update） | 单条目（`rev++`，尾部） |
| `show_thinking` / `expand_tools`（RenderFlags） | 全部（flags 入 `LayoutKey`；低频） |
| `thinking_overrides` / `user_tool_expanded`（单条目 toggle） | 受影响条目（override 键携带 rev 或按条目失效——两种里取**条目级失效**，简单且低频） |
| 宽度变化（终端 resize） | 全部重建一次（width 入 `LayoutKey`；滚动中 resize 罕见） |
| theme 注入 | 全部重建一次（低频） |
| compaction / Notice 中部插入 | 变更点之后前缀重编号 + 新条目布局（不重排未触条目） |
| 会话重建 / `clear_selection` | 整缓存丢弃（沿用现有语义） |

### D5 与 selection_doc 的共享

`refresh_selection_doc` 改为从 `LayoutCache` **投影纯文本**（`Line` 拼接为
提取串），删除它自己的第二份 `chat_lines_themed + wrap_for_viewport` 调用；
`content_len` 相等即复用的现有缓存语义保留（它已经工作）。

### D6 内存预算与上限

全量 `EntryLayout` ≈ 全文逻辑行数 × `Line<'static>` 成本：5.9MB 文本约
6–18MB（估算，实施时用基准实测）。上限策略（按序启用）：
1. 不做——桌面场景可接受（先实测）；
2. 仅缓存近窗口条目（如视口 ±500 条）+ LRU 重建——滚动到远处付一次重建；
3. 只缓存 `meta/计数`，`lines` 由块级 AST（解析结果）二级缓存拼出。
   （实测校准：19.2MB @1500 条目/5.8MB——超原估上限 18MB、低于 2 倍触发线
   36MB → 档 2 不触发）

### D7 变更检测（实施期新增决策，原方案缺口）

原方案 D1 的 `rev` 字段没有“谁来 ++”的机制——实施期补齐为 session 层
`SessionState { rev: u64, dirty_from: usize }` + `take_dirty()`（纯 usize，
不违层约束），约 14 处变异点埋标（push 系 / `apply_tool` / `apply_tool_result` /
`flush_inflight` / `load_messages` / bash 系）：

- **标记分级（选项 A）**：条目变异 → 精确 `dirty=idx`；全量替换 → 0；
  **易失尾（inflight/bash）与良性事件（phase/队列/名字）→ 尾部脏 `dirty=len`**
  （重排范围为空、对账非空）——由此“rev 变了 + 无脏”只剩真漏标；
- **安全网**：`handle()` 出口统一 `rev++`；draw 帧尾对账“rev 变而无脏” →
  降级全量重建（`miss_rebuilds` 计数 + debug_assert 提示，不 panic 保可测）；
- 尾部窗口 K≥2 常新（tool_end 乱序兑底）；前缀基数每帧整数重算 O(n)。

详见 change `optimize-long-session-rendering` 的 design.md D7。

---

## 5. 实施阶段

> P1/P2 已随 change `optimize-long-session-rendering` 完成（2026-11-20）；
> P3 未触发（D6 实测 19.2MB < 36MB 线、冷首帧 585ms 可接受）。

- **P1（最高性价比）**：D1 + D2 —— `chat.rs`/`transcript.rs` 落条目缓存与
  前缀，`view::render_chat` 读缓存；parity 单测先行。
- **P2**：D3 视口化 wrap —— `wrap.rs` 增加"按前缀和切逻辑行区间"的入口，
  `wrap_for_viewport` 保留全文路径供 selection 用。
- **P3（可选）**：冷首帧分帧（首帧只排前 N 屏，后台补全）+ D6 上限策略。
- 每阶段独立可回归：parity 单测 + `cargo test` + `cargo clippy --all-targets`。

## 6. 正确性风险与验收清单

- **Parity 单测（硬性）**：同一 `SessionState` 输入，缓存路径 vs 现全量路径，
  断言 `lines`、`meta`、`logical_start`、`thinking_seq`/`msg_seq` 编号、
  `wrap_for_viewport` 输出、`collect_hits` 区间、`ChatScroll` 回写**逐字节相等**；
  覆盖：空会话/单条目/流式中途/override toggle/宽度变化/中部插入。
- **滚动绝对行号**（design D2/D3）：`scroll.content_len`、`top`、贴底判定
  与现状一致——验收用 `replay_jsonl` 回放 §1 的 5.9MB 会话做前后对比。
- **selection_doc**：`lines.len() == content_len` 复用判据在 D5 投影后含义
  不变（**已核对**：现口径 = wrap 后全文行数 `wrapped.len()`；D5 投影保持
  同口径，判据未改）。
- **streaming 期间的 rev 竞态**：单线程 draw（ratatui 闭包）内不存在并发，
  但 `message_update` 与 draw 的先后会导致 rev 断言差异——测试用回放序列
  固定顺序。
- 基准脚本化：把 §3 指标做成 `cargo bench` 或 `#[test]` 计时断言（宽松阈值
  防 CI 抖动），随 P1 提交。

## 7. 不在本方案内

- 宿主 GUI 侧的"滚轮事件合并"（方案 A）——可选的补充缓解，归
  `balabala-agent-gui` 仓，本方案是根治，做了 A 可不做。
- RPC 协议、会话存储格式（JSONL schema）不动。
- `virtual_terminal`/ANSI 输出管线不动（输入帧仍走现有 embed 通路）。
