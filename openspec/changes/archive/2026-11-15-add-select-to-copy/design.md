# Design

## Context

动机与 pi 全量实测见 proposal.md。决定方案形态的既有资产与约束：

- `UiEvent::PointerMove/PointerDrag/PointerUp` 已转发（`add-message-scrollbar`
  铺就），`event.rs` **零改动**。
- 消费顺序已与 pi 逐层一致：浮层 → 滚动条列 → 组件 `handle_click`
  → 背景（现为 no-op——选区将占据这个最后兜底位）。
- 复制双通路现成：`sinks.copy_text`（宿主注入、验证成功）优先 +
  `osc52_sequence` 兜底；`notice` 即 pi 的 flash 通道。
- 纯函数管线现成：`chat_lines` + `wrap_for_viewport` 可按现状签名重算
  全文行（提取用）；`chat_area` 上帧 Rect 供命中判定。
- pi 关键事实：渲染 = paint 后 ANSI `[7m` 反视频（样式保留）；词连接符
  常量 `["/", "-", "+", "\\", "~", "_", "."]`；连击窗口 500ms；
  `copyOnSelect` 默认 true；清除 = 组件按下 + transcript 替换（#9311）；
  边缘自动滚动 50ms。

## Goals / Non-Goals

**Goals:**

- pi 选区全量镜像：字符/词/行粒度、反视频、边缘自动滚动、释放即复制。
- 与既有点击/滚动条/浮层语义零冲突（选区只做最后兜底）。
- 提取与渲染零新依赖、`event.rs`/`chat.rs`/`wrap.rs` 签名零改动。

**Non-Goals:**

- URL 点击打开（需 `pressedUrl` 悬停基建，链接交互另议）；
  `copyOnSelect`/键位配置化（无 settings，硬编码 pi 默认 true）；
  键盘选区、编辑器内选区、普通模式终端委托（恒 alt-screen，N/A）；
  选区内容的语法着色保留（反视频天然保留——非目标即达成，不另测主题）。

## Decisions

### D1 状态与坐标空间 = 绝对屏行 + 显示列（App 字段组）

`selection: Option<SelectionState>`，内含 `anchor/focus: Option<(usize, u16)>`
（绝对屏行、显示列）、`granularity: Character|Word|Line`、
`initial_range: Option<(usize, u16, usize, u16)>`（词/行初选锁定，拖拽延展
用，pi `selectionInitialRange` 同构）、`press_active/dragged`、
`last_click: Option<(Instant, usize, u16, u16)>`（500ms 连击窗）、
`auto_scroll: Option<i8>`（边缘方向）。坐标与 `card_hits` 同空间 →
行判定复用 `chat_area` + `scroll.top` 换算。

- **替代：存每帧全文行供提取**：否——会话大时内存翻倍；见 D2。

### D2 文本提取 = 复制时重算纯函数管线

释放复制时：`chat_lines(state, flags, overrides, …)` →
`wrap_for_viewport(…, height=content_len)` 取**全文窗口** → 按选区绝对行
切 `[start_row..=end_row]` → 每行按显示列切片（unicode-width 逐字累加，
宽字符整字不劈）→ `trim_end` → `join("\n")`。空串 = 无选区（不复制）。

- **行号稳定性**：transcript append-only → 选区建立后内容增长只在底部
  追加，旧行号与文本不变（与命中表同一论证）；会话重建直接清选区（D6）。
- **样式天然剥离**：提取走 `Line` 的文本内容，ANSI/样式不进入结果 ✓
  （规格"不含 ANSI 序列"由此结构性满足）。

### D3 渲染 = draw 闭包内加 REVERSED 修饰（=[7m）

`terminal.draw` 闭包中 `view::render` 之后、对 `frame.buffer_mut()` 按
选区行/列范围逐 cell 加 `Modifier::REVERSED`（宽字符两格同换）——ratatui
会为该属性输出 ANSI `7m`，**属性级反转**即 pi paint 后 `[7m` 的真义：
bold/italic/颜色值全部原样保留，且 **default 色文本同样可见**（实施修正：
原案“交换 fg/bg 值”对 default/default 无效——普通 markdown 文本正是
default 色，值交换后不可见，apply 期由 PTY 用例设计阶段发现）。选区行
范围经 `chat_area` 与 `scroll.top` 换算，边界按宽字符吸附（起点回退、
终点扩格，与提取同一语义），越界裁剪。

- 已知视觉副作用（pi 同款，记录不规避）：工具卡面板色行被选中时
  面板 bg 与文字 fg 互换——与 pi 屏幕级反转行为一致。

### D4 事件分发 = 背景兜底位（Down 起选，与既有组件按下并存）

```
Click(Down): 浮层早退 → 滚动条列（既有消费）→ hit_at 命中 Tool/Thinking
             → 既有 handle_click + 清选区 → MessageStart/无命中 → 起选
             （anchor=点、granularity=Character、press_active=true）
PointerMove: drag 中 → focus 更新 + 边缘自动滚动判定（D5）；
             非 drag → 忽略（无悬停选区语义）
PointerUp:   press_active → 定形；未拖动 = 纯点击 → 清选区（不复制）；
             拖动过 → 非空则提取+复制+Copied!（D2/D6）
```

- MessageStart 行属背景（其点击语义本就是 no-op）→ 可起选，与 pi 一致
  （组件只有 thinking/工具卡两类）。
- `handle_click` 返回值不再区分"组件动作"与"背景"——分发改为先查
  `hit_at` 的 **Tool/Thinking** 分支再落选区（MessageStart 走选区）。

### D5 粒度与边缘自动滚动

- 连击判定抄 pi `getClickCount`：500ms、同行同列同词界 → `count%3+1`；
  Word 分段 = 空白/标点切词 + **joiner 常量抄 pi**
  `["/", "-", "+", "\\", "~", "_", "."]`（joiner 段本身 selectable，
  左右贯通条件：两侧 selectable 且至少一侧为 joiner——pi `canJoin` 同式）。
- Word/Line 粒度下 `update_selection_focus`：焦点在初选范围内 → 只动
  focus 端；越出 → anchor 与 focus 互换扩展（pi `initialRange` 语义，
  `boundary` 端点同义）。
- 边缘自动滚动：Move 时指针 `y ≤ chat_area.y` → -1、`≥ y+height` → +1，
  挂**事件循环 200ms 超时支**（既有 tick）每拍 `scroll_up/down_lines(1)`
  并同步 focus（内容移动 Δ行 → focus.row ±= Δ）；离缘/抬起停止。
  **与 pi 的 50ms 差异记录接受**（更慢的自动滚，体感可接受；改 200ms
  轮询值影响面大，不动）。

### D6 复制与提示

`copyOnSelect` 恒 `true`（pi 默认）→ PointerUp 且选区非空 →
提取（D2）→ 非空 → `sinks.copy_text` 或 `osc52_sequence`（既有双通路，
宿主注入优先的 pi 同款语义）→ `notice("Copied!", 绿)`。空文本 →
静默。**`/copy` 零改动**：pi 的 `preferSelection` 分支仅在
`copyOnSelect=false` 时启用，我们恒 true → 释放时已复制，`/copy`
保持"最后一条 assistant 文本"语义。

### D7 清除时机

- 组件行（Tool/Thinking）按下 → 清（pi :965 同位）。
- `AsyncKind::Messages`（`load_messages` 替换 transcript）→ 清
  （pi interactive-mode:4259 #9311 同理——坐标指向被替换内容）。
- 纯点击（未拖动的释放）→ 清；浮层打开期间不产生新选区（浮层在
  分发最前，既有早退）。
- **不新增**滚动清除（pi 未清——滚动后选区行号仍指原文档行，提取
  按绝对行，语义自洽）。

### D8 与滚动条列的边界

滚动条列（`chat_area` 最右列）按下被 `handle_scrollbar_press` 先消费
→ 永不进入起选 ✓；滚动条 hover/drag 期间的 `PointerMove` 也先走滚动条
分支（既有顺序）→ 选区拖拽不会与 thumb 拖拽打架（同一时间只有一种
手势：按下时已定归属）。

## Risks / Trade-offs

- [选区行号依赖 append-only] → 会话重建清选区（D7）+ 增长只在底部
  （既有论证）；单测钉"增长后旧选区提取文本不变"。
- [宽字符列计算] → 提取与高亮共用同一列换算（unicode-width 逐字累加）；
  PTY 用中文内容压选区验证不劈字。
- [面板色行反视频的视觉怪异] → pi 同款行为，记录为已知副作用（D3）。
- [200ms 自动滚 vs pi 50ms] → 记录接受（D5）；若体感差再议。
- [Down 起选改变背景点击的既有 no-op] → no-op 无可观测行为变化，
  起选是纯新增；组件/滚动条路径逐条回归（单测 + PTY）。

## Migration Plan

纯新增行为，无数据/协议迁移；随归档同步 tui-app（ADDED +1）。回滚 =
还原本 change 提交（选区为内存态，无遗留）。

## Open Questions

（无——连击词分段的边界样例与 PTY 编排在 tasks 阶段对拍/定夺。
