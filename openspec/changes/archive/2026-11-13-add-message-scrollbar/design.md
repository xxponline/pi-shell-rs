# Design

## Context

动机见 proposal.md。现状约束（实测）：

- 滚动状态 `ChatScroll{top, following, viewport}`（`scroll.rs`，Copy），
  `wrap_for_viewport` 每帧窗口化：drain 前拿得到内容总行数 `len`、
  `effective_top`、`height`——**几何所需三元组全在手，但 total 未外传**。
- 事件循环：`ui_rx.recv_timeout(200ms)` 超时支 + `timer_redraw_due`
  （现仅工具走字）——已有周期性 tick 可挂判隐。
- `event.rs` 源线程丢弃 `Moved / Up(Left) / Drag(Left)`（边界测试钉着，
  与滚轮/卡片点击同一批历史约定）。
- pi 实测（本轮 explore）：auto 模式瞬态（滚动即显/1s 隐/内容增长不触发）、
  几何公式、字形 `│┃█`、theme 双色、**悬停揭示隐藏条**（`includeHiddenAuto=true`）、
  列上按下消费 + 点轨道跳转 + 拖 thumb scrub（`tui-alt-screen.ts:954-1140`）。

## Goals / Non-Goals

**Goals:**

- pi 全量三件套：瞬态可视化 + 悬停（加粗常显、并揭示未显示的条）+ 拖拽 scrub。
- 几何/绘制纯函数化（无 TTY 单测）；状态机边沿触发（Moved 洪水不炸帧）。
- 既有滚动语义（PageUp/Down、滚轮、卡片/thinking 点击）零改动——仅滚动条列
  上的按下被消费。

**Non-Goals:**

- settings 可配置（`always`/`hidden` 模式、hideDelay、色值覆盖——硬编码 auto
  与 dark token，同前两轮无 settings 的决定）。
- 滚动条列上的滚轮差异（我们滚轮不看指针位置，行为不变）。
- alt-screen 搜索、prompt 跳转等其余滚动挂账；内容不足一屏时的任何美化。

## Decisions

### D1 total 载体 = `ChatScroll.content_len`（+1 字段）

`wrap_for_viewport` 在回写 `next` 时置 `next.content_len = len`（drain 前总行数），
零签名变化；App 经 `self.scroll.content_len` 拿到**上帧** total——与
`card_hits` 同一"上帧反馈"模式，帧内自洽。

- **替代 (b) 返回四元组**：否——wrap 测试 8 处三元解构 + `render` 返回值 +
  `Cell` 消费者连锁；且 content_len 本就是视口状态族（top/following/viewport
  伴生字段），概念归属正确。
- 波及：15 处 `ChatScroll{...}` 字面量补 `content_len: 0`（测试为主，机械，
  编译器兜底）；`Default` 实现补0。

### D2 几何与绘制进 `wrap.rs`（纯函数 + 共用几何）

```rust
pub fn scrollbar_geometry(content_len: usize, viewport: usize, top: usize)
    -> Option<ScrollbarGeometry>   // None = 内容不超视口（不画、不可命中）
// thumb_h = max(min(2, h), round(h*h/content))
// thumb_top(top) = round(top/max_top * (h - thumb_h))   ← 直接移植 pi 公式
```

- **共用**：wrap 绘制与 App 悬停/拖拽命中判定调同一函数、同帧同参
  （`content_len` 来自 D1、`top/viewport` 来自 `scroll`），判定与画面不可能
  错位。
- 绘制：窗口化 + bg 补齐**之后**（覆盖末列）；行处理 = 截断显示宽至
  `width-1` 再补"空格×n + 滚动条字符"span——CJK 双格尾字自然整字丢弃，
  不留半字（pi 的 grapheme 替换同义）。样式 = fg token 色、**清 bg**
  （对齐 pi `replaceScrollbarCell` 整格替换，工具卡面板底色在该格让位）。
- `wrap_for_viewport` 增参 `scrollbar: Option<ScrollbarOverlay{hovered: bool}>`
  （None=本帧不画）；字形：轨道 `│`、thumb `┃`、hovered thumb `█`。
- 颜色：dark.json 两 token okhsl→RGB，走既有 `tool_panel_bg` 同款
  oklab 换算先例（注释注明核算来源）。

### D3 瞬态状态机 = App 显式状态 + 边沿判隐

App 增 `scrollbar_visible / scrollbar_activity: Option<Instant> /
scrollbar_hovered / scrollbar_drag: Option<DragState{grab_offset}>`。

- **触发** `mark_scroll_activity()`：`activity=now`，`visible =
  content_len > viewport`（overflow 才亮）+ 重绘。调用点：`ScrollPage` 臂、
  `handle_wheel`、drag 设 top、**hover 离开**（pi `setScrollbarActive(false)`
  → re-arm，离开后仍显示 1s）。
- **判隐**（200ms 超时支，单沿）：
  `if visible && !hovered && activity.elapsed() >= 1s { visible=false; redraw }`
  ——纯函数 `scrollbar_should_hide(...)` 供单测（不真睡）。隐延迟
  **1.0–1.2s**（pi 精确 1000ms，偏差 ≤ 一个 tick，接受并记录）。
- **不触发**：内容增长/流式 clamp（pi `updateLayout` 不 mark）、
  `AsyncKind::Messages` 重置。
- 纯函数 `visible = overflow && (hovered || activity<1s)` 只是渲染期兜底
  （防状态机与几何不同步），状态以 D3 显式字段为准。

### D4 事件转发（`event.rs`，源线程零状态）

`Moved → UiEvent::PointerMove{x,y}`、`Drag(Left) → PointerDrag{x,y}`、
`Up(Left) → PointerUp{x,y}`；`Down/Up(Right)`、`ScrollLeft/Right` 仍丢。
既有边界测试按滚轮先例**翻转改名**
（`..._for_add_tool_card_click_boundary` →
`pointer_events_forwarded_for_scrollbar_and_click_boundary`），不静默改变
前置断言。

### D5 分发顺序与消费语义（`mod.rs`）

```
PointerDown(既有 Click 臂) ── 列上(命中 geometry) ──> 消费: 不进 handle_click
│                                                  on_thumb → 记 grabOffset
│                                                  否则 → 立即 jump(指针→thumb中)
│                                                  起 drag + mark_activity
└── 非列上 → 既有 handle_click（卡片/thinking 不变）

PointerMove ── drag 中? ──> scrub: y→thumbOffset→set_top_abs + 重绘
           └─ 否则 hover 判定(列+轨道行) → 仅状态翻转时重绘
              (enter: hovered=true, visible=overflow揭示; leave: re-arm)
PointerUp  ── drag 中 → 结束 + mark_activity; 否则忽略
```

- `chat_area` Rect 由 draw 存上帧（同 `card_hits` 模式）。
- **浮层打开时三类 pointer 全忽略**（与 `handle_click` 同规则：浮层遮挡不
  透传下层交互）。
- `ChatScroll::set_top_abs(top)`（`scroll.rs` 新纯函数）：clamp 到
  `[0, max_top]` + `following = (top == max_top)`——jump 与 scrub 共用，
  复刻 pi `scrollTo` 的跟随恢复语义。

### D6 悬停揭示（pi `includeHiddenAuto` 对齐）

内容超视口时，指针移入滚动条列**即使当前未显示**也 `visible=true` 并
`hovered=true`（spec「悬停加粗并常显」的揭示语义）；内容不超视口时列无
geometry → 不命中、不消费（pi 无 target 同）。

## Risks / Trade-offs

- [Moved 洪水] → 源线程不过滤（O(1) 转发），主循环只在 hover/drag 状态
  **翻转**时重绘；仅 drag 中 Move 逐帧画（本来就跟手）。
- [末列 CJK 双格] → 截断补位（整字丢弃）+ PTY 用中文内容压末列验证。
- [15 处 `ChatScroll` 字面量] → 机械补字段，编译器逐点兜底。
- [末列按下与卡片/thinking 点击交叉] → 列上 Down 被消费（pi 语义），
  spec 场景「列上按下被消费」钉住；内容不超视口时不消费（行为与现状全同）。
- [判隐 1.0–1.2s vs pi 1000ms] → ≤一个 tick 偏差，记录接受；若实测体感差
  可把超时支收短（改 200ms 轮询值影响面大，不动）。

## Migration Plan

纯新增行为，无数据/协议迁移；随归档同步 tui-app（ADDED +1）。回滚 = 还原
本 change 提交，无状态遗留（全部为内存态）。

## Open Questions

（无——PTY 场景编排与几何向量在 tasks 阶段定。）
