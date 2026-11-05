# Spec Delta

## ADDED Requirements

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
