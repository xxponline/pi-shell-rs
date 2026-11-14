# Proposal

## Why

滚动增强挂账的最后一簇：消息区目前只能整页/滚轮线性翻，无法按**语义消息
边界**跳转（pi alt-screen 的 `ctrl+↑/↓`）或一键顶/底（`ctrl+home/end`），
半页滚动无键。`add-message-scrollbar` 刚交付的 `set_top_abs` +
`mark_scroll_activity` 正是这些跳转所需的全部原语——本 change 是其自然收尾。
pi 侧关键事实已探明：OSC133 标记是 pi **自产自销**（user/assistant 组件给
自己包 `133;A`、绘制时剥离，bash 组件 0 处标记）——零 shell 集成依赖。

## What Changes

- **prompt 跳转**：`ctrl+↑` / `ctrl+shift+↑` 跳至上一条消息起点，
  `ctrl+↓` / `ctrl+shift+↓` 跳至下一条（pi `tui.altScreen.previousPrompt/
  nextPrompt` 默认键实测）；语义消息 = **用户与 assistant 条目**（对齐 pi
  标记范围：通知与 `!` bash 执行不构成跳转点）；端点 clamp——已在最早消息
  再按上跳归顶、已在最末消息再按下跳贴底恢复跟随。
- **顶/底**：`ctrl+home` / `ctrl+end` 分别到消息区绝对顶/底（pi
  `tui.altScreen.top/bottom` 默认键实测：`ctrl+home`/`ctrl+end`）。
- **半页滚动**：`shift+PageUp` / `shift+PageDown` = 页长一半（至少 1 行）；
  补全菜单打开时保持既有菜单整页行为（前序拦截不变）。该键现误撞全页，
  按方案一批准修正为半页。
- **消息起始行标记**：以 `HitKind::MessageStart(seq)` 复用既有命中表
  （点击该行无动作，观察等价于 pi 内容行点击）——载体形态见 design D1。
- **明确不做**：行级滚动——pi `lineUp/lineDown` **默认键即为空**（动作留给
  用户 settings 绑定），我们无键位配置系统 → 无处可挂；挂账销账注明
  "pi 默认无键，已对齐"。跳转原语复用 `set_top_abs`/`mark_scroll_activity`
  （零新滚动 API）；`event.rs` 零改动（纯键位映射层）。
- 文档同步（`Docs/tui-usage.md` 键位表、`Docs/roadmap.md` 挂账更新）；
  主规格 tui-app 增加 ADDED 需求。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `tui-app`：新增 ADDED 需求「消息区提示跳转与滚动键」（消息边界跳转与端点、
  顶/底、半页、菜单/浮层下的键位语义）；既有「消息区滚动回看」「鼠标滚轮
  滚动」「消息区滚动条」需求语义不变（滚动行为与滚动条点亮规则复用），无需
  修改。

## Impact

- `src/app/chat.rs`：`chat_lines` 对用户/assistant 条目首逻辑行打
  `HitKind::MessageStart(seq)` 标记（seq 全局递增，通知/bash 不消耗）。
- `src/app/wrap.rs`：**零改动**（meta/`HitKind`/`collect_hits` 已泛化，
  新 kind 自动成段）；补分组单测。
- `src/app/mod.rs`：`key_intent` 新意图（`JumpPrompt(±1)`、`ScrollTop/
  Bottom`、`ScrollHalfPage(±1)`，分支置于 Edit 族与菜单前序之后）；
  分发复用 `set_top_abs` + `mark_scroll_activity`（top 变化才点亮，
  与既有规则一致）；`handle_click` 对 `MessageStart` 加 no-op 臂。
- `src/app/scroll.rs`：跳转目标纯函数 `prompt_jump_target(...)`（可测）。
- 测试 + PTY（跳转/顶底/半页/菜单与浮层下的键位语义）；Docs×2。
- 不改 RPC、session 层、`event.rs`。
