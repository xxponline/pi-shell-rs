# Proposal

## Why

独立窗口输入区当前的观感过杂：浅灰底 + gpui-kit 内建装饰（圆角、主题底色、
常态细边框、聚焦时 ring 变色与 focus 光环），验收反馈期望极简形态——
lighting（浅色）模式下**纯黑矩形边框**，获得焦点（可输入时）**边框加粗**，
其余效果一律不要。

## What Changes

- `Textarea` 加 `appearance(false)`：一步剥掉 kit 内建的底色、圆角、
  常态边框、聚焦变色与 focus ring（这些全部 gate 在 `appearance` 上，
  见 gpui-component `input.rs` 渲染链）。
- 容器自绘黑色直角边框：常态 1px、聚焦 2px（render 内读焦点柄；
  若焦点切换不触发重绘，补 `on_focus_change → notify` 一行）。
- 底色改白（方案 b）：移除输入区灰底，与消息区同底，仅靠黑框区分层次。
- `palette` 常量块增 `INPUT_BORDER`（黑），`INPUT_BG` 退役。
- **不改**：编辑语义（内建编辑器自理不变）、补全菜单、浮层、状态行、
  键位路由；无依赖变化；ratatui 路径零改动。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `gpui-agent-terminal`: 新增输入区简约边框需求（纯黑直角框、聚焦加粗、
  无内建装饰；编辑语义不受外观改动影响）。

## Impact

- **代码**：`src/gpui_agent/terminal.rs`（Textarea 构造 + 输入容器渲染 +
  焦点读取）与 `palette` 常量；可能一行焦点通知接线。
- **测试**：既有 GPUI 测试以文本快照为主，预期全绿；新增焦点态边框
  的纯函数断言（若有抽取）；手动开窗验收边框形态。
- **依赖 / 共享层**：零变化（不触碰 `app/`、`rpc/`、`session/`）。
