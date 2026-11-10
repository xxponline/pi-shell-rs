# Proposal

## Why

roadmap 挂账"工具卡片对齐 change 的移出项"④：工具卡片当前只能 `Ctrl+O`
**全局**切换展开，无法针对单张卡片。pi 的实现已核实
（`tool-execution.ts:createResultRegion`）：左键点击卡片**结果区** →
`setExpanded(!expanded)` 单卡切换，且仅在已有 result（非运行中）时响应。

本 change 是 roadmap"滚轮+卡片点击"块的**后半**：滚轮与鼠标捕获由
`add-mouse-wheel-scroll` 承担（前置，须先落地）——届时 Down/Up 事件已经
到达主循环，本 change 不再碰终端/事件捕获层，只剩纯逻辑：行号→卡片命中、
点击分发、单卡展开态。

## What Changes

- **点击命中表**：渲染时记录每个工具卡片占用的换行后行区间（`chat_lines`
  产物的行范围 → 卡片标识），App 保存上一帧命中表；点击坐标 → 行号 → 卡片。
  与 `viewport` 反馈同模式（渲染端产出、App 存储）。
- **点击语义**（对齐 pi）：左键点击**已有结果**的工具卡片 → 切换该卡片
  展开/收起；运行中（无 result）的卡片点击不响应；点击消息区其它位置
  （非卡片行）不动作。
- **单卡展开态**：从全局 `expand_tools` 标志扩展出按卡状态（toolCallId
  粒度），与全局标志的叠加规则（默认折叠策略、Ctrl+O 全局切换对单卡覆盖
  的关系）在 design 阶段定。
- **事件**：`event.rs` 转发点击类 `MouseEvent`（`Down`/`Up`，左键）为新
  `UiEvent` 变体——**捕获本身由前置 change 开启，本 change 不动
  terminal.rs**。
- **Spec**：`tui-app` ADDED「工具卡片点击展开」。
- **文档**：`tui-usage.md` 键位/操作表补"点击卡片切换展开"；
  `roadmap.md` 移出项 ④ 删除。

**移出本 change / 明确暂不做：**
- **滚动条（可视化、悬停反馈）——暂不做**，roadmap 挂账保留；
- hover 高亮/光标变化等悬停反馈（可后补，不阻塞点击）；
- 滚轮滚动（前置 `add-mouse-wheel-scroll`）；
- 点击会话树/模型选择器/补全菜单等其它组件的导航行为；
- 右键粘贴（pi 的 win32 特性）、scroll-to-end 指示器点击。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `tui-app`：新增 ADDED 需求「工具卡片点击展开」——钉住可观察行为：左键
  点击有结果的卡片切换其展开/收起（单卡粒度）、运行中卡片点击无效、
  Ctrl+O 全局切换行为不变。

## Impact

- `src/app/`（view/chat 或 tools）：卡片行区间命中表的产出与存储
- `src/app/event.rs`：点击事件转发（新 `UiEvent` 变体）
- `src/app/mod.rs`：点击分发 → 单卡展开态翻转
- 单卡展开态存放位置（flags/独立状态）→ design 阶段定，可能涉及
  `RenderFlags` 或新结构
- `Docs/tui-usage.md`、`Docs/roadmap.md`、`openspec` delta
- 测试：命中表纯函数单测（行号→卡片）、点击分发单测、PTY 注入
  SGR 点击序列（`\x1b[<0;x;yM` 按下 / `<0;x;yM` 抬起）屏幕断言
- **前置**：`add-mouse-wheel-scroll`（捕获已开）；本 change 不改 terminal.rs
