# pi RPC 协议 —— Rust 客户端实现笔记

> 写给自己看的实现笔记。目标：基于本文档直接在 Rust 端建模事件循环与渲染状态。
> 不做 pi 源码说明，只讲**Rust 端必须知道什么**。
>
> 来源（pi 源码只读参考）：
> - `packages/coding-agent/src/modes/rpc/rpc-types.ts`
> - `packages/coding-agent/src/modes/rpc/rpc-mode.ts`
> - `packages/coding-agent/src/modes/json-event.ts`
> - `packages/coding-agent/src/core/agent-session.ts`（`AgentSessionEvent`）
> - `packages/agent/src/types.ts`（`AgentEvent`）
> - `packages/ai/src/types.ts`（`AssistantMessageEvent` / `Usage` / `ToolResultMessage`）

---

## 0. 进程与通道模型

```
        stdin (JSONL)         ┌─────────────────────┐   stdout (JSONL)
   Rust  ────────────────►   │  pi-agent --mode rpc  │ ───────────────►  Rust
                             └─────────────────────┘
```

- **逐行 JSON**：stdin 每一行一条命令，stdout 每一行一个 JSON 对象。两者互相独立。
- **stdout 上有两种流混在一起**：
  1. `response`（对应 Rust 发出去的某条 command，带 `id`）
  2. `event`（持续推送的会话事件，**无 id**）
  3. `extension_ui_request`（扩展弹窗，带 `id`）
  4. `extension_error`

Rust 端的事件循环必须**用同一个行读取器把这四种消息分开路由**，不能假设 response 一定紧跟 command。

---

## 1. 每条 command 的 id 语义

```ts
{ id?: string; type: "..."; ... }
```

- `id` 是可选的；不传时响应也不带 id。
- **Rust 端必须自己生成 id**（如 nanoid / 递增整数），用于把异步响应关联回发起者。
- 事件流（`type: "agent_start"` 等）**永远不带 id**，看到就直接广播给 UI 状态机，不要等待。

### 响应格式（统一外壳）

成功：
```json
{ "id": "xxx", "type": "response", "command": "get_state", "success": true, "data": { ... } }
```

失败：
```json
{ "id": "xxx", "type": "response", "command": "prompt", "success": false, "error": "..." }
```

**Rust 建模建议**：

```rust
enum RpcResponse {
    Success { id: Option<String>, command: CommandKind, data: serde_json::Value },
    Failure { id: Option<String>, command: String, error: String },
}
```

先按 `command` 字段 match 出具体 `data` 结构，再用 `serde` 反序列化到对应类型。

---

## 2. Command 清单（按用途分组，附 Rust 端关注点）

### 2.1 发 prompt / 排队

| command | 字段 | response.data | Rust 关注点 |
|---|---|---|---|
| `prompt` | `message`, `images?`, `streamingBehavior?: "steer"\|"followUp"` | `{ disposition: "started" \| "handled" \| "queued" }` | `disposition="queued"` 时 UI 应显示"已排队"，不是"已开始" |
| `steer` | `message`, `images?` | `{ disposition: "handled"\|"queued" }` | 打断当前生成 |
| `follow_up` | `message`, `images?` | `{ disposition }` | 排在当前生成之后 |
| `abort` | — | `{ success: true }` | 立即生效 |
| `clear_queue` | — | `{ steering: string[], followUp: string[] }` | 返回被清掉的内容列表，UI 可弹提示 |
| `new_session` | `parentSession?` | `{ cancelled: boolean }` | `cancelled=true` 说明被扩展 `session_before_switch` 拦了 |

**实现要点**：`prompt/steer/follow_up` 三者都**不等同于** agent 真的开始跑。真正的"开始/结束"看事件流里的 `agent_start` / `agent_settled`。

### 2.2 状态与模型

| command | 字段 | response.data |
|---|---|---|
| `get_state` | — | `RpcSessionState`（见下） |
| `set_model` | `provider`, `modelId` | `Model` |
| `cycle_model` | — | `{ model, thinkingLevel, isScoped } \| null` |
| `get_available_models` | — | `{ models: Model[] }` |
| `set_thinking_level` | `level: ThinkingLevel` | `success` |
| `cycle_thinking_level` | — | `{ level } \| null` |
| `get_available_thinking_levels` | — | `{ levels: ThinkingLevel[] }` |
| `set_steering_mode` | `mode: "all"\|"one-at-a-time"` | `success` |
| `set_follow_up_mode` | `mode` | `success` |

**`RpcSessionState`**（`get_state.data`）：
```
model?: Model
thinkingLevel: "off"|"minimal"|"low"|"medium"|"high"|"xhigh"|"max"
isStreaming: bool
isCompacting: bool
steeringMode / followUpMode
sessionFile?: string
sessionId: string
sessionName?: string
autoCompactionEnabled: bool
messageCount: int
pendingMessageCount: int      ← 队列里有几条待处理
```

Rust 端启动后**第一件事就是发 `get_state`**，把这个 snapshot 作为 UI 初始状态。

### 2.3 Compaction / Retry

| command | 字段 | data |
|---|---|---|
| `compact` | `customInstructions?` | `CompactionResult` |
| `set_auto_compaction` | `enabled` | `success` |
| `set_auto_retry` | `enabled` | `success` |
| `abort_retry` | — | `success` |

`CompactionResult`:
```
summary: string
firstKeptEntryId: string
tokensBefore: int
estimatedTokensAfter?: int
usage?: Usage
```

### 2.4 Bash（非 agent 循环的独立 bash）

| command | 字段 | data |
|---|---|---|
| `bash` | `command`, `excludeFromContext?` | `BashResult` |
| `abort_bash` | — | `success` |

`BashResult`:
```
output: string
exitCode: int | null
cancelled: bool
truncated: bool
fullOutputPath?: string    ← 输出被截断时，完整结果在这
```

### 2.5 会话导航 / 树

| command | 字段 | data |
|---|---|---|
| `get_session_stats` | — | `SessionStats` |
| `export_html` | `outputPath?` | `{ path }` |
| `switch_session` | `sessionPath` | `{ cancelled }` |
| `fork` | `entryId` | `{ text, cancelled }` |
| `clone` | — | `{ cancelled }` |
| `get_fork_messages` | — | `{ messages: [{entryId, text}] }` |
| `get_entries` | `since?` | `{ entries: SessionEntry[], leafId: string|null }` |
| `get_tree` | — | `{ tree: SessionTreeNode[], leafId }` |
| `get_last_assistant_text` | — | `{ text: string|null }` |
| `set_session_name` | `name` | `success` |
| `get_messages` | — | `{ messages: AgentMessage[] }` |
| `get_commands` | — | `{ commands: RpcSlashCommand[] }` |

**Rust 端重点**：`SessionEntry` 是 12 种 `type` 的联合体（`message` / `usage` / `compaction` / `branch_summary` / `label` / `model_change` / `thinking_level_change` / `custom` / `custom_message` / `context_edit` / `session_info`）。要完整复刻 UI，这些都得能渲染——至少得**不崩**。

---

## 3. 事件流（这是 Rust UI 状态机的真正驱动源）

### 3.1 一次 prompt 的完整事件时间线

```
Rust 发: prompt { message: "hi" }
           │
           ▼
stdout: response { command:"prompt", data:{disposition:"started"} }   ← 这只是"受理"
           │
           ▼
stdout: agent_start
stdout: turn_start
stdout: message_start  { message: <初始 assistant 骨架> }
stdout: message_update { usage, assistantMessageEvent: text_delta ... }  × N
stdout: message_update { assistantMessageEvent: toolcall_start ... }
stdout: tool_execution_start { toolCallId, toolName, args }
stdout:   tool_execution_update { partialResult ... }                   × M
stdout: tool_execution_end   { result, isError, durationMs }
stdout: message_update { assistantMessageEvent: text_delta ... }       ← 后续文本
stdout: message_end { message: <完整 assistant> }
stdout: turn_end { message, toolResults }
stdout: agent_end { messages, willRetry }
stdout: agent_settled { aborted: bool }                                ← 一个 run 真正结束
           │
           ▼
Rust UI: 回到空闲态
```

### 3.2 事件分类（按渲染关注点）

**A. 生命周期**（Rust 状态机直接消费）：
- `agent_start` / `agent_end { messages, willRetry }` / `agent_settled { aborted }`
- `turn_start` / `turn_end { message, toolResults }`
- `message_start { message }` / `message_end { message }`

**B. 流式增量**（Rust 必须**自己累积**文本）：
- `message_update { usage, assistantMessageEvent }`
  - 见第 4 节详解。

**C. 工具执行**（UI 画"工具卡片"用）：
- `tool_execution_start { toolCallId, toolName, args, parentToolCallId? }`
- `tool_execution_update { toolCallId, toolName, args, partialResult, parentToolCallId? }`
- `tool_execution_end   { toolCallId, toolName, result, isError, durationMs?, parentToolCallId? }`

**D. 队列/会话状态**：
- `queue_update { steering: string[], followUp: string[] }`
- `entry_appended { entry: SessionEntry }`
- `session_info_changed { name }`
- `thinking_level_changed { level }`

**E. Compaction 流程**：
- `compaction_start { reason: "manual"|"threshold"|"overflow" }`
- `compaction_end   { reason, result?, aborted, willRetry, errorMessage? }`

**F. Auto-retry**：
- `auto_retry_start { attempt, maxAttempts, delayMs, errorMessage }`
- `auto_retry_end   { success, attempt, finalError? }`

**G. Summarization retry**（compaction 失败重试）：
- `summarization_retry_scheduled { attempt, maxAttempts, delayMs, errorMessage }`
- `summarization_retry_attempt_start { source: "branchSummary" | { source:"compaction", reason } }`
- `summarization_retry_finished`

**H. 独立 bash 输出流**（对应 `bash` command）：
- `bash_execution_update { id?, delta }`

### 3.3 重要：事件**不带 id**

Rust 端不要指望用 id 关联事件到 command。**唯一的关联方式是时间序和状态机**：
- 你发了 `prompt` → 等 `agent_settled` 算"这次 prompt 结束"。
- 你发了 `bash` → 等 `response { command:"bash" }` 算结束（期间穿插 `bash_execution_update`）。

---

## 4. `message_update` 的真相（Rust 端最容易踩坑）

### 4.1 源码做了什么

`rpc-mode.ts` 并不是把 `AgentSessionEvent` 原样透传，它经过了 `toJsonEvent()` 这层"瘦身"：

```
原始事件:
{
  type: "message_update",
  message: <完整累积的 AssistantMessage>,     ← 被剥掉
  assistantMessageEvent: {
    type: "text_delta",
    contentIndex: 1,
    delta: "hello",
    partial: <累积快照>                          ← 被剥掉
  }
}

stdout 实际发出:
{
  type: "message_update",
  usage: <当前累积 usage>,
  assistantMessageEvent: {
    type: "text_delta",
    contentIndex: 1,
    delta: "hello"
  }
}
```

两个关键操作：
1. **剥掉累积的 `message`**（每帧都发整个 message 太大）。
2. **剥掉 assistantMessageEvent 里的 `partial`**（同理）。
3. **对 `toolcall_start` 特判**：把 `toolCallId` 和 `toolName` 提到顶层（`id` / `toolName` 字段），方便客户端路由。

### 4.2 Rust 端必须自己维护的状态

```
每个正在流式的 assistant message:
├─ content_blocks: Vec<ContentBlock>
│   ├─ Text { text: String }
│   ├─ Thinking { text: String }
│   └─ ToolCall { id, name, args_json: String }     ← args 是流式累积的 JSON 字符串
├─ usage: Usage                                       ← 每帧覆盖
└─ stop_reason: Option<...>
```

### 4.3 assistantMessageEvent 的 12 种 delta（Rust 端 dispatch 表）

| type | 关键字段 | Rust 端动作 |
|---|---|---|
| `start` | （wire 上已剥 partial） | 通常由 `message_start` 完成初始化，可忽略 |
| `text_start` | `contentIndex` | 在 content_blocks[contentIndex] 占位为 Text |
| `text_delta` | `contentIndex`, `delta` | 追加字符串到对应 Text 块 |
| `text_end` | `contentIndex`, `content` | 用最终 content 校准（防 drift） |
| `thinking_start` | `contentIndex` | 占位 Thinking 块 |
| `thinking_delta` | `contentIndex`, `delta` | 追加思考文本 |
| `thinking_end` | `contentIndex`, `content` | 校准 |
| `toolcall_start` | `contentIndex`, **`id`**, **`toolName`** | 新建 ToolCall 块，记 id+name |
| `toolcall_delta` | `contentIndex`, `delta` | 追加 args_json 到对应 ToolCall |
| `toolcall_end` | `contentIndex`, `toolCall` | 用完整 ToolCall 校准 |
| `done` | `reason: "stop"\|"length"\|"toolUse"\|"deferred"` | 标记本次 assistant 完成 |
| `error` | `reason: "aborted"\|"error"`, `error` | 标记失败 |

**注意**：`toolcall_start` 在 wire 上和其它 delta 形状不一样——它的 `id` / `toolName` 在顶层而不是嵌套在 `toolCall` 里。Rust serde 建模要对它做单独的 enum variant。

### 4.4 什么时候可以信赖最终值

- `message_end` 携带**完整 message**（含所有 content blocks 最终态）。
- `turn_end` 携带最终 message + 所有 toolResults。
- `agent_end` 携带本轮所有 messages 数组。

**渲染策略建议**：
- 流式中：用 4.3 的累积状态做"乐观渲染"。
- 收到 `message_end`：用权威值**整体替换**累积状态（防止 drift）。

---

## 5. `Usage` 结构（每帧都可能带）

```
input, output, cacheRead, cacheWrite: u32
cacheWrite1h?: u32
reasoning?: u32
totalTokens: u32
cost: {
  input, output, cacheRead, cacheWrite, total: f64
}
```

Rust 端：每次 `message_update` 覆盖当前 usage；`agent_end` 后可用最终 usage 渲染"本次消耗"。

---

## 6. 扩展 UI 通道（双向）

### 6.1 Rust 收到 `extension_ui_request`

9 种 method：

| method | 关键字段 | Rust 端 UI |
|---|---|---|
| `select` | `title`, `options: string[]`, `timeout?` | 弹列表，返回选中的 option 字符串 |
| `confirm` | `title`, `message`, `timeout?` | 弹窗，返回 bool |
| `input` | `title`, `placeholder?`, `timeout?` | 文本输入，返回字符串 |
| `editor` | `title`, `prefill?` | 打开编辑器（$EDITOR 或自绘多行） |
| `notify` | `message`, `notifyType?: "info"\|"warning"\|"error"` | 状态栏/浮层 toast |
| `setStatus` | `statusKey`, `statusText?: string` | 状态栏更新 |
| `setWidget` | `widgetKey`, `widgetLines?: string[]`, `widgetPlacement?: "aboveEditor"\|"belowEditor"` | 编辑器上方/下方的小部件 |
| `setTitle` | `title` | 窗口标题 |
| `set_editor_text` | `text` | 外部预填编辑器内容 |

### 6.2 Rust 回复（stdin 再发）

```json
// 对 select / input / editor:
{ "type": "extension_ui_response", "id": "<原 request id>", "value": "<返回字符串>" }

// 对 confirm:
{ "type": "extension_ui_response", "id": "...", "confirmed": true }

// 用户取消（任何 method）:
{ "type": "extension_ui_response", "id": "...", "cancelled": true }
```

**实现要点**：Rust 端要维护一个 `pending_extension_requests: HashMap<RequestId, oneshot::Sender>`。弹窗关闭时把值塞回去。

### 6.3 旁路事件

```
extension_error { extensionPath, event, error }
```
不阻塞主流程，但应在 UI 角落显示。

---

## 7. 冷启动时序（Rust 端 init 流程建议）

```
1.  spawn `pi-agent --mode rpc`
2.  读 stdout 直到出现第一行（可能是 extension_ui_request 或其它事件）
3.  发: get_state                          → 拿 RpcSessionState 作为初始 UI
4.  发: get_available_models               → 模型列表
5.  发: get_available_thinking_levels     → thinking 级别列表
6.  发: get_commands                       → slash command 列表（供自动补全）
7.  发: get_entries                        → 历史消息（如果要恢复视图）
8.  进入主循环:
    - stdin: 用户输入 → 组装 prompt command 发出
    - stdout: 行解析 → 分发到 (response | event | extension_ui_request | extension_error)
    - response: oneshot.wake
    - event:   更新渲染状态
    - extension_ui_request: 弹对应 UI
```

---

## 8. 缺口验证结果（探针实测，pi v1.5.0）

以下由 `probe` 二进制对真实 `pi --mode rpc` 跑 8 个场景得出。原始 JSONL 见
`probe-out/<scenario>/latest/`（未入库）；证据场景名标注在每条后。

1. **✅ 已验证（text_only）—— `message_start` 对 system/user/assistant 三种消息都发**，不只是 assistant：
   - `system`：`message.content` 是**空字符串 `""`**（真正的 system 内容在 `message.sections.*` 里，按段分块，如 `sections.agent_browser`）。
   - `user` / `assistant`：`message.content` 是**空数组 `[]`**，后续靠 delta 填充。
   - 一次 text turn 观察到 3 对 `message_start/message_end`（system、user、assistant），但只有 assistant 那段夹着 `message_update`。**Rust 端要按 role 过滤，不能把每个 message_start 都当成新 assistant 气泡**。
2. **✅ 已验证（tool_call）—— `toolcall_start` 确实被展平到顶层**：wire 形状为
   `{type:"message_update", usage, assistantMessageEvent:{type:"toolcall_start", contentIndex, id, toolName}}`。
   `id`（如 `call_dabsq0p9k...`）和 `toolName`（如 `read`）都在 assistantMessageEvent 顶层，`contentIndex` 保留。
3. **✅ 已验证（text_only）—— `usage` 每帧都带**（26/26 条 message_update 都有 usage），但生成早期全为 0。可以每帧覆盖。
4. **✅ 已验证（text_only）—— `agent_end.messages` 是本 run 的全量消息数组**（一次单轮对话为 3 条：system/user/assistant），**不是增量**。跨 run 的完整历史仍需 Rust 自己累积或用 `get_messages`/`get_entries`。字段为 `{type, messages, willRetry}`。
5. **⚠️ 未触发 —— `entry_appended`**：8 个场景（含工具、compaction、steer、follow_up）都没观察到该事件。它可能只在从磁盘恢复/追加历史条目时发，而不是普通流式中。普通流式用 message_* 即可，**Rust 端不应依赖 entry_appended 渲染当前对话**；待后续 resume 场景再验证。
6. **✅ 已验证（compact）—— 错误响应的 `error` 是人类可读字符串**，例如 compact 在会话过小时返回：
   `{type:"response", id:"compact-3", command:"compact", success:false, error:"Nothing to compact (session too small)"}`。注意：即便返回这个业务错误，`compaction_start`/`compaction_end` 事件**仍然发出**。compaction 阈值是 `keepRecentTokens = 20000`，探针级小会话无法让它成功，这是预期。
7. **⚠️ 未触发 —— `bash_execution_update`**：没有覆盖 Rust 主动发 `bash` command 的场景（当前内置场景只走 agent 循环）。其 `id` 语义待补一个 direct-bash 场景验证。
8. **✅ 已验证（全部场景）—— stderr 完全干净**：所有运行的 `stderr.log` 为 0 行；stdout 无任何非 JSON / ANSI 行（探针没有产生任何 `parse_error` 分类）。`notify` 一类通知走 stdout 的 `extension_ui_request`，**不**走 stderr。另注意：pi 自身的 TPS 等提示也是通过 `extension_ui_request.notify` 出现的。

### 额外观察（类型定义里不显眼、但影响渲染）

- assistant 内容里 `thinking_*` 与 `text_*` 会**各自成对**出现 `*_start`/`*_end`，且一次回答可有多个 thinking 块（tool_call 场景观察到 2 个 thinking_start）。content block 必须按 `contentIndex` 建索引，不能假设只有一块 thinking + 一块 text。
- `steer` 会触发额外的 `turn_start/turn_end` 与 `queue_update`；`follow_up` 排队时一次 run 内有多个 turn（3 个 turn_start）。
- 工具失败时 `tool_execution_end.isError = true`（failed_tool 已验证），但 assistant 仍会继续生成解释文本。

---

## 9. 探针（已实现）

探针已由 `build-rpc-probe-client` 变更实现为 cargo bin target：

```bash
cargo build --bin probe
./target/debug/probe --list                      # 列出内置场景
./target/debug/probe --scenario text_only        # 跑单个场景
./target/debug/probe --all \
  --ext probe-fixture-extension/probe-ui.ts      # 跑全部（extension_ui 需 fixture 扩展）
```

- 二进制名是 **`pi`**（v1.5.0，`pi --mode rpc`），不是旧文档里的 `pi-agent`；可用 `PI_BIN` 覆盖。
- 产物在 `probe-out/<scenario>/runs/<ts>/`（git-ignored），`latest/` 为最新副本；`--snapshot` 写截断快照到 `tests/fixtures/probe/<scenario>/snapshot/`。
- 已覆盖场景：text_only / tool_call / thinking / steer / follow_up / compact / failed_tool / extension_ui。
- 尚未覆盖（后续可加）：Rust 主动发 `bash`（验证 `bash_execution_update`）、resume 会话（验证 `entry_appended`）、其余 extension UI method（input/editor/setStatus/setWidget/setTitle/set_editor_text）。
