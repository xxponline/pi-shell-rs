# Proposal

## Why

GPUI 独立窗口（`pi-shell-gpui`）的消息区渲染是 P0 简化版：工具卡只有"名字+状态+首行预览"，缺头行参数、计时、折叠策略与受保护尾部；thinking 缺空块跳过与单块展开；通知行缺 `[kind]` 前缀；`!` bash 条目方向取错（取头 5 行而 TUI 取尾 500 行）；`ctrl+t`/`ctrl+o` 未进键路由白名单导致全局展开切换在窗口里不可达；队列条与 extension widget 面板完全未渲染。与原生 pi 并排对比时差异明显，且两套卡片逻辑若各写一份将从此漂移。

## What Changes

- **共享层抽中性语义模型（仅加法）**：从 `app/tools.rs`、`app/chat.rs` 抽出与渲染框架无关的卡片语义 API（工具卡 `Seg` 段序列模型：头行参数/折叠策略/受保护尾部、通知行前缀样式、bash 条目尾部截断与状态行、thinking 空块跳过），返回结构化数据而非 ratatui `Line`；现有 `tool_lines_themed`/`chat_lines_themed` 改为基于该模型组装，行为零变化、存量测试全绿。
- **GPUI 工具卡对齐 TUI 语义**：头行 `● name · 关键参数 · 状态 · 计时`、按工具折叠（bash 尾 5 / grep 15 / find·ls 20 / 其余头 10）+ `... (N lines, ctrl+o to expand)` 提示、受保护尾部（exit 行 + 截断警告）、三态面板底色、运行中 partial 实时渲染；edit 卡以**内建文本 runs** 画红绿 diff（不引入 `Diff` widget）。
- **thinking 块对齐**：空块不占行、折叠占位 `<thinking N chars>`、展开正文弱化样式；`Collapsible` 内建控件 + 单块点击展开。
- **键路由修复**：`route_keystroke` 白名单补 `ctrl+t`（thinking 全局切换）、`ctrl+o`（工具卡全局展开），进入共享 `App::key_intent` 分发。
- **计时走字**：存在运行中工具时起 1s interval 任务 `cx.notify`，`Elapsed` 每秒刷新（对齐 TUI 事件循环 tick）。
- **通知行对齐**：`[kind]` 前缀 + error 粗红 / retry 黄 / 其余弱化斜体；用内联文本行，不引入 toast `Notification` widget。
- **`!` bash 条目对齐**：尾部 500 行保留 + `…（已截断 N 行）` + 状态行（`exit 0` 绿 / `exit N` 红粗 / `cancelled` / 运行中）+ `（不进上下文）` 标记。
- **补齐队列条**：输入区上方渲染 `[steer]`/`[queue]`/`[queued]` 行（共享 `queue_lines` 语义），空队列不占位。
- **补齐 extension widget 面板**：`setWidget`/`clearWidget` 事件不再静默丢弃，渲染带 ` widget ` 标题边框的面板。

## Capabilities

### New Capabilities

（无——卡片语义的"是什么"已由 `tui-app` 定义，本变更不新增能力边界）

### Modified Capabilities

- `gpui-agent-terminal`: 消息区渲染面从"P0 简化卡"升级为与 TUI 同语义（工具卡全语义、thinking 空块/单块展开、通知行前缀、bash 条目、队列条、widget 面板）；全局键位补 `ctrl+t`/`ctrl+o`；内建控件渲染模型明确"语义来自共享中性模型、渲染用内建控件"的分工。

## Impact

- **共享层（加法）**：`src/app/tools.rs`、`src/app/chat.rs` 新增中性模型 API；两文件既有导出行为不变（`gpui-agent-terminal` 的"共享代码仅加法演进"要求 + 存量测试兜底）。
- **GPUI 渲染**：`src/gpui_agent/terminal.rs`（`tool_card`/`assistant_block`/`notice_row`/`bash_row` 重写、interval 计时任务、队列条与 widget 面板）、键路由白名单。
- **依赖**：仅用 gpui-kit / gpui-component 内建（`Collapsible`、`AccordionItem`、`Kbd`、`Spinner`、文本 runs），不引入 `Diff`/`Notification` 组件。
- **不影响**：ratatui TUI 行为与外观、`rpc/`/`session/` 层、默认特性构建产物。
