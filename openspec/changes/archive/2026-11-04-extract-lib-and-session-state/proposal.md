# Proposal

## Why

路线图（`Docs/roadmap.md`）阶段 0+1 要求：把当前锁在 `src/bin/probe/` 里的 RPC 连接层提炼成可复用库，并在不依赖终端的情况下实现"事件流 → 结构化会话"的无头状态机。这是后续 ratatui TUI（阶段 2+）的地基：渲染层只消费结构化状态，不直接解析 JSONL。现在 probe 与未来的 `tui` bin 是两个无法共享模块的孤立二进制，且探针只做弱分类统计、并不重建对话——TUI 真正需要的累积状态尚不存在。

## What Changes

- 新增 **lib crate**（`src/lib.rs`，crate 名 `pi_rust_tui`），把现有连接相关代码从 `src/bin/probe/` 迁入并增强：
  - `rpc::process`：spawn `pi --mode rpc`、三线程 stdin/stdout/stderr（从 probe 迁移）。
  - `rpc::jsonl`：wire 类型、command id 注入（从 probe 迁移）。
  - `rpc::client`：在传输层之上提供"发命令 → 按 id 等响应"与"订阅事件"的会话化客户端（区别于 probe 里写死在场景 runner 中的路由）。
- 新增**无头会话状态机**（`session/`，纯逻辑、不依赖终端也不依赖真实 pi）：
  - 消费顶层事件（`message_start/update/end`、`turn_*`、`agent_*`、`tool_execution_*`、`queue_update`、`thinking_level_changed`、`session_info_changed` 等），增量维护权威 `SessionState`。
  - 按 `contentIndex` 累积 `text_delta` / `thinking_delta` / `toolcall_delta`，并以 `message_end` 的完整消息做权威校正。
  - 归一化为**有序 transcript 条目**：用户消息、assistant 消息（含 text/thinking/toolCall 内容块）、工具执行（start/update/end、isError、结果）、压缩/retry/队列等状态。
  - 暴露运行态标志：是否正在流式、队列中的 steering/follow_up、当前 model/thinking、usage 等。
  - 提供从 JSONL 文件**回放**事件的入口，供离线测试与未来 TUI 调试复用。
- **probe 迁移为库的消费者**：改为 `use pi_rust_tui::...`，删除其私有重复实现；行为与产物保持不变，现有测试全部继续通过。
- 新增离线测试：用 `tests/fixtures/probe/{text_only,tool_call,failed_tool}` 快照回放状态机，断言重建出的 transcript 结构、内容块、工具结果、多 turn 等。
- **不在本变更范围**：不引入 ratatui/crossterm、不写任何终端渲染、不做命令行 UI、不新增真实 RPC 命令（30 种命令的强类型 builder 只做本变更状态机/客户端需要的最小集，其余留待后续 change）。

## Capabilities

### New Capabilities

- `rpc-client`: 以库形式提供的 `pi --mode rpc` 连接与消息收发能力——子进程生命周期、stdin/stdout/stderr 分离、命令 id 关联响应、事件与响应分流。供 probe 与未来 TUI 共用。
- `session-state`: 与终端无关的会话状态机——把 JSONL 事件流增量累积、归一化为结构化 transcript 与运行态，并可从快照文件离线回放。

### Modified Capabilities

（无。`rpc-probe` 是诊断工具能力，本变更只把它重构为库的消费者，其外部行为与规格不变；若迁移中发现探针可观察行为需要改变，再另开变更。）

## Impact

- **代码结构**：新增 `src/lib.rs` 与 `src/rpc/`、`src/session/` 模块；`src/bin/probe/{process,jsonl,...}` 迁移/瘦身；`src/main.rs`（未来 tui bin）本变更保持 hello world 不动。
- **依赖**：继续只用 `serde` / `serde_json`，不新增 async runtime 或终端库（并发模型留到阶段 2）。
- **测试**：现有 13 个 probe 测试需在迁移后继续通过；新增状态机离线回放测试（纯文件驱动，不烧 token、不依赖 pi 在线）。
- **fixtures**：复用已提交的 4 个 snapshot；若状态机需要 follow_up/steer 的多 turn 证据而现有快照不足，可在实现阶段按需补跑 snapshot（探针已具备能力），但不改变探针规格。
- **无破坏性变更**：库与探针均为本项目内部，尚未对外发布。
