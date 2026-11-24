# Proposal

## Why

独立窗口验收暴露两块短板：

1. **文字看不清**：我方 16 处硬编码深色面板底（消息/状态/输入/补全）与
   Light 主题的内建控件深色字相撞，深字深底，观感像"坏掉的暗色模式"。
2. **悬浮层/选择器整体缺席**：共享层 `Overlay`（14 个变体）+ `overlay_key`
   纯函数 + 队列 + 数据入口早已完备、TUI 全量可用，GPUI 却既没画也没路由，
   `/model`、alt+t、`/tree`、`/settings` 等入口只能退化成 notice 或
   "暂不支持"桩——这是当前最大的功能缺口。

趁 `add-gpui-support` 刚归档、架构记忆新鲜，一次补齐：独立窗口达到
"能日常用"，完整主题系统另行开 change。

## What Changes

- **浅色模式固定（可读性 pass）**：独立窗口启动时显式
  `Theme::change(ThemeMode::Light, None, cx)`（不订阅系统外观）；16 处
  硬编码面板底色浅色化，使内建控件文字在浅色主题下可读。完整主题系统
  （ActiveTheme token 化、深浅热切换、TuiTheme 注入对齐）**不在本变更**。
- **悬浮层渲染（14 变体全量）**：新增 GPUI 浮层渲染层（居中面板 + 遮罩），
  从共享 `App.overlay` 状态直绘全部变体——列表/查看型：ModelSelect、
  ThinkingSelect、SessionSelect、ScopedModels、Info、Tree；面板/确认型：
  Settings、Hotkeys、Trust、ImportConfirm；扩展对话型：ExtSelect、
  ExtConfirm、ExtInput、ExtEditor（由既有 RPC 扩展请求入队，GPUI 只需
  渲染与回发）。样式取内建控件与主题 token，**状态与按键 100% 留在共享层**
  （不采用 kit 状态化 Select/Combobox/Dialog 作控制器，避免双源真相）。
- **按键与滚轮路由**：浮层打开时全部按键 → `App::handle_key` 并
  `stop_propagation`（Textarea 不截胡），滚轮 → `App::handle_wheel`；idle
  时把打开键（ctrl+l、alt+t 等，镜像共享 `key_intent` 表）纳入拦截。
- **斜杠入口扩容**：GPUI 斜杠分发从"查讯子集 + 其余桩"扩为"查讯子集 +
  可开浮层命令（model / thinking / tree / settings / trust / hotkeys /
  session / import / scoped-models 等）+ 其余桩"；无参 `/model`、
  `/thinking` 从用法提示改为真开选择器（TUI 同款语义）。
- **共享层仅加法**：`app/` 只放宽 overlay 相关可见性（`pub(crate)`），
  无删除、无行为变更；既有测试套件保持全绿。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `gpui-agent-terminal`: 新增浅色模式固定、悬浮层渲染、悬浮层按键与
  滚轮路由、悬浮层入口四组需求；斜杠子集语义由"仅查讯型"扩展为
  "查讯型 + 可开浮层命令"。

## Impact

- **代码**：`src/gpui_agent/{window,terminal}.rs`，新增
  `src/gpui_agent/overlay.rs`（浮层渲染）；`src/app/{mod,overlay}.rs`
  仅可见性放宽；`Docs/tui-usage.md` GPUI 段更新。
- **依赖**：无新增（gpui-kit / gpui-component 既有内建）；feature 门控
  不变；ratatui 路径与 embed 管线零改动。
- **测试**：双矩阵 `cargo test` / `cargo test --features gpui-support`
  + clippy 双列零告警；新增 14 变体逐个冒烟、按键路由单测、
  fake-pi E2E（alt+t 开选择器 → esc 关闭）。
