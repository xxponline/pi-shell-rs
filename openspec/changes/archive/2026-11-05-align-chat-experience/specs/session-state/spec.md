# Spec Delta

## ADDED Requirements

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
