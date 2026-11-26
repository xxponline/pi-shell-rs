# Design

## Context

见 proposal.md 的 Why。约束与现状：

- 输入区当前 = 浅灰底容器 + `Textarea::new` 默认态（kit 内建：主题底色、
  圆角、常态 1px 边框、聚焦 ring 变色与 focus 光环）；根栈为自上而下
  flex（消息区 `flex_1` 吸收余量 → 状态行 → 输入区 → 补全容器贴底，
  即**底部锚定**布局）。
- gpui-kit 按 `=0.9.0` 钉版；`Textarea` 是 `Input` 的薄壳
  （`TextareaState = InputBaseState<TextareaMode>` 类型别名）。
- 浮层以 `absolute().inset_0()` 锚定根容器（add-gpui-overlays D2）。
- 焦点句柄已存在（拦截层用 `is_focused` 判键），gpui 实体仅在 notify 时
  重跑 render，焦点变化不自动触发。

## Goals / Non-Goals

**Goals:**

- 浅色（lighting）下输入区 = 白底（与消息区同底）+ 纯黑直角边框；
  常态 1px、聚焦加粗 2px；无圆角、无光环、无 kit 内建装饰。
- 验收反馈间距：输入框离窗口底边 10px；状态行与输入区间留 10px 消除重叠。
- 零行为变化：编辑语义、键位路由、补全菜单、浮层、状态行、ratatui 路径
  均不受影响；无依赖与共享层改动。

**Non-Goals:**

- 主题系统（ActiveTheme token 化、深浅切换）——后续 change。
- 输入框前缀/后缀元素、内联 token、placeholder 等装饰面升级。
- kit `prefix/suffix`（`Input::from_state` 为 `pub(crate)`，且本变更不需要）。

## Decisions

### D1 kit 装饰一次性剥除：`appearance(false)`

`gpui-component input.rs` 渲染链中，底色、圆角、常态边框、聚焦时
`ring` 变色、`focus_ring_style` 全部 gate 在 `appearance && bordered` 上。
`Textarea::new(...).appearance(false)` 一行全部关闭，`bordered/
focus_bordered` 随之成为无操作——比逐项关（圆角、聚焦色、光环）更稳，
不会漏。

**备选**：改主题 token（input 色、radius）→ 否决：全局副作用，且主题
系统是后续 change 的边界。

### D2 边框自绘在容器，焦点切换 = render 读 + 订阅重绘

容器 `div#gpui-input` 自绘 `border_1()`（聚焦 `border_2()`）+
`border_color(INPUT_BORDER)`；宽度在 render 内读
`self.focus.is_focused(window)` 分支。焦点变化不重绘实体，故在 `new()`
注册 `cx.on_focus_in/out` → `cx.notify()`（订阅存组件字段 RAII 反注册）。

**备选与否决**：
- gpui Div 的 `.focus(|s| s.border_2())` 状态样式——对"焦点在子孙
  （Textarea 编辑器）而非容器本身"的语义不确定，弃用；选订阅 + render
  读的确定性路径。
- 只在下一次渲染顺带更新（靠打字等既有 notify）——焦点进出而无输入时
  会停留旧宽度，不可接受。

### D3 间距落在补全容器与状态行，不碰根容器

- 底部 10px：加在**补全容器**（栈内最后一个子元素，空菜单时高度 0 +
  margin 即把整栈上推）→ 输入框离底边 10px，补全打开时菜单仍贴输入框。
- 状态行 10px：`mb(px(10.))` 于状态行容器 → 与输入区留隙，消除重叠
  （状态行净上移 20px，输入框净上移 10px，与验收口述一致）。

**备选与否决**：根容器 `pb(10)` —— 会给 `absolute().inset_0()` 的浮层
锚定引入 10px 命中缝（遮罩/滚轮阻断可能漏边），规避。

### D4 调色板：`INPUT_BG` 退役，增 `INPUT_BORDER`

白底 = 移除容器 `.bg()`、随根 `CHAT_BG`；黑色边框常量进 `palette`
（与其它面板色同处，后续主题 change 统一替换）。

## Risks / Trade-offs

- [焦点订阅若未触发，边框停在旧宽度] → 订阅为 RAII 字段随实体存活；
  `on_focus_in/out` 为 gpui 官方监听；1.3 已开窗目视往返切换确认。
- [边框/边距改动影响既有布局断言] → 双矩阵回归（415 / 461）+ clippy
  双列零告警锁定；边框为 border-box 内含不改布局盒，margin 仅改位置。
- [间距目视属主观验收] → 1.4 与 3.1 合并为一次开窗目视，已由用户确认。

## Migration Plan

纯外观改动，无迁移；回滚 = git revert 该 change 的 feat 提交。

## Open Questions

无——技术决策已在探索与验收反馈中全部敲定（D1–D4）。
