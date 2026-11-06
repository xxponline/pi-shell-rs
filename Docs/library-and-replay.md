# 库结构与离线回放（rpc / session）

阶段 0+1 的产物：把连接层从 `probe` 抽成可复用库，并实现与终端无关的会话状态机。
渲染层（ratatui）尚未引入；本库不依赖任何终端库与 async runtime。

## 模块

```
src/
├── lib.rs                 # pi_shell_rs
├── rpc/
│   ├── process.rs         # Transport: spawn pi --mode rpc, 三线程 stdin/stdout/stderr
│   ├── jsonl.rs           # WireMessage / OutgoingCommand / Direction
│   └── client.rs          # RpcClient: id 关联响应 + 事件分流
└── session/
    ├── blocks.rs          # Block(Text/Thinking/ToolCall), ToolExecution
    ├── transcript.rs      # SessionState / Entry / Usage / Phase
    └── replay.rs          # 从探针 transcript JSONL 离线回放
```

## rpc：连接与收发

- `Transport::spawn_rpc(binary, extra_args)`：启动 `pi --mode rpc`（`binary` 通常取
  `rpc::resolve_binary()`，受 `PI_BIN` 覆盖），返回 `events` / `stderr` 两个通道与发送句柄。
- `RpcClient`：在 Transport 之上的会话化客户端。
  - `send_command(value, timeout, policy)`：自动分配 `id`，返回该命令的响应 `data`；
    无 `id` 的消息（agent_*/message_*/tool_execution_*/extension_ui_request 等）
    从 `events()` 通道分流，绝不被命令等待吞掉。
  - `ResponsePolicy`：`RequireSuccess`（success=false 时报错）或 `AcceptError`（返回完整响应，
    用于 compact 这类业务上可预期失败的命令）。
  - `send_raw(&Value)`：发送自带 id 的行（如 `extension_ui_response`），不做 id 路由。
  - `ClientEvent::{Event, ParseError{raw}, Closed}`：普通事件 / 非 JSON 脏行（保留原文）/ pi 退出。

```rust
use pi_shell_rs::rpc::{RpcClient, ResponsePolicy, ClientEvent};
use std::time::Duration;

let client = RpcClient::spawn("pi", &[])?;
let state = client.send_command(
    serde_json::json!({"type":"get_state"}),
    Duration::from_secs(15),
    ResponsePolicy::RequireSuccess,
)?;
while let Ok(ev) = client.events().recv_timeout(Duration::from_millis(100)) {
    if let ClientEvent::Event(v) = ev { /* 按 v.type 归约进 SessionState */ }
}
```

## session：无头状态机

`SessionState` 是纯数据 + 纯归约，不持有子进程：

- `SessionState::handle(&serde_json::Value)`：喂入一条顶层事件，增量更新。
  处理生命周期、text/thinking/toolCall 内容块累积、工具执行归一化、队列/级别/会话名/usage；
  未知事件类型安全忽略。
- 输出（不依赖终端）：
  - `phase`：`Idle` / `Streaming`
  - `entries()`：有序 transcript，条目为 `Entry::User` / `Entry::Assistant { turn, blocks, usage }`
    / `Entry::Notice`。
  - `Block`：`Text` / `Thinking` / `ToolCall { id, name, args_json, tool }`。
  - `steering_queue` / `follow_up_queue` / `thinking_level` / `session_name` / `usage`。

累积语义（与 `rpc-protocol-notes.md` 第 4 节一致）：

- 线上 `message_update` 只有 delta，按 `assistantMessageEvent.contentIndex` 累积。
- `toolcall_start` 从事件顶层取展平的 `id` / `toolName`。
- `message_end` 是权威完整消息，用于整体校正；`turn_end` 只回填/修正已落盘条目，
  不产生重复 assistant 条目。
- 工具执行按 `toolCallId` 关联；早于内容块到达的工具事件先暂存（pending）再合并。

## 离线回放

探针落盘每行形如 `{seq, ts_ms, dir, raw}`。replay 只喂 `dir=="in"` 的 `raw`，
因此无需启动 pi、无需网络、不耗 token：

```rust
use pi_shell_rs::session::{replay_file, Entry, Block};

let state = replay_file(std::path::Path::new(
    "tests/fixtures/probe/tool_call/snapshot/transcript.jsonl",
))?;
for e in state.entries() {
    match e {
        Entry::User { text } => println!("user: {text}"),
        Entry::Assistant { blocks, .. } => {
            for b in blocks {
                match b {
                    Block::Text { text } => println!("assistant: {text}"),
                    Block::Thinking { text } => println!("thinking: {} chars", text.len()),
                    Block::ToolCall { name, tool, .. } => {
                        println!("tool {name} error={} result={}", tool.is_error, tool.result.is_some());
                    }
                }
            }
        }
        Entry::Notice { kind, .. } => println!("notice: {kind}"),
    }
}
```

也可用 `replay_jsonl(reader: impl Read)` 从内存字符串 / 任意流回放。

## 测试

- `cargo test --lib`：rpc（spawn 失败、fake-pi 回放、stderr 分离、id 路由、超时）
  与 session（生命周期、delta 累积、工具归一化、队列、多 turn、内联回放）。
- `cargo test --test session_replay`：直接加载已提交的 text_only / tool_call /
  failed_tool 快照做离线断言。
- `probe` 二进制已改为消费本库，自身不再含连接层实现。
