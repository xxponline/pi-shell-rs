# Spec Delta

## MODIFIED Requirements

### Requirement: 手动 compact 流程

`/compact [自定义指令]` MUST 发送 `compact`（可带 customInstructions）；进行中界面 MUST 显示压缩状态（compacting 标志与通知），完成 MUST 显示结果通知（压缩前后 token 量），业务失败（如会话过小）MUST 以红色错误通知呈现。**compaction 期间提交输入 MUST 本地排队而不立即发送**（直接发送会被 pi 拒绝），压缩结束后按序补发，补发的 prompt MUST 携带 `streamingBehavior:"steer"` 以兼容进入流式的情形。**例外**：命中动态扩展命令的输入 MUST 立即发送（pi 的扩展命令执行先于其压缩检查，排队反而会延迟执行）；skill 与模板命令无此例外，仍走排队。

#### Scenario: 触发与完成通知

- **WHEN** 用户执行 /compact 回车
- **THEN** 界面出现压缩进行中状态，完成后显示含 token 压缩量的结果通知，输入框清空

#### Scenario: 压缩期间排队补发

- **WHEN** compaction 进行中用户输入消息并回车
- **THEN** 消息进入本地队列并可见（不立即发送、不出现红色错误），压缩结束的通知后该消息被自动发出

#### Scenario: 压缩期扩展命令立即执行

- **WHEN** compaction 进行中用户输入已注册的扩展命令 /my-ext 回车
- **THEN** 该行立即作为 prompt 发送（不入本地队列、不出现排队提示），输入框清空

#### Scenario: 压缩失败可见

- **WHEN** compact 返回业务错误（如会话过小）
- **THEN** 红色错误通知显示原因，界面回到空闲
