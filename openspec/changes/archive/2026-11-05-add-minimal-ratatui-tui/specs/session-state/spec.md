# Spec Delta

## ADDED Requirements

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
