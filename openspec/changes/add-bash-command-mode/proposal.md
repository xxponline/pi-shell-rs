# Proposal

## Why

差异表 #6：pi 支持在输入框直接键入 `!command` 执行 bash（`!!` 前缀不进上下文），编辑器边框即时变色提示模式、运行中 Esc 可中止、输出流式回显并落进会话历史（`bashExecution` 消息，后续 `get_messages` 可回放）。我们的 TUI 已有 RPC `bash`/`abort_bash` 与事件 `bash_execution_update`（当前被忽略），但输入侧完全没有这条通路。

## What Changes

- **`!`/`!!` 提交分流**：输入以 `!` 起始（trim 后）→ 提取命令走 `bash` RPC（`!!` 传 `excludeFromContext: true`）；`!` 后无内容按普通消息照常发送；**分流先于压缩排队与斜杠分发之外的常规路径**（对齐 pi 次序）
- **运行态门控**：bash 运行中再次提交 `!cmd` → 黄条警告 "A bash command is already running. Press Esc to cancel it first." 且**保留输入不清空**
- **Esc 中止**：bash 运行中空闲 Esc → `abort_bash`（优先级：流式中止 > bash 中止 > 双击 Esc，且不计入双击窗口）
- **模式提示**：输入以 `!` 起始时编辑器边框变色（对齐 pi 的 `getBashModeBorderColor` 语义）
- **流式输出渲染**：`bash_execution_update{delta}` 实时累积显示（运行中状态区），`bash` 响应（BashResult：output/exitCode/cancelled/truncated/fullOutputPath）到达后落为消息区条目；`!!` 条目标注"不进上下文"
- **会话重建**：`load_messages`/回放解析 `role: bashExecution` 消息为新条目（切换会话后 bash 历史不丢失）
- **顺带修正**：斜杠命令在压缩期间被本地排队的既有偏差——按 pi 次序把斜杠分发移到压缩门控之前（命中即执行）
- **文档**：`Docs/tui-usage.md`（`!` 键位/边框）、`Docs/pi-tui-interaction.md` 差异 #6 完成、`Docs/slash-commands.md` 边界说明

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `session-state`: ADDED "bash 执行归约与重建"需求（update 累积、结果落条目、快照重建解析 bashExecution、excludeFromContext 标记）
- `tui-app`: ADDED "bash 模式提交与运行"需求（分流、运行警告保留、Esc 中止优先级、边框提示、流式渲染与完成态）

## Impact

- `src/session/transcript.rs`：`Entry::Bash { command, output, exit_code, cancelled, excluded }` + `bash_execution_update` 累积 + 结果归约 + `load_messages` 解析新 role
- `src/app/mod.rs`：`submit_as` 重排（斜杠 → `!` 分流 → 压缩排队 → 流式 → 常规）、`bash_running` 门控、Esc 中止分支、`AsyncKind::BashDone`
- `src/app/chat.rs` / `view.rs`：Bash 条目渲染（运行中 loader / 完成 exit code / 取消态 / 截断路径）
- `src/app/event.rs`：`AsyncKind::BashDone`
- 不改 RPC 协议（`bash`/`abort_bash`/`bash_execution_update` 均已存在）
