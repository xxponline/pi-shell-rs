# Design

## Context

pi 侧（已读源码）：提交时 `text.startsWith("!")` 分流——`!!` 为 excludeFromContext；
命令为空则落到普通提交；bash 运行中警告 "A bash command is already running. Press Esc
to cancel it first." 并 `editor.setText(text)` **保留输入**；随后 `handleBashCommand` →
RPC 同款 `session.executeBash`（分片经 `_emit({type:"bash_execution_update", id, delta})`
推到 stdout，结束返回 BashResult{output,exitCode,cancelled,truncated,fullOutputPath}），
`recordBashResult` 落 `role:"bashExecution"` 消息（`excludeFromContext` 标记；流式时先入
pending 队列）。Esc 优先级：流式中止 > `isBashRunning`→`abortBash` > bash 模式清空 >
空输入双击；`isBashMode` = 输入 `trimStart().startsWith("!")` → 边框
`getBashModeBorderColor`（主题色，我们取黄色）。
我们侧：RPC `bash`/`abort_bash` 与事件 `bash_execution_update` 已存在但被忽略
（transcript.rs 归入 no-op 臂）；`submit_as` 当前把压缩门控放在斜杠分发**之前**
（压缩期间斜杠命令被排队——与 pi 次序不符，本变更顺带修正）。动机见 proposal.md - Why。

## Goals / Non-Goals

**Goals:**
- `!`/`!!` 分流、运行门控、Esc 中止优先级、边框提示、流式输出与完成态渲染，逐条对齐 pi
- bash 历史进 transcript 并可经 `get_messages` 快照重建
- 提交次序对齐 pi（斜杠 → `!` → 压缩排队 → 流式 steer → 常规）

**Non-Goals:**
- pi 的 `user_bash` 扩展拦截（本 TUI 无扩展运行时）
- bash 输出的外部截断文件（fullOutputPath）跳转查看（仅显示路径提示）
- shell 前缀/别名设置（`shellCommandPrefix`，RPC 侧由 pi 自理）
- 多命令并发排队（pi 同样单命令门控）

## Decisions

### D1. bash 状态归 SessionState，UI 只发命令
`SessionState` 增 `bash_running: bool` + `bash_output: String`（运行缓冲）与
`Entry::Bash { command, output, exit_code, cancelled, truncated, full_output_path,
excluded }`。理由：与 `compacting`/transcript 同层，`load_messages` 重建时可一并复位，
且 `bash_execution_update` 本来就进 `state.handle()`（现有 no-op 臂即插入点）。
App 发送 `bash` 时置 `bash_running=true`；`AsyncKind::BashDone` 携带结果与
attach（命令文本 + excluded，经 serde 编码）→ `state.finish_bash(...)` 落条目、清缓冲。
**单命令假设**：pi 与我们都门控同时只有一个 bash，`bash_execution_update` 不需要按 id
关联（事件里的 id 忽略）。

### D2. submit_as 重排为 pi 次序
`can_submit` → **斜杠分发**（prompt 路径）→ **`!` 分流** → 压缩排队 → 清空+发送。
- 斜杠前移：修正"压缩期间斜杠被排队"的既有偏差（斜杠非 prompt，不受 pi 压缩拒绝约束；
  session-management 的排队需求仍对 prompt 成立）
- `!` 分流在压缩门控**之前**（pi 同序：压缩中也能跑 bash）
- `!` 无命令体 → 落到常规发送；bash 运行中提交 `!cmd` → 黄条 + **不清空输入**
  （注意：分流必须发生在"清空输入"之前——重排后天然如此）
- `!` 分流仅 prompt 路径（alt+enter 的 follow_up 不拦截，与斜杠同策略）

### D3. Esc 三级优先级在一个拦截点表达
`handle_key` 现有空闲 Esc 拦截（`Esc + Ignore` 分支）内按序：
流式（`Abort` 意图，走不到这）→ **bash_running → abort_bash + return（不碰
last_escape）** → 双击判定。浮层/补全的 Esc 在更早分支已消费。与 pi 的
onEscape 链一致；bash 中止即使输入非空也生效（pi 在 empty 检查之前）。

### D4. BashDone 失败路径
`bash` 的业务结果（含 exitCode≠0、cancelled）都是 success 响应 → BashDone；
仅传输级失败走 `SendFailed`。处理：**SendFailed 到达且 `bash_running` 为真时**
视为 bash 失败（清 running、红条）——单命令门控下误归属面极小；设计上接受该取舍
（替代方案要给 UiEvent::SendFailed 加 kind，改动面大）。

### D5. 渲染
- **条目**（chat.rs）：`$ {command}` 行 + 输出行（弱化）+ 状态行
  （`exit N` / `cancelled` / `truncated → path` / `（不进上下文）`），输出渲染截断
  （保留末尾 500 行 + 截断提示；pi 侧 output 本身已按其策略截断）
- **运行中**：`chat_lines` 末尾追加 live 块（`⟳ $ cmd` + 已累积输出），复用现有
  in-flight 追加位置；不用动画 loader（spec 未要求，静态"运行中"足够）
- **边框**：`render_editor` 依 `ui.input.trim_start().starts_with('!')` 给边框
  黄色（pi 主题色的稳定近似），其余保持现状

### D6. 快照重建
`load_messages` 的 role 分支加 `bashExecution`：command/output/exitCode/cancelled/
truncated/fullOutputPath/excludeFromContext → `Entry::Bash`（缺字段按默认值，不 panic）。
未知 role 维持跳过。离线回放（replay/probe）不在本变更范围——其夹具不含该 role，
遇到时按未知事件忽略（现状即如此）。

## Risks / Trade-offs

- **超大输出**：live 缓冲与条目都做行数上限（500 行）+ 提示，避免撑爆渲染；
  pi 侧已有 truncated/fullOutputPath 兜底 → 我们显示路径不展开
- **SendFailed 误归属**（D4）→ 单命令门控 + 红条文案含 "bash"，可读即可定位
- **压缩中跑 bash 与排队交互**：bash 结果落 transcript 不经压缩门控（非 prompt），
  与 pi 一致；若压缩同时进行，`recordBashResult` 的 pi 侧 pending 语义由 pi 自理，
  我们只信 RPC 响应顺序
- **`!` 误触**：普通消息含 `!` 前缀会被执行——这是 pi 的既有语义（差异文档已注明），
  不做额外保护

## Open Questions

（无——分流/门控/优先级/事件流均已在 pi 源码确认。）
