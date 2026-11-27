# Proposal

## Why

edit 工具卡目前只有整行红绿（`details.diff` 逐行按前缀着色、降级路径行级
LCS 也整行），而 pi 已有两层体验：`renderDiff` 在「恰 1 行改 1 行」时做
**词级行内高亮**（共同缩进剥离 + 变更词反视频），以及执行前的**异步
preview**（args 齐全即读文件算 diff 先行展示、结果到达后替换）。二者是
roadmap 明确挂账的移出项①②，explore 已定全量承接（含降级路径同染）。

## What Changes

- **共享行内 diff 纯函数**（`src/app/tools.rs`）：词级 LCS（词/空白/标点
  分词、token 乘积上限保护、无新依赖），pi 算法规则对齐——共同前导缩进
  先剥离、首个 removed/added 段的前导空白保留不染、变更 token 标记。
- **`details.diff` 路径**：连续 `-` 块与 `+` 块配对，恰 1-1 时按行内分段
  着色（行色红/绿不变、变更词加反视频），多行块维持整行（pi 同）；
  上下文/`@@` 行为不变。
- **降级 `line_diff` 同染**：old/new 单编辑对恰是 1-1 场景，按同一规则
  分段（explore 决策：不同染则罕见路径反而比 pi 差）。
- **执行前 preview（①）**：edit 卡 args 完整且结果未到（Pending/Running）
  时，app 层异步读文件 + 算 preview diff 先行展示（红绿 + 行内）；结果
  到达以 `details.diff` 替换；文件缺失/oldText 不匹配走错误态（红底头行
  + 错误文案，pi `EditDiffError` 同款）；会话重建/切换清空预览态。
- **双表面消费**：TUI 走 ratatui `Modifier::REVERSED` 原生反视频；GPUI 的
  `line→run` REVERSED 管线与 `lines_to_segs` 已就位，零渲染改造。
- 规格：`gpui-agent-terminal` MODIFIED `edit 卡 diff 渲染` + ADDED
  `edit 执行前预览`；`tui-app` MODIFIED `工具调用卡片`（edit 条款收紧）+
  ADDED `edit 执行前预览`。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `gpui-agent-terminal`：`edit 卡 diff 渲染` 增 1-1 行内反视频与降级同染
  场景；新增执行前预览需求。
- `tui-app`：`工具调用卡片` 的 edit 条款收紧（行内高亮）并补场景；新增
  执行前预览需求（共享渲染，双面同源）。

## Impact

- 代码：`src/app/tools.rs`（词级 LCS + `edit_body`/`line_diff` 接线）、
  preview 异步管道（app 层状态字段 + spawn/回填通路，复用既有
  spawn_async/UiEvent 模式）、渲染两表面零改（REVERSED 管线已就位）。
- 不动：session 归约层语义、pi RPC、`@@` 青色（我们自定 spec）、args
  流式分帧（移出项④，纯性能）。
- 测试：共享纯函数单测（TUI 线级断言 + GPUI Seg 样式断言）、preview
  e2e（先行展示/结果替换/错误态/清空）、双矩阵、PTY 对照 pi 手感。
- 文档：`Docs/tui-usage.md` 工具卡段补行内高亮与 preview 描述。
