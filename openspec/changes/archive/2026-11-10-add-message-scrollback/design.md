# Design

## Context

渲染现状（`src/app/view.rs`、`src/app/wrap.rs`）：`chat_lines` 全量逻辑行 →
`wrap_for_viewport` 贪心换行后 `drain(..len-height)` **丢弃头部** → Paragraph。
App/`ViewUi` 无任何滚动状态。`PageUp/PageDown` 在 `key_intent` 归入 `Edit`
臂，`edit_key` 无对应分支 → 静默忽略（死键）；但两条优先级链已天然就位：

- 浮层 `overlay_key` 对未知键一律 `OverlayOutcome::Handled` → 浮层打开时
  PageUp 不会穿透到聊天区；
- 补全菜单消费块在 `key_intent` **之前** → 菜单打开时 PageUp/Down = 菜单
  翻页（`completion_move(±5)`，既有测试 `completion_menu_opens_on_slash_and_navigates`
  断言）。

`draw` 签名为 `&self`，两个调用点（`mod.rs:320`、`mod.rs:464`）都在
`&mut self` 方法内。`ViewUi` `#[derive(Default)]`，有测试用
`..Default::default()` 构造。会话重建路径：`AsyncKind::Messages →
state.load_messages()` 整段替换 transcript（切换链的 get_messages 应答）。

动机见 proposal.md - Why；行为契约见 specs/tui-app「消息区滚动回看」。

## Goals / Non-Goals

**Goals:**

- 规格三场景全部落地：整页回看（页长 = 视口高 − 4）、上滚后新内容不抢位、
  骤变 clamp 到底恢复跟随。
- follow 三态语义与 pi `ScrollView(follow:"end")` 对齐（贴底跟随/上滚钉住/
  回底恢复）。
- 键位通路零新冲突：浮层、补全菜单优先级保持现状；embed 经 `inject_key`
  免费获得。

**Non-Goals:**

- 鼠标滚轮（需 `EnableMouseCapture` + 打破 event.rs"鼠标即丢"约定，roadmap 已挂账二期）。
- 滚动条可视化、alt-screen 搜索、prompt 跳转（OSC133）、半页/行级滚动键。
- 宽度变化后的重锚定（见 Risks：接受漂移，与 pi 一致）。
- 提交消息自动回底（见 D6：规格字面为不抢位，不回底）。

## Decisions

### D1 滚动状态：`ChatScroll { top, following, viewport }`（新模块 `src/app/scroll.rs`）

- `top: usize` — **换行后**绝对行号（当前视口首行）；`following: bool` —
  贴底跟随标志；`viewport: u16` — 上次渲染的消息区高度（页长计算用）。
- `Default` = `{ top: 0, following: true, viewport: 0 }`（新会话即贴底，
  `ViewUi` 的 derive(Default) 依赖此语义，测试构造自动获得现行为）。
- 纯方法：`page()`（`max(viewport − 4, 1)`，`PAGE_SCROLL_OVERLAP = 4` 对齐
  pi）、`page_up()`（`following = false; top = top.saturating_sub(page)`）、
  `page_down()`（`top = top.saturating_add(page)`，恢复跟随交给渲染端
  absorb）、`absorb(effective_top, at_bottom)`（回写 top、`following = at_bottom`）。

**为何是绝对行号 + following，而不是"距底偏移量"**：距底偏移在内容增长时
数值不变、绝对位置下移 → 视口随新内容滑动，正是规格禁止的"抢位"；绝对行号
在增长时不动即钉住，语义天然正确。pi 用 `scrollTop + followingEnd`，同构。

**为何 `viewport` 存进状态**：页长需要视口高，而视口高只有渲染布局后才知道
（输入框/菜单动态占高）；上一帧反馈即可，resize 后下一帧自纠，误差一帧、被
clamp 兜底。备选"从终端尺寸推算"被否：要复刻 `editor_height`/`completion_height`
布局公式，重复且易漂移。

### D2 渲染回写：`view::render` 返回吸收后的 `ChatScroll`，`draw` 改 `&mut self`

- `render(frame, state, ui) -> ChatScroll`：`render_chat` 内 wrap+窗口化得到
  `(window, effective_top, at_bottom)`，经 `absorb` 后返回。
- `draw(&mut self)`：闭包内 `terminal.draw(|f| fb.set(view::render(...)))`，
  闭包外 `self.scroll = fb.get()`（`Cell` 局部收集——ratatui 闭包返回 `()`，
  只能经可变捕获带出）。
- `ViewUi` 增加 `pub scroll: ChatScroll`（**按值 Copy**）：只读输入，保持
  `#[derive(Default)]` 与既有 `..Default::default()` 测试构造不破。

**备选与否决**：
- `ViewUi` 放 `&mut ChatScroll` — 破坏 `Default`（无 `&mut` 默认值），且
  `&mut` 进渲染闭包与 `&self.state` 并存徒增借用噪音。
- `App.scroll` 直接用 `Cell` 常驻 — `draw(&self)` 不用改，但把可变状态藏进
  interior mutability，`handle_key` 的读改写都要 `get/set`，可读性差。
- 改回 `&mut` 的代价核查过：调用点仅 2 处且均在 `&mut self` 方法内，编译期
  即可证。

### D3 窗口化收进 `wrap_for_viewport` 单入口

签名：`wrap_for_viewport(lines, width, height, scroll: &ChatScroll)
-> (Vec<Line<'static>>, ChatScroll)`。

1. 贪心换行 + 样式传播/补 pad（现状不动，先于窗口化）；
2. `effective_top = if following { len − height } else { min(top, len − height) }`
   （clamp 对 `len < height` 退化为 0）；
3. `drain(..effective_top)` + 截断到 `height`；
4. 返回窗口与 `absorb(effective_top, at_bottom)` 后的状态（含更新后的
   `viewport = height`）。

`following == true` 时行为与现状逐字节一致——**既有 `over_height_keeps_tail_not_head`
测试改签名后断言不变**，即回归网。

**备选与否决**：
- 拆成 `wrap_lines` + `window` 两个函数、调用方组装 — 多一次全量拷贝或
  生命周期拼接，收益只有"纯度"，窗口逻辑本身另有单测覆盖。
- 用 `Paragraph::scroll((top, 0))` 替代 drain — 我们的内容已预换行，语义等价，
  但 `top` 要窄化到 `u16`（>65535 行的长会话溢出），drain 无此上限；沿用现状。

### D4 键位：`key_intent` 新增 `ScrollPage(i8)`，`handle_key` 在菜单位置之后拦截

- `key_intent`：`PageUp | PageDown` 从 `Edit` 臂移出 →
  `KeyIntent::ScrollPage(-1 / +1)`（纯函数，映射测试可断言）。
- `handle_key`：菜单消费块（若开）已先行拦截 → `key_intent` 之后、
  `apply_intent` 之前拦截 `ScrollPage`，调 `self.scroll.page_up/page_down()`，
  与 `ForcePath` 同构（`mod.rs:1743` 模式）。
- 优先级链事实保证：浮层打开 → `overlay_key` 吞掉（不动聊天区）；菜单打开 →
  菜单翻页（回归测试已有）。**不需要新增任何优先级代码**，只需不破坏顺序。

**备选与否决**：在 `edit_key` 内处理 — `Edit` 臂语义是"编辑器键"，滚动是
视口操作，混入会让 `key_intent_mapping` 测试无法表达该键的意图归属。

### D5 骤变双层防护：显式重置（主）+ 渲染 clamp（兜底）

- **主路径**：`AsyncKind::Messages`（会话重建/切换链应答）→
  `self.scroll = ChatScroll::default()`。必要性：切换到**更长**会话时 clamp
  救不了（top=1000 < 新 len，不越界、不恢复跟随）→ 用户会钉在新会话中段，
  流式内容全程不可见。显式重置直接落规格场景 3 的"会话切换"。
- **兜底**：任何未预见的行数骤减（如压缩后重建）由 D3 的 clamp +
  `absorb(following = at_bottom)` 恢复——`at_bottom` 判定
  `effective_top + height >= len`，clamp 到底即恢复跟随。

### D6 提交消息不回底（规格字面）

规格要求"上滚离开底部后，新到达的内容 MUST NOT 抢占视口"，未给用户主动提交
开例外 → 用户上滚阅读时提交，视口保持钉住，自己刚发的消息从下方流过。pi
实际行为未核实；若日后要对齐"提交回底"，属行为变更，走规格 delta 而非实现
顺手改。

### D7 存储位置与 embed

`ChatScroll` 挂 `App` 字段、经 `ViewUi` 入渲染；`inject_key` → `handle_key`
同一通路，embed 宿主零改动获得滚动。

## Risks / Trade-offs

- **[宽度变化换行重排，`top` 指向漂移]** → 接受（pi 的 `scrollTop` 同样在
  重排后漂移）；不属规格承诺范围。若手验中观感明显，在 `tui-usage.md` 记一行。
- **[`viewport` 滞后一帧 → resize 后首键页长略偏]** → 误差被 D3 clamp 兜底，
  下一帧自纠；无需监听 resize 事件。
- **[`draw` 签名 `&self → &mut self`]** → 调用点仅 2 处、均在 `&mut self`
  方法内，编译期验证；`view::render` 返回值在测试调用点被忽略（非 `must_use`，
  无告警）。
- **[`wrap_for_viewport` 签名变更波及既有测试]** → `over_height_keeps_tail_not_head`
  改传 `ChatScroll::default()`（following），断言不变即回归网；新增 pinned
  窗口断言。
- **[每帧全量换行的既有开销]** → 本 change 不加剧（现状已全量 wrap 后丢头）；
  若长会话未来卡顿，增量 wrap 另立变更，不在本处预优化。

## Migration Plan

无数据/协议影响，纯 UI 状态。回滚 = revert 实现提交（规格 delta 随归档流程
同步，回滚主规格需另行归档变更）。按 tasks 分组可逐组落地，每组独立可测。

## Open Questions

（无——提交回底问题已由规格字面解决并记录于 D6；pi 对齐若需要，走未来规格变更。）
