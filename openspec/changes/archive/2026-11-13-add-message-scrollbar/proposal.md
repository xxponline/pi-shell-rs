# Proposal

## Why

消息区键盘整页回看（`add-message-scrollback`）与鼠标滚轮（`add-mouse-wheel-scroll`）
都已落地，但**看不到当前位置**——滚到中段与贴底在视觉上无法区分；roadmap 挂账项
「滚动条可视化/悬停（暂不做）」经复看后确认要做。pi 原版有完整滚动条
（`scroll-view.ts` + `layout.ts` + `tui-alt-screen.ts` 实测）：auto 瞬态显示、
悬停加粗、以及挂账没写全的**拖拽拖动条**（点轨道跳转 / 抓 thumb 拖动）。

## What Changes

- **消息区右列滚动条（auto 模式，硬编码）**：
  - 可见性——内容总行数 > 视口高时，任一**滚动移动**（PageUp/PageDown、滚轮、
    回底等）后瞬态显示，1s 无滚动即隐；内容 ≤ 视口立即不显；内容增长本身
    不触发显示（对齐 pi `updateLayout` 语义）。
  - 几何——直接移植 pi 公式：轨道 = 区域最右列全高、覆盖内容末列；
    `thumbH = max(min(2,h), round(h*h/content))`；
    `thumbTop = round(top/maxTop * (h-thumbH))`。
  - 字形/颜色——轨道 `│`、thumb `┃`、悬停 thumb `█`；前景色对齐 pi theme
    `scrollbarTrack` / `scrollbarThumb`（dark okhsl(237 7% 33%) / okhsl(232 7% 65%)，
    经既有 oklab 换算先例转 RGB）。
- **悬停**：转发鼠标 `Moved` → 指针命中滚动条列且在轨道行内 → thumb 加粗为
  `█` 且不受 1s 计时隐藏（常显）；离开恢复 `┃` 并重启常规计时。仅在悬停状态
  **翻转时**重绘（Moved 洪水不炸帧）。
- **拖拽 scrub**：该列上按下（`Down(Left)`）被消费——不穿透为卡片/文本行点击；
  点轨道任意处 → thumb 中心跳到指针；抓 thumb → 记 grabOffset；拖动（`Drag`）
  → `thumbOffset` 按比例映射 `scrollTop`；抬起（`Up(Left)`）结束。相应翻转
  `event.rs` 的 Moved/Up/Drag 丢弃约定与边界测试（同滚轮/卡片点击先例）。
- **状态与绘制落点**：瞬态计时与 hover/drag 状态在 App 层（复用事件循环
  `recv_timeout` 超时支判隐）；几何与绘制进纯函数层（`wrap`/`view`，无 TTY 可测）。
- 文档同步（`Docs/tui-usage.md`、`Docs/roadmap.md` 挂账项更新）；主规格 tui-app
  增加 ADDED 需求「消息区滚动条」。

**明确不做**（本轮范围外）：settings 可配置（`always`/`hidden` 模式——无 settings
体系，硬编码 auto，同前轮决定）；滚动条列上的滚轮差异拦截（我们滚轮不看指针
位置，行为不变）；alt-screen 搜索、prompt 跳转等其余滚动挂账；窄内容/空内容的
特殊美化。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `tui-app`：新增 ADDED 需求「消息区滚动条」（瞬态可见性与几何、悬停、拖拽
  消费与跳转）；既有「消息区滚动回看」「鼠标滚轮滚动」需求语义不变（滚动行为
  本身零改动），无需修改。

## Impact

- `src/app/event.rs`：`Moved` / `Up(Left)` / `Drag(Left)` 转发为新 `UiEvent`
  （源线程不加状态）；既有「仅 Down(Left) 转发」边界测试按计划翻转改名。
- `src/app/mod.rs`：滚动条瞬态状态（滚动活动时间戳）、hover/drag 状态机、
  上帧 `chat_area` Rect 存档（列判定）、超时支判隐、新事件分发臂。
- `src/app/wrap.rs` / `src/app/view.rs`：滚动条几何与绘制（纯函数，含 total
  行数来源——design 取舍：wrap 内绘制 vs `ChatScroll` 记 content_len）。
- `src/app/scroll.rs`：滚动活动标记的触发点归拢（page/wheel/回底）。
- 测试：几何/状态机单测 + PTY 手验（瞬态显隐、thumb 位置、悬停、拖拽）；
  `Docs/tui-usage.md`、`Docs/roadmap.md`。
- 不改 RPC 协议、不改 session 层；工具卡片/thinking 点击语义仅在**滚动条列上**
  被消费（对齐 pi），其余行不变。
