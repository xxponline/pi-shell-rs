# Spec Delta

## ADDED Requirements

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
