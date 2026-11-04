# Design

## Context

路线图阶段 0+1。当前唯一的 Rust 代码是 `src/bin/probe/`（约 1750 行），它把
"连接 pi / 收发行 / 场景驱动 / 产物落盘"全写在一个二进制里，模块对其它 bin 不可见。
探针已经验证了协议（见 `Docs/rpc-protocol-notes.md` 第 8 节）：

- stdout 混合 response（带 id）/ 事件（无 id）/ extension_ui_request / extension_error。
- `message_update` 在线上只带 delta（`message` 与 `partial` 被剥离），客户端必须按
  `contentIndex` 自己累积；`message_end` 给权威完整消息。
- assistant 内容块有 text / thinking / toolCall 三类；一次 run 可有多 turn、
  多 thinking 块。
- 已提交 4 个真实 snapshot（text_only / tool_call / failed_tool / extension_ui），
  可离线回放。

本变更产出两个解耦层：`rpc`（连接）与 `session`（无头状态），probe 退化为
"`rpc` + 场景/产物"的消费者。不引入终端库与 async runtime。

## Goals / Non-Goals

**Goals:**
- 建 lib crate，把可复用连接代码从 probe 私有模块迁到 `src/rpc/`。
- 提供会话化的 `rpc::client`：发送命令并按 id 等响应，同时把事件分流给订阅者。
- 实现 `session`：输入事件流，输出结构化 transcript + 运行态；可从 snapshot 离线回放。
- probe 改用库，外部行为/产物/测试保持不变。
- 新增离线回放测试，不启动 pi、不烧 token。

**Non-Goals:**
- 不做任何 ratatui/crossterm 渲染、不做编辑器、不做弹层（阶段 2+）。
- 不做 30 个命令的完整强类型 builder；只提供通用"发送命令对象 + 等响应"，
  以及状态机实际需要的最小命令封装。
- 不改 probe 的场景集、fixture 扩展、产物目录格式。
- 不实现会话树/fork/resume 的状态建模（阶段 4）。
- 不决定主 TUI 的并发模型（std vs tokio 留到阶段 2）。

## Decisions

### 1. crate 布局：单 crate 多模块，probe 作为 bin 消费库

```
src/
├── lib.rs                 # pub mod rpc; pub mod session;
├── rpc/
│   ├── mod.rs             #   RpcClient (公共门面)
│   ├── process.rs         #   从 probe 迁入：Transport/三线程/spawn
│   └── jsonl.rs           #   从 probe 迁入：WireMessage/OutgoingCommand/Direction
├── session/
│   ├── mod.rs             #   SessionState + handle(event) + transcript()
│   ├── blocks.rs          #   TextBlock/ThinkingBlock/ToolCallBlock + 累积
│   ├── transcript.rs      #   Entry(User/Assistant/Tool/Notice) 有序条目
│   └── replay.rs          #   从 JSONL 读 in 方向 raw 事件回放入口
└── bin/
    ├── probe/...          #   改为 use pi_rust_tui::{rpc, session::classify...}
    └── (tui 本变更不建)
```

probe 现有的 `classify.rs` 属于"计数/诊断"，迁到 `rpc` 下不合适；它只被探针
summary 使用，因此**保留在 probe bin 内**（或迁为 `session::classify` 仅当
状态机也需要判别时）。初版保持在 probe 内，避免为复用而扩散。

`Transport` 与 `RpcClient` 的边界：
- `Transport`（迁自 process.rs）：只管"一条命令行进去 / 一行 WireMessage 出来
  + stderr"，是通用双向管道，不懂 id 路由。
- `RpcClient`（新，薄）：在 Transport 上提供 `send_command(value) -> 等待 id 响应`
  与一个事件迭代/接收通道；它复用 probe scenario.rs 已验证的
  "HashMap<id, oneshot>" 思路，但从"场景步骤"里解耦成可独立使用的 API。

**备选**：直接让 TUI 复制 probe 的 process.rs —— 被否，正是本变更要消除的重复。

### 2. 状态机：显式 `handle(event)` 归约，不持有子进程

`SessionState` 是纯数据 + 纯归约：

```
SessionState {
  phase: Idle | Streaming,
  thinking_level, session_name,
  queue: { steering: Vec<String>, follow_up: Vec<String> },
  usage: Option<Usage>,
  transcript: Vec<Entry>,       // 有序、只追加(允许权威校正最后一条 assistant)
  inflight: 流式累积临时区
}

fn handle(&mut self, event: &Value)        // 接受弱类型 JSON，内部判别
```

输入用 `serde_json::Value`（而非为全部事件生成强类型 enum），原因：
- 线协议随 pi 演进，状态机只需要少数字段；未知事件要能安全忽略。
- 与 probe"弱类型、原始体保留"的既有取向一致，降低迁移面。
判别用顶层 `type` 字符串；需要二级判别的（message_update）再下钻。

### 3. assistant 累积：按 message 生命周期建临时区，message_end 校正

- `message_start`：若 role=assistant，开一个 `InflightAssistant { blocks: Vec<Block> }`；
  system/user 不入 transcript 累积区（user 在 message_start/message_end 时作为
  用户条目记录；system 忽略）。
- `message_update`：按 assistantMessageEvent.type：
  - `*_start`：在 `blocks[contentIndex]` 占位（text/thinking）；
    `toolcall_start` 时从顶层取 `id`、`toolName` 建 toolCall 块。
  - `*_delta`：向对应块追加字符串（toolCall 追加 args JSON 串）。
  - `*_end`：用完整 content 校正该块。
  - `done`：标记结束原因。
- `message_end`：用权威 `message.content` 重建 blocks，整体替换 inflight，
  然后把该 assistant 作为条目落进 transcript（或替换已占位的同 id 条目）。
- `turn_end`：携带最终 message 与 toolResults，作为又一次权威校正来源；
  toolResults 用于工具结果的权威补全（与 tool_execution_end 二选一更全者合并）。
- `agent_end`：`messages` 是本 run 全量数组——用它做最终一致性校验/补漏，
  但**不**当作跨 run 历史（笔记已确认它不是跨 run 的）。

内容块模型：

```
enum Block { Text{ text }, Thinking{ text }, ToolCall{ id, name, args_json, status, is_error, result } }
Entry::Assistant { blocks, usage, turn }
Entry::User { text | images.. }
Entry::ToolNotice / 或并入 Assistant.ToolCall（实现时二选一，倾向并入 ToolCall 块）
Entry::Notice { kind: Compaction|AutoRetry|Queue|..., text }
```

工具执行与 toolCall 的关联键是 `toolCallId`；`tool_execution_start` 在对应
assistant 块存在时挂到该块，若先于块到达则进一个 `pending_tools: by_id` 暂存，
块出现时合并。tool_execution_update 的 partialResult 存为"进行中预览"，
end 用 result 定稿。

### 4. replay：直接消费 probe transcript 文件格式

probe 落盘每行 `{seq, ts_ms, dir, raw}`。replay 逐行解析，只取 `dir=="in"` 的
`raw` 调 `state.handle()`。这样**状态机测试输入 = 已归档快照**，不需要额外夹具，
也验证了 probe 产物的可回放契约（呼应 rpc-probe 规格的"归档产物可被测试加载"）。

### 5. probe 迁移策略（保行为）

分步、每步跑测试：
1. 建 `src/rpc/{process,jsonl}.rs`，从 probe 移动代码并改成库 pub 接口。
2. probe 删除私有文件，`use pi_rust_tui::rpc::{...}`；`cargo test` 必须仍 13 过。
3. 新增 `RpcClient`；probe 的 scenario runner 改为基于它（或先保留 runner、
   仅把底层 Transport 换成库——**优先小改**：本变更只要求 probe 复用 rpc 连接层，
   scenario 编排留在 probe；RpcClient 作为库 API 供未来 TUI，用单测覆盖即可）。
4. artifact/scenario/scenarios 仍留在 bin。

为降低风险，RpcClient 与 probe scenario runner 允许短期并存：runner 直接用
`Transport`（库）也满足"复用连接层"，RpcClient 的正确性由库内单测保证，
不强行重写 runner。

## Risks / Trade-offs

- **[迁移导致 probe 回归]** → 先纯移动并保持 13 测试通过，再叠加 RpcClient；
  移动与新增分开提交，产物格式不变。
- **[增量累积与权威消息不一致]** → message_end / turn_end / agent_end 三处
  权威校正；测试用真实 snapshot 的最终字段断言，而非只断言 delta 拼接。
- **[tool 事件早于/晚于 toolCall 块]** → pending_tools 暂存 + id 关联合并。
- **[follow_up/steer 多 turn 缺少 snapshot]** → 现有 4 个 snapshot 不含多 turn
  完整样本；状态机的多 turn 逻辑用构造事件单测保证，如需要再补跑一次
  follow_up snapshot（探针已支持，属数据补充，不改规格）。
- **[弱类型 Value 丢失编译期保证]** → 在 session 内集中判别并提供强类型的
  Block/Entry 输出，把不稳定性限制在 handle 的边界。
- **[system 消息的 sections 很大]** → 状态机忽略 system 内容，不进 transcript。

## Migration Plan

纯内部重构 + 新增模块，无外部用户。顺序：建 rpc 库并迁移（保测试）→ 加
RpcClient 单测 → 实现 session 归约 → 加 snapshot 离线回放测试 → probe 收尾。
回滚即撤销 commit；不影响已归档 change 与 fixtures。

## Open Questions

- tool 结果是作为独立 `Entry` 还是并入 `Assistant.ToolCall` 块？倾向并入，
  实现时以"渲染时工具卡片紧贴其调用"为准，若发现并列更清晰再调整（不影响规格）。
- 是否需要补 follow_up/steer 的入库 snapshot 作为多 turn 回归基线？实现阶段
  跑一次即可决定，不阻塞结构设计。
