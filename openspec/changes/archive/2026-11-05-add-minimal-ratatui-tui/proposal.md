# Proposal

## Why

路线图阶段 2：rpc 连接层与无头 session 状态机（已归档的 `rpc-client` / `session-state`）已就绪并可离线测试，但还没有任何可见界面——主 bin 仍是 hello world。现在需要第一次把 ratatui 接起来，用真实 `pi --mode rpc` 打通"键盘输入 → prompt → 流式回复 → 中断"的最小闭环，尽早在真实终端验证终端库选型、事件循环并发模型与 session 状态到画面的映射，为阶段 3 的体验对齐铺底。

## What Changes

- 引入终端栈依赖：`ratatui` + `crossterm`（含其 event/terminal 能力），仅用于新的 `tui` 主 bin。
- 把 `src/main.rs` 实现为真正的 TUI 入口（替换 hello world），或新增 `src/bin/tui.rs` 并保留默认 bin——实现时以最简为准（倾向直接让 package 默认 bin 即 TUI）。
- 最小界面（五区骨架的精简版，阶段 2 只做必需三块）：
  - **滚动消息区**：渲染 `SessionState` 的 transcript（用户消息、assistant 的 text；thinking/工具块先以占位/单行摘要显示，不做折叠卡片）。
  - **底部输入区**：单行/基础多行文本编辑，回车发送 prompt。
  - **顶部标题/状态行**：会话名（若有）与 phase（流式/空闲）。
- **流式渲染**：除已落盘 transcript 外，还要实时显示当前正在生成、尚未 `message_end` 的 in-flight assistant 内容（text/thinking delta）。为此给 `SessionState` 增加只读的 in-flight 访问器（库的小幅扩展，不改状态归约语义）。
- **事件循环**：用 std 线程 + mpsc 桥接三类事件源——crossterm 键盘事件、`RpcClient` 的事件通道、渲染 tick；把 RPC 事件喂给 `SessionState`，发送用户输入时调用 `prompt` 命令。
- **中断**：流式中按 esc 发送 `abort`；空闲时 esc（或 q）退出程序并恢复终端。
- 启动时做 `get_state` 探活（复用客户端能力），未登录/启动失败要在终端给出可读错误并干净退出，而不是挂起或 panic。
- 终端 raw mode / alternate screen 的进入与退出，确保 Ctrl-C/panic 时也能恢复终端（含 panic hook 或 RAII guard）。
- **不在本变更范围**：markdown 高亮、thinking 折叠交互、工具结果卡片、follow_up/steer 队列交互、模型/会话选择器、extension UI 弹窗、多 turn 复杂状态、外部编辑器、图片——全部留待阶段 3+。输入区先做基础编辑（字符输入/退格/回车），不做 pi 那个 2483 行编辑器。

## Capabilities

### New Capabilities

- `tui-app`: 在通用终端中以 ratatui 运行的最小交互界面——管理终端生命周期与事件循环，把键盘输入转为 RPC prompt、把 session 状态渲染为滚动对话，并支持流式显示与中断。它是后续所有终端交互能力的宿主。

### Modified Capabilities

- `session-state`: 需要新增对"正在流式、尚未落盘的 in-flight assistant 内容"的只读访问，使 TUI 能实时渲染 delta。状态归约语义与既有回放/transcript 行为不变；现有离线测试继续通过。

## Impact

- **依赖**：`Cargo.toml` 新增 `ratatui`、`crossterm`（features 按需最小开启）。
- **代码**：
  - 新增 `src/app/`（事件循环、终端 guard、布局、chat/editor/footer 渲染）或等价模块。
  - `src/session/transcript.rs` 增加只读访问器（小改库）。
  - `src/main.rs` 由 hello world 变为 TUI 入口。
  - `rpc::RpcClient` 可能需要一个把后台事件从 `Receiver` 桥接进 UI 线程的用法（不改库，消费方自行起线程）。
- **测试**：session 访问器为纯逻辑可单测；ratatui 渲染层以可在无 TTY 下测试的纯函数（`SessionState → 行/片段`）拆出，事件循环与终端 IO 部分以手工真机验证为主。
- **运行时**：需要本机已登录 pi 才能完整对话；未登录/无 pi 时需可读报错。
- **无破坏性变更**：probe 二进制与既有 rpc/session 规格行为保持不变。
