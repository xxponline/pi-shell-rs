# Spec Delta

## ADDED Requirements

### Requirement: bash 模式提交与运行

输入 trim 后以 `!` 起始且 `!` 后有内容时，提交 MUST 走 `bash` RPC 执行（`!!` 前缀传 `excludeFromContext: true`），该行不发送给模型、输入清空；`!` 后无内容 MUST 按普通消息照常发送。输入以 `!` 起始时编辑器边框 MUST 变色提示 bash 模式，恢复普通文本后还原。bash 运行期间再次提交 `!cmd` MUST 显示黄条警告（说明已有命令在跑、Esc 可中止）且**输入保留不清空**；运行中空闲 Esc MUST 发送 `abort_bash` 中止执行，其优先级高于双击 Esc（不计入双击窗口），且低于流式中止（流式时 Esc 仍中止生成）。运行中的输出 MUST 在消息区实时显示，命令完成后条目呈现退出码/取消态/截断提示。

#### Scenario: bang 提交执行与边框提示

- **WHEN** 用户输入 `!ls -la` 回车（此前输入 `!` 时边框已变色）
- **THEN** 发送 bash 命令（非 prompt），输入清空，边框恢复；`!!echo hi` 发送时带 excludeFromContext=true

#### Scenario: 空 bang 照常发送

- **WHEN** 用户只输入 `!` 回车
- **THEN** 作为普通 prompt 发送，不触发 bash 执行

#### Scenario: 运行中门控与保留

- **WHEN** bash 运行中用户输入 `!pwd` 回车
- **THEN** 黄条警告已有命令在跑（提示 Esc 中止），输入文本保留在编辑器中，不发送新命令

#### Scenario: Esc 中止与优先级

- **WHEN** bash 运行中按 Esc（空闲、无浮层）
- **THEN** 发送 abort_bash 且不触发双击 Esc；流式中按 Esc 时仍为中止生成而非 bash

#### Scenario: 流式输出与完成态

- **WHEN** 命令执行中持续收到输出分片、随后收到结果
- **THEN** 消息区实时增长输出文本，完成后条目显示退出码（或取消/截断状态）
