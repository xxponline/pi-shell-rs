# Proposal

## Why

路线图阶段 4a（经确认拆分方案 A 的第一半）：TUI 已具备聊天主界面（阶段 2/3），但所有"弹层式交互"完全缺失——无法切换模型/思考级别，扩展发起的 UI 请求（select/confirm/input/editor 等）被静默丢弃，write/grep/find/ls 四种工具仍走通用兜底卡片。这些交互共享同一套 overlay 基础设施，适合一次建好；会话管理与会话树（最重的部分）留到 4b。

## What Changes

- **overlay 弹层框架**：居中浮层（`Clear` 打底 + 居中 rect）、焦点键路由（弹层打开时优先消费方向/回车/esc）、关闭后完整恢复底层界面。所有选择器与扩展弹窗复用此框架。
- **model 选择器**：`ctrl+l` 打开（`get_available_models` 拉取列表），上下导航、回车应用（`set_model`）并更新状态行模型名，esc 取消；同时补 `ctrl+p` / `shift+ctrl+p` 模型循环（`cycle_model`，对齐 pi 键位）。
- **thinking 级别选择**：`shift+tab` 循环（`cycle_thinking_level`）并即时生效；可打开级别列表选择（`get_available_thinking_levels` → `set_thinking_level`），状态行同步。
- **extension UI 对话方法（4 种需回复）**：`select` / `confirm` / `input` / `editor` 请求到达时弹出对应浮层，用户提交回发 `extension_ui_response {id, value|confirmed}`、esc 取回发 `cancelled`；editor 为终端内多行编辑覆盖层（prefill 可编辑，非 $EDITOR）。等待回复期间不阻塞事件循环。
- **extension UI 被动方法（5 种免回复）**：`notify` → 对话流通知行；`setStatus` → 状态行显示；`setWidget` → 编辑器上方 widget 区；`setTitle` → 终端窗口标题；`set_editor_text` → 写入输入框。收到这些请求 MUST NOT 等待或发送回复。
- **其余工具卡片**：write / grep / find / ls 专用卡片——write 显示目标路径与内容摘要，grep 显示 `/pattern/ in path` 与匹配输出行（截断标记），find 显示 pattern/path 与结果，ls 显示目录条目；复用 `ctrl+o` 展开折叠与既有状态/耗时头行。结果形状防御式读取（实测 grep/find/ls 主要为 content 文本 + details 元数据，无 structuredContent）。
- **不在本变更范围**：session 选择器、会话树、新建/重命名/删除会话、compact 流程（4b）；外部编辑器 `$EDITOR`（ctrl+g，后置）；图片、主题、mermaid（路线图第三档）；选择器的输入即筛（fuzzy filter）——先做导航选择，filter 视实现成本后置。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `tui-app`: 新增弹层与选择能力需求——overlay 框架、model/thinking 选择器与模型循环、extension UI 9 种 method 的处理、write/grep/find/ls 专用工具卡片。

## Impact

- **依赖**：无新 crate（ratatui 自带 `Clear` 与布局能力）。
- **代码**：新增 `src/app/overlay.rs`（弹层框架与通用列表/输入/确认组件）与 `src/app/dialogs.rs`（extension UI 9 method 的分发与回复）；`chat/tools` 增加 4 种卡片；`view.rs` 增加浮层与 widget 区渲染；`mod.rs` 增加焦点键路由（弹层打开时拦截方向/回车/esc）、`extension_ui_request` 事件分发、ctrl+l/ctrl+p/shift+tab 等键位。
- **事件流**：`extension_ui_request` 目前被 `SessionState::handle` 静默忽略（`_ => {}` 分支），本变更加 App 层分发；session 层归约语义不变。
- **测试**：现有 89 项继续通过；新增选择器导航/弹层焦点/extension 回复与取消/被动 method/4 种卡片的纯函数与状态单测；extension UI 用既有 `extension_ui` fixture 回放 + 假 pi PTY 端到端验证。
- **文档**：`tui-usage.md` 键位与界面更新、roadmap 阶段 4a 标注。
- **无破坏性变更**。
