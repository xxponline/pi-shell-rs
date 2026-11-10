# session-state Specification

## Purpose

提供一个与终端、网络都无关的会话状态机：把 RPC 事件流增量累积并归一化为结构化 transcript 与运行态，使渲染层只需消费稳定的数据结构；它必须能从已归档的 JSONL 快照离线回放，以便无 token、无 pi 进程地测试。

## Requirements

### Requirement: 事件驱动的增量状态更新

状态机 MUST 接受按时间顺序的顶层会话事件并增量更新自身权威状态，至少处理 `message_start`、`message_update`、`message_end`、`turn_start`、`turn_end`、`agent_start`、`agent_end`、`agent_settled`、`tool_execution_start`、`tool_execution_update`、`tool_execution_end`、`queue_update`、`thinking_level_changed`、`session_info_changed`。未知事件类型 MUST 被安全忽略而不中断。

#### Scenario: 一次完整对话驱动流式与空闲状态

- **WHEN** 向状态机依次送入 text_only 快照中的 agent_start → message_* → turn_* → agent_settled
- **THEN** 状态机先反映"正在流式"，在 agent_settled 后反映"空闲"，且 transcript 中得到一条用户消息与一条 assistant 消息

#### Scenario: 未知事件不致命

- **WHEN** 状态机收到一个当前版本未识别的事件类型
- **THEN** 状态机不报错、不丢失此前状态，后续已知事件仍被正常处理

### Requirement: assistant 内容块按 contentIndex 累积

状态机 MUST 依据 `message_update.assistantMessageEvent` 的 `contentIndex` 维护 text、thinking、toolCall 三类内容块，用 `text_delta` / `thinking_delta` / `toolcall_delta` 追加，并在收到对应 `*_end` 或 `message_end` 时用权威值校正。toolcall_start MUST 从线格式顶层取得工具调用 id 与工具名。

#### Scenario: 文本与思考增量累积

- **WHEN** 重放某 assistant 消息的一串 thinking_delta 与 text_delta
- **THEN** 重建出的对应内容块文本等于该消息 `message_end` 中的最终 text 与 thinking 内容

#### Scenario: 工具调用块记录 id 与名字

- **WHEN** 重放 tool_call 快照中的 toolcall_start → toolcall_delta → toolcall_end
- **THEN** assistant 内容中存在一个 toolCall 块，其 id 与 name 与权威消息一致

#### Scenario: message_end 权威校正

- **WHEN** 增量累积后收到携带完整消息的 message_end
- **THEN** 该 assistant 消息的最终内容以 message_end 为准，消除增量漂移

### Requirement: 工具执行归一化

状态机 MUST 把 `tool_execution_start/update/end` 关联到对应工具调用，记录工具名、参数、进行中/结束状态、`isError`、结果与耗时，并作为 transcript 中可区分的工具执行条目（或 assistant 消息上的结构化字段）暴露。`tool_execution_update` 携带的 `partialResult` MUST 被记录为该工具运行中的部分结果（供渲染层实时展示）；`tool_execution_end` 到达时完整结果 MUST 替换部分结果，部分结果 MUST NOT 在结束后残留或与完整结果拼接。无 `partialResult` 字段的 update MUST NOT 清空已有部分结果。

#### Scenario: 成功工具执行

- **WHEN** 重放 tool_call 快照的 read 工具 start → update → end(isError=false)
- **THEN** 状态机中该工具执行为已结束、无错误，且包含结果内容

#### Scenario: 失败工具执行

- **WHEN** 重放 failed_tool 快照，其 tool_execution_end 为 isError=true
- **THEN** 该工具执行被标记为错误，且不影响同 turn 后续 assistant 文本被正常累积

#### Scenario: 运行中记录部分结果

- **WHEN** start 之后收到携带 partialResult 的 update
- **THEN** 该工具执行携带部分结果且状态为运行中，渲染层可据此显示已到达的输出

#### Scenario: 完整结果替换部分结果

- **WHEN** 已有部分结果的工具收到 tool_execution_end 的完整 result
- **THEN** 部分结果被完整结果替换，结束后不残留部分结果

### Requirement: 结构化 transcript 与运行态

状态机 MUST 暴露有序 transcript（用户/assistant/工具等条目，顺序对应会话发生顺序）以及运行态：是否正在流式、thinking 级别、会话名、steering/follow_up 队列、最近 usage。多 turn（follow_up 排队产生多个 turn）MUST 全部保留，不能只保留最后一轮。

#### Scenario: transcript 顺序与角色

- **WHEN** 重放 tool_call 快照（system/user/assistant/toolResult/assistant）
- **THEN** transcript 中用户消息先于 assistant 回复，工具执行位于其触发的 assistant 内容与后续回复之间

#### Scenario: 队列状态更新

- **WHEN** 收到 queue_update 事件
- **THEN** 运行态中的待处理 steering/follow_up 反映事件给出的队列内容

### Requirement: 从 JSONL 快照离线回放

状态机 MUST 提供入口，读取探针归档的 transcript JSONL（每行含 `dir` 与 `raw`），只回放入站（in）事件来重建状态，整个过程不需要启动 pi 或访问网络。

#### Scenario: 离线重建真实快照

- **WHEN** 离线测试加载 text_only、tool_call、failed_tool 三个 snapshot 的 transcript.jsonl 并回放
- **THEN** 三个场景各自重建出非空 transcript，关键断言（消息数、内容块类型、工具成败）成立，且测试不依赖 pi 进程
### Requirement: 流式内容只读访问

状态机 MUST 提供只读访问器，暴露当前尚未由 `message_end` 落盘、正在累积的 in-flight assistant 内容块与是否存在进行中流式消息的标志，供渲染层实时显示 delta。该访问 MUST NOT 改变归约状态，且在没有进行中消息时返回空。

#### Scenario: 流式期间可见 in-flight 内容

- **WHEN** 状态机已收到 assistant 的 message_start 与若干 text_delta，但尚未收到 message_end
- **THEN** 只读访问器返回当前累积的内容块，且已落盘 transcript 不被该读取改变

#### Scenario: 落盘后 in-flight 清空

- **WHEN** message_end（或 turn 结束）把 assistant 消息写入 transcript
- **THEN** 访问器不再返回该已落盘消息的 in-flight 内容，后续读取返回空或仅新的进行中消息

#### Scenario: 空闲时访问为空

- **WHEN** 没有任何进行中的 assistant 消息时读取访问器
- **THEN** 返回空内容，且不报错
### Requirement: 运行级错误归约为通知

状态机 MUST 在 assistant 的 `message_end` 携带非空 `errorMessage` 时追加一条 `Notice{kind:"error"}` 条目，内容为该错误文本；`turn_end` 对同一消息的权威修正 MUST NOT 产生重复通知。正常消息 MUST NOT 产生 error 通知。

（本条补记已实现行为，使规格与实现一致。）

#### Scenario: errorMessage 落成通知

- **WHEN** 重放带 `errorMessage` 的 assistant message_end（content 可为空）
- **THEN** transcript 出现 kind 为 error、文本等于 errorMessage 的 Notice

#### Scenario: turn_end 不重复

- **WHEN** 随后的 turn_end 携带同一 errorMessage 的消息
- **THEN** error 通知数量保持为 1

#### Scenario: 正常消息无通知

- **WHEN** assistant message_end 不含 errorMessage
- **THEN** 不产生 error 通知

### Requirement: 自动重试事件归约为通知

状态机 MUST 把 `auto_retry_start` 归约为通知条目（kind 为 retry，文本含尝试次数、最大次数与错误摘要），把 `auto_retry_end` 归约为结果通知（成功或最终失败）；该处理 MUST NOT 影响 phase 或 transcript 中消息的归约。

#### Scenario: 重试开始与结束

- **WHEN** 依次收到 auto_retry_start(attempt=1, max=3) 与 auto_retry_end(success=true)
- **THEN** transcript 依次出现两条 retry 通知，且 phase 与消息条目不受影响

### Requirement: Usage 携带费用

`Usage` MUST 从 wire 的 usage 对象读取 cost 字段（input/output/cacheRead/cacheWrite/total，浮点），缺失时费用为 0 而非错误；token 计数行为保持不变。

#### Scenario: 解析费用

- **WHEN** message_update 的 usage 含 `cost.total: 0.012`
- **THEN** 解析出的 Usage 费用为 0.012，token 计数照常

#### Scenario: 无费用字段

- **WHEN** usage 对象缺少 cost 字段
- **THEN** Usage 各费用为 0，解析不失败

### Requirement: 应用 get_state 响应

状态机 MUST 提供把 `get_state` 响应（RpcSessionState）应用到自身的方法：至少设置模型标识、会话名与 thinking 级别；字段缺失时保留既有值。

#### Scenario: 应用模型与会话名

- **WHEN** 用含 model/thinkingLevel/sessionName 的 get_state 响应调用该方法
- **THEN** 状态中的模型、thinking 级别与会话名被更新

#### Scenario: 缺失字段保留旧值

- **WHEN** 响应缺少 sessionName 字段且状态中已有会话名
- **THEN** 会话名保持不变

### Requirement: 从消息快照重建

状态机 MUST 提供从 `get_messages` 响应（消息数组）整体重建 transcript 的入口：system 消息跳过，user/assistant 消息映射为对应条目（assistant 内容块按 content 解析），toolResult 消息按 toolCallId 并入最近的 assistant ToolCall 块；重建后 phase 为 Idle、流式缓冲与队列清空。重建 MUST 可重复调用（切换会话时以新快照整体替换旧 transcript），输入非法或字段缺失不得 panic。

#### Scenario: 从快照重建完整对话

- **WHEN** 用含 system/user/assistant/toolResult/assistant 的消息数组调用重建
- **THEN** transcript 依次为用户消息、含工具调用（结果已并入）的 assistant、最终 assistant 文本，phase 为 Idle

#### Scenario: 重复重建替换旧状态

- **WHEN** 已有旧 transcript 时再次用新会话的消息数组重建
- **THEN** 旧条目被完全替换，队列与 in-flight 清空

#### Scenario: 缺字段与空数组不 panic

- **WHEN** 消息对象缺少 content 字段，或传入空数组
- **THEN** 跳过无法解析的消息、空数组清空 transcript，均不报错

### Requirement: compaction 状态与通知

状态机 MUST 在 `compaction_start` 时置 compacting 标志并追加进行中通知，在 `compaction_end` 时清除标志并追加结果通知（成功含压缩前后 token 量，失败含错误原因）；compacting 标志 MUST 供上层在压缩期间拦截输入提交。该处理 MUST NOT 影响 phase 与消息归约。

#### Scenario: 压缩开始与成功结束

- **WHEN** 依次收到 compaction_start(reason=manual) 与 compaction_end(result 含 tokensBefore)
- **THEN** compacting 标志先置后清，transcript 依次出现进行中与成功结果通知，消息条目不受影响

#### Scenario: 压缩失败清除标志

- **WHEN** compaction_end 携带 errorMessage（aborted 或业务失败）
- **THEN** compacting 标志被清除，出现含失败原因的通知，不 panic

### Requirement: bash 执行归约与重建

状态机 MUST 归约 bash 执行生命周期：`bash_execution_update{delta}` 追加到当前运行中的输出缓冲（同一时刻至多一个 bash，无需按 id 区分）；bash 命令结果（output/exitCode/cancelled/truncated/fullOutputPath，经异步结果通道到达）MUST 落为消息区的 bash 条目并清空运行缓冲。条目 MUST 携带命令文本、最终输出、退出码/取消态与 `excludeFromContext` 标记。`get_messages` 快照中的 `role: "bashExecution"` 消息 MUST 被解析为同构条目（切换会话重建后 bash 历史保留）；未知 role 维持现有跳过行为。该处理 MUST NOT 影响 phase 与助手消息归约。

#### Scenario: 流式累积并落条目

- **WHEN** 依次收到 bash_execution_update(delta=行1)、bash_execution_update(delta=行2)、命令结果(exitCode=0)
- **THEN** 运行期间输出缓冲依次含行1、行2；结果到达后消息区出现含完整输出与退出码 0 的 bash 条目，缓冲清空

#### Scenario: 取消与不进上下文标记

- **WHEN** 命令被中止（cancelled=true），或命令以 `!!` 发出（excludeFromContext=true）
- **THEN** 条目呈现取消态；excludeFromContext 的条目带"不进上下文"标记，均不 panic

#### Scenario: 快照重建保留 bash 历史

- **WHEN** 用含 system/user/bashExecution/assistant 的消息数组调用重建
- **THEN** transcript 依次含用户消息、bash 条目（命令与输出保留）、assistant 文本，phase 为 Idle
