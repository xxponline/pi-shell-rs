# Proposal

## Why

消息区滚动回看（`add-message-scrollback`，已归档）只通键盘 PageUp/PageDown，
鼠标滚轮/触摸板完全不可用——事件在三层被丢：终端未开捕获（`terminal.rs` 无
`EnableMouseCapture`）→ `event.rs` 对 `Event::Mouse` 返回 `None` → 测试
`mouse_event_is_ignored` 把"鼠标即丢"钉成约定。roadmap 挂账"滚轮/滚动条/
搜索等滚动增强"首项即此。

pi 侧有成熟且带测试的实现（`packages/tui/src/wheel-scroll.ts` 加速器 +
`tui-alt-screen.ts` 的 `routeWheel`），模型可直接移植。更关键的是：本 change
是 **`EnableMouseCapture` 的首次开启**——捕获生命周期（enter / restore /
panic / suspend / resume 五点）是共享地基成本，后续 `add-tool-card-click`
在其上只剩纯逻辑层，不再碰终端。

## What Changes

- **捕获生命周期**（`terminal.rs`）：`EnableMouseCapture` 随 alt-screen 进入；
  `DisableMouseCapture` 对称出现在 `restore_now` / panic hook /
  `suspend_for_child`（外部编辑器子进程必须拿到干净终端）/ resume——漏任一
  点即"退出后终端仍在报鼠标"的坏体验。
- **事件翻译**（`event.rs`）：`Event::Mouse` → `UiEvent::Wheel(dir)`
  （仅 `ScrollUp`/`ScrollDown`）；`Moved`/`Drag`/`Down`/`Up` 首期仍丢弃——
  丢弃发生在事件源线程，crossterm 捕获标志含 1003（任意移动）的 Moved 洪水
  不进主循环、不触发重绘。翻转 `mouse_event_is_ignored` 既有测试约定。
- **行粒度滚动**（`scroll.rs`）：`scroll_up_lines(n)` / `scroll_down_lines(n)`
  ——`page_up`/`page_down` 同构换参；`absorb` 现有机制天然覆盖"滚到底恢复
  跟随、内容不足一屏自愈"，状态层零新语义。
- **滚动加速器**：移植 pi `WheelScrollAccelerator` 为纯 Rust（无 TTY，可单测）：
  darwin 本地 1 行/事件（OS 已加速，防双加速）、其余按事件间隔加速
  （100ms=1行 … 20ms=5行，上限 6 行；<5ms 视为同一 notch 不加速；200ms
  手势断开；分数进位）、alt+wheel 倍率。首期硬编码 `"auto"`，不加设置键。
- **路由**：浮层打开时忽略滚轮（不滚被遮住的消息区）；其余位置一律滚消息区。
  单滚视图，**不做** pi 式坐标命中测试与 overscroll 链。
- **Spec**：`tui-app` ADDED「鼠标滚轮滚动」——不动「消息区滚动回看」的
  PageUp/PageDown 文本（键盘需求不变，滚轮是增量行为）。
- **文档**：`tui-usage.md` 补滚轮用法与已知差异（开捕获后原生鼠标选择需
  Shift 透传——本 feature 固有 UX 代价，pi 以自家 select-to-copy 解决、属
  挂账）；`roadmap.md` 挂账措辞更新（滚轮落地，剩余滚动条/搜索改列）。

**移出本 change / 明确暂不做：**
- **滚动条（可视化、悬停反馈）——暂不做**，roadmap 挂账保留；
- 卡片点击（独立 change：`add-tool-card-click`）；
- 坐标命中路由、多视图 overscroll 链（单滚视图无增益）；
- `fullscreenWheelScrollLines` 设置键（首期硬编码，后续小活）；
- scroll-to-end 指示器、select-to-copy、alt-screen 搜索、prompt 跳转。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `tui-app`：新增 ADDED 需求「鼠标滚轮滚动」——钉住可观察行为：滚轮按行
  粒度滚动消息区（加速语义）、滚到底恢复贴底跟随、浮层打开时忽略。

## Impact

- `src/app/terminal.rs`：捕获生命周期 5 点（enter/restore/panic/suspend/resume）
- `src/app/event.rs`：Mouse→Wheel 翻译 + `mouse_event_is_ignored` 测试翻转
- `src/app/scroll.rs`：行粒度滚动方法 + 加速器纯逻辑（新模块或并入）
- `src/app/mod.rs`：`UiEvent::Wheel` 分发（事件 batch 已天然合并连发、单次重绘）
- `Docs/tui-usage.md`、`Docs/roadmap.md`、`openspec` delta
- 测试：加速器单测（间隔→行数表）、滚动语义单测、PTY 注入 SGR
  （`\x1b[<64;x;yM` 上滚 / `<65` 下滚）屏幕断言
- **前置**：无（本 change 即捕获地基）；后继 `add-tool-card-click` 依赖它
