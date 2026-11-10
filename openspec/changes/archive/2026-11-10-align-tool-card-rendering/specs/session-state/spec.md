# Spec Delta

## MODIFIED Requirements

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
