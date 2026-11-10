# Proposal

## Why

工具执行显示是对话中曝光最高的元素，但当前实现与原生 pi TUI 差距明显：运行中的
命令看不到任何输出（`tool_execution_update` 的 `partialResult` 被状态层显式丢弃）、
运行中无计时、bash 折叠预览取头部导致 exit code 永远看不到、截断警告缺行数与
完整输出路径、折叠截断无"N more lines"提示——用户感知为"粗糙"。

## What Changes

- **状态层**：`tool_execution_update` 的 `partialResult` 存储为运行中的部分结果，
  `tool_execution_end` 到达时被完整结果替换（现状是 update 仅标记 running 存在）。
- **运行中流式输出**：卡片在工具执行期间实时显示已到达的部分输出。
- **计时**：运行中显示 `Elapsed 4.2s`（每秒刷新），结束后 `Took 4.2s`；时长格式
  对齐 pi（`4.2s` / `1m 30s` / `2h 3m 4s`，替代现在的裸毫秒）。事件循环在存在
  运行中工具时周期重绘。
- **背景面板**：整卡背景按状态着色（运行灰 / 成功墨绿 / 失败暗红），叠加在现有
  状态文字之上（不替代）。
- **头行对齐 pi**：bash `$ command` prompt 样式 + `(timeout Ns)` 后缀；read 行
  范围 `:1-20` 与折叠态 compact 分类（skill/docs/resource）；grep 显示
  `glob`/`limit`，find/ls 显示 `limit`；write 折叠预览行数与总行数；未知工具
  头行改 `key=value` 摘要（100 字符，展开态分行）。
- **折叠预览方向**：bash 折叠保留**尾部** 5 行（exit code/最终结果在末尾）+
  `... (N earlier lines, ctrl+o to expand)` 提示；其余工具补
  `... (N more lines, ctrl+o to expand)` 折叠提示。
- **截断警告**：bash 显示 `[Full output: <path>]` 与行数/字节截断详情；grep/find/ls
  显示各自 limit 警告（matches/results/entries limit、bytes limit）；read 显示
  首行超限/行数/字节三形态；均带具体数字。
- **更新节流**：bash 部分输出更新按 100ms 节流，避免高频 RPC 事件驱动全量重绘。

**移出本 change（已挂账至 `Docs/roadmap.md`）**：edit 执行中预计算 diff、
语法高亮、结果图片块渲染、卡片点击展开、args 流式分帧渲染优化。

**保持现状（已记录至 `Docs/pi-tui-interaction.md` #16/#17）**：状态文字不被
背景面板替代；read 折叠态保留内容预览——两条均为规格背离，本 change 不改。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `tui-app`：「工具调用卡片」需求扩展——运行中流式输出、运行/完成计时与格式、
  背景面板、bash 折叠取尾、折叠行数提示、截断警告详情；「write/grep/find/ls
  专用工具卡片」需求扩展——头行参数（glob/limit/预览行数）与各自 limit 警告。
- `session-state`：「工具执行归一化」需求扩展——`tool_execution_update` 的
  `partialResult` MUST 被记录为运行中部分结果，end 到达时被完整结果替换。

## Impact

- `src/session/transcript.rs`（`on_tool_update` 接住 partialResult）、
  `src/session/blocks.rs`（`ToolExecution` 增加运行中部分结果字段）
- `src/app/tools.rs`（头行/结果体渲染大头）、`src/app/chat.rs`（接线）、
  `src/app/view.rs`（背景面板样式）、`src/app/mod.rs`（event_loop 运行中周期
  重绘、节流）
- 既有单测：`tools.rs` 折叠断言（取头→取尾）、`transcript` update 归约断言
  需同步；新增流式/计时/警告场景测试
- 文档：`Docs/tui-usage.md` 工具卡片描述、`Docs/pi-tui-interaction.md` 差异
  清单补对齐行
