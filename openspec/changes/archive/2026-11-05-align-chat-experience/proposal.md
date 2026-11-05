# Proposal

## Why

路线图阶段 3（体验对齐）：阶段 2 的 TUI 已能对话、流式、中断、报错，但与原生 pi 并排对比时差距集中在聊天面的呈现质量——回复是纯文本无 markdown、thinking 是固定占位、工具只有一行状态、状态行缺 tokens/cost、队列不可见、auto-retry 静默。本变更把这 6 项一次做完，使"与 pi 基本无差别"的阶段验收成立。

## What Changes

- **markdown 渲染**：assistant 文本从纯文本改为 markdown 渲染——标题/粗体/斜体/行内代码/代码块（带边框与语言标签）/列表/引用/链接；代码块做语法高亮（方案在 design 对比后选定，倾向 pulldown-cmark + syntect）。流式中的不完整 markdown（如未闭合围栏）不得闪烁或崩溃。
- **thinking 折叠**：默认显示单行占位，`ctrl+t` 在折叠/展开间切换（对齐 pi 的 `app.thinking.toggle`）；展开时以弱化样式显示完整思考文本。
- **工具卡片**：read / bash / edit 从单行状态升级为结构化卡片——read 显示文件路径与内容预览、bash 显示命令与输出、edit 显示 diff（红绿），均带状态与耗时；`ctrl+o` 切换输出展开/折叠；未知工具走通用卡片（不崩）。为获取 bash/edit 的真实结果结构，向 probe 增补两个工具场景并归档 fixture。
- **状态行补全**：显示模型名、tokens（in/out/cache）、费用（cost）；流式中随 `message_update` 的 usage 实时刷新。
- **队列可见与 follow_up**：status/输入区上方显示 steering/follow_up 队列内容；`alt+enter` 发送 follow_up（流式中排队），`alt+up` 把队列尾部的 follow_up 取回输入框（对齐 pi 的 `app.message.followUp` / `app.message.dequeue`）。
- **auto-retry 提示**：`auto_retry_start/end` 事件渲染为对话流中的通知行（含 attempt/max/错误摘要）；同时把已实现但未入规格的"assistant 运行级错误 → 红色 error 通知"补进 session-state 规格。
- **session 层配套**（修改既有能力）：`Usage` 增加 cost 字段；`auto_retry_*` 归约为通知条目；新增把 `get_state` 响应（model/sessionName/thinkingLevel）应用到状态的方法。
- **不在本变更范围**：表格、LaTeX、mermaid、图片（路线图第三档/后续）；write/grep/find/ls 之外工具的专用卡片（先走通用卡片）；会话/模型选择器、compact 流程（阶段 4）；主题系统、编辑器多行历史与自动补全。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `tui-app`: 新增聊天面呈现与交互需求——markdown 渲染、thinking 折叠、工具卡片、状态行 tokens/cost/model、队列显示与 follow_up 键、retry/error 通知显示。
- `session-state`: 新增状态扩展需求——Usage 携带 cost、auto_retry 事件归约为通知、get_state 响应应用、运行级错误通知（补记已实现行为）。

## Impact

- **依赖**：新增 markdown 解析与语法高亮库（design 选定后引入，预计 pulldown-cmark + syntect）。
- **代码**：`src/app/chat.rs` 显著扩展（markdown/工具卡片渲染，纯函数保持可测）；`src/app/view.rs`（状态行、队列条）；`src/app/mod.rs`（ctrl+t/ctrl+o/alt+enter/alt+up 键位与展开状态）；`src/session/transcript.rs`（Usage/case/auto_retry/get_state）；`src/bin/probe/scenarios/`（+2 工具场景）。
- **测试**：现有 48 项继续通过；新增 markdown/卡片/状态行/键位的纯函数单测 + 新 fixture 离线断言；语法高亮引入后注意编译时间与二进制体积。
- **fixtures**：新增 bash/edit 两个真实场景 snapshot（探针既有能力，不改 rpc-probe 规格——其场景要求为"至少包括"）。
- **文档**：`Docs/tui-usage.md` 更新键位与界面说明；`Docs/roadmap.md` 阶段 3 验收勾选。
- **无破坏性变更**。
