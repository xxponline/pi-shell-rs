# Design

## Context

现状三层（探索已核实）：`terminal.rs` 无 `EnableMouseCapture`（alt-screen 由
`take_terminal()` 内 `ratatui::init()` 进入）→ `event.rs::ui_event_from_crossterm`
对 `Event::Mouse` 返回 `None` → 测试 `mouse_event_is_ignored` 把"鼠标即丢"
钉成约定。crossterm 0.29 的 `EnableMouseCapture` = `?1000h ?1002h ?1003h
?1006h`（含 1003 任意移动 → Moved 洪水），`DisableMouseCapture` 对称发 l 序列。

状态层已备好：`ChatScroll{top,following,viewport}` + `absorb`（渲染端 clamp +
贴底恢复跟随）+ `page_up/page_down`；事件循环已是 **batch 模式**（收一批、
处理完单次重绘）。pi 参考实现：`wheel-scroll.ts`（`WheelScrollAccelerator`，
带测试）+ `tui-alt-screen.ts`（SGR 解析、`routeWheel`、
`ALT_WHEEL_SCROLL_MULTIPLIER = 5`）。

动机与范围见 proposal.md；行为契约见 specs/tui-app「鼠标滚轮滚动」。

## Goals / Non-Goals

**Goals:**

- 规格四场景落地（行粒度+速度放大、Alt 5 倍、回底恢复跟随、浮层忽略）。
- 捕获生命周期**一次做对**：`add-tool-card-click` 复用后不再碰 terminal.rs。
- 加速器为纯逻辑，用 pi 测试向量对拍，无 TTY 可测。

**Non-Goals:**

- **滚动条（可视化/悬停）——暂不做**（roadmap 挂账保留，proposal 已明写）。
- 卡片点击（`add-tool-card-click`；本期 `Down`/`Up` 仍丢弃）。
- pi 式坐标命中路由与 overscroll 链（单滚视图无增益）。
- `fullscreenWheelScrollLines` 设置键（首期硬编码 `"auto"`）。
- 水平滚轮（`ScrollLeft/Right` 丢弃）、embed 宿主滚轮注入、select-to-copy。

## Decisions

### D1 捕获生命周期：4 个插入点收口 5 个场景

| 插入点 | 动作 | 覆盖场景 |
|---|---|---|
| `take_terminal()`（`ratatui::init()` 后） | `EnableMouseCapture` | 正常进入 |
| `restore_now()`（`ratatui::restore()` 后） | `DisableMouseCapture` | finish / Drop / **panic hook**（三路共用此函数） |
| `suspend_for_child()`（`LeaveAlternateScreen` 前） | `DisableMouseCapture` | 外部编辑器子进程拿干净终端 |
| `resume_from_child()`（`EnterAlternateScreen` 后） | `EnableMouseCapture` | 子进程结束恢复 |

`restore_early()`（未进 alt-screen、未开捕获）与 `enter_embedded()`（无 tty）
**不动**。备选"包一个 `MouseCaptureGuard` RAII"被否：现有 KB_PUSHED 静态标志
已是同款手工对称模式，再引一层 guard 与 `ratatui::init/restore` 的所有权
关系纠缠，收益只有形式美。

### D2 加速器与行数计算放在**事件源线程**，`UiEvent::Wheel(i16)` 携带最终行数

- 签名：有符号行数——负 = 上滚（更早内容），正 = 下滚（更新内容）；行数已含
  Alt×5 与加速结果。
- **为何不在主循环算**：事件 batch 在同一次迭代内处理，`Instant::now()`
  间隔 ≈ 0 → 全部判为 burst（<5ms）→ 速度信息塌缩、加速完全失效。读取点
  的时间戳才反映真实滚轮节奏（pi 同构：SGR 解析时就算好）。
- `ui_event_from_crossterm` 从纯函数变为持有 `WheelAccel` 状态（spawn 循环
  局部），Key/Resize 路径不变；`WheelAccel` 本身仍是纯逻辑、直接单测。
- 备选"`Wheel{dir, at_ms}` 进主循环再算"被否：要把时间戳塞进 UiEvent、
  App 状态多一块，换来的只是"状态都在 App"的形式整齐——而测试照样直测
  `WheelAccel`。

### D3 行粒度滚动：`scroll_up_lines(n)` / `scroll_down_lines(n)`

`page_up`/`page_down` 同构换参：上滚 = `following=false` + 饱和减；下滚 =
饱和加（不改 following）。**absorb 兜底全部边界，状态层零新语义**：滚到底
→ 渲染端 clamp → `at_bottom` → 恢复跟随；内容不足一屏 / 越界 → 自愈。

### D4 分发与路由（mod.rs batch match 臂）

```
UiEvent::Wheel(lines) →
    overlay.is_some()  → 忽略（不置 need_draw：浮层遮住消息区，滚动无意义）
    lines < 0          → scroll_up_lines(lines.abs())
    lines > 0          → scroll_down_lines(lines)
    → need_draw = true（batch 末尾单次重绘，连发天然合并）
```

补全菜单打开时滚消息区（菜单非浮层、非滚视图——proposal 已定，无需命中
测试）。`log_event("key", …)` 记录实际行数便于 PTY/日志排查。

### D5 加速器移植：仅 auto 模式，常量与测试向量照抄 pi

- `WheelAccel { last_ms, last_dir, avg_gap: Option<f64>, carry: f64 }`，
  `next(dir, now_ms) -> u16`；常量 `BURST_GAP_MS=5`、`GESTURE_GAP_MS=200`、
  `REFERENCE_GAP_MS=100`、`MAX_AUTO_LINES=6` 逐一对应。
- `terminal_accelerates_wheel()`：`cfg!(target_os="darwin") && SSH_CONNECTION/
  SSH_CLIENT/SSH_TTY 均未设` → 不加速（OS 已给 1 行/事件，防双加速），否则
  按速度加速。纯函数注入 env 判定，可测。
- **固定行数模式不移植**（settings 键不做，无消费者）——`lines` 参数与
  `setLines` 整个砍掉，状态少一半。
- 对拍基准：`ref/pi/packages/tui/test/wheel-scroll.test.ts` 的 auto 用例
  表（150ms→1行、50ms→2行、20ms→5行、10ms→6行、<5ms burst→1行、
  换向/停顿重置为 1）逐条转成 Rust 断言。
- Alt×5 在翻译层（`MouseEvent.modifiers` 含 ALT 时 `lines × 5`），不进
  `WheelAccel`（pi 同：accel 出行数后再乘）。

### D6 捕获标志：用 crossterm `EnableMouseCapture`，不手写精简 CSI

1003（任意移动）带来的 Moved 洪水在 `ui_event_from_crossterm` 丢弃——成本
O(1)、发生在源线程，主循环不醒、不重绘。手写 `?1000h ?1006h`（仅点击+滚轮）
可免洪水，但放弃 crossterm API 对称、且 `Down`/`Up` 事件（`add-tool-card-click`
需要）依赖同一套解析路径——1000 本就含点击，洪水只是 Moved，不值得自造 CSI。

### D7 事件翻译边界（测试约定翻转）

`ScrollUp`→`Wheel(负)`、`ScrollDown`→`Wheel(正)`；`ScrollLeft/Right`、
`Moved`、`Drag`、`Down`、`Up` → `None`。`mouse_event_is_ignored` 改写为
"滚轮翻译通过 + 其余仍丢"两断言——**明确本期不转发点击**，与
`add-tool-card-click` 的边界写进测试名。

## Risks / Trade-offs

- **[漏 Disable 点 → 退出后终端仍在报鼠标]** → `restore_now` 收口三路
  （finish/Drop/panic）；suspend/resume 单列成对；组 5 PTY 断言退出序列含
  `?1006l`、启动序列含 `?1006h`。
- **[子进程拿到鼠标模式]** → suspend/resume 成对；PTY 难复现外部编辑器全流程
  → 代码对称 + review 清单兜底（记录于 tasks 该组验证）。
- **[batch 时间戳塌缩 → 加速失效]** → 加速器放源线程（D2），并在
  `WheelAccel` 单测中显式覆盖"同毫秒多事件 = burst"用例。
- **[SSH 下 1003 Moved 流量]** → 接受（D6 取舍）；仅源线程 O(1) 丢弃。
- **[原生鼠标选择失效]** → 本 feature 固有代价（捕获即抢走选择）；
  `tui-usage.md` 已知差异记"Shift+拖拽透传"，select-to-copy 属挂账。
- **[与 add-tool-card-click 的事件边界]** → 本期 `Down`/`Up` 明确丢弃
  （D7 + 测试名钉住），其 change 再开转发。

## Migration Plan

无数据/协议影响，纯输入通道扩展。回滚 = revert 实现提交（spec delta 随归档
流程同步）。按 tasks 分组逐组落地，每组独立可测；PTY 验证集中在文档组。

## Open Questions

（无——菜单打开时滚消息区、embed 不注入滚轮均已在 proposal/Non-Goals 定死。）
