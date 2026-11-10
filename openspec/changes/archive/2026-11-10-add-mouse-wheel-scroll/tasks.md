# Tasks

## 1. 滚动状态层（行粒度方法）

- [x] 1.1 `scroll.rs` 增 `scroll_up_lines(n)` / `scroll_down_lines(n)`
      （`page_up`/`page_down` 同构换参：上滚 = following=false + 饱和减；
      下滚 = 饱和加、不改 following，回底恢复交给渲染端 absorb）；
      验证：单测覆盖"上滚打断跟随""下滚不改 following""饱和不越界"
      三类断言，`cargo test scroll::` 全绿

## 2. 加速器与事件翻译

- [x] 2.1 移植 `WheelAccel`（仅 auto 模式；常量 BURST_GAP_MS=5 /
      GESTURE_GAP_MS=200 / REFERENCE_GAP_MS=100 / MAX_AUTO_LINES=6 +
      分数进位；`terminal_accelerates_wheel` = darwin 且 SSH_* 未设
      判定为纯函数）；验证：pi `wheel-scroll.test.ts` auto 用例表逐条转
      Rust 断言（150ms→1、50ms→2、20ms→5、10ms→6、<5ms 同 notch→1、
      换向/停顿重置为 1、固定输入确定性）+ env 判定单测，
      `cargo test wheel` 全绿
- [x] 2.2 `event.rs`：新增 `UiEvent::Wheel(i16)`（负=上滚、正=下滚，
      行数已含 Alt×5 与加速结果）；`ui_event_from_crossterm` 持有
      `WheelAccel` 状态（spawn 循环局部），`ScrollUp/ScrollDown` →
      加速算行数 → `Some(Wheel)`；`ScrollLeft/Right`、`Moved`、`Drag`、
      `Down`、`Up` → `None`；验证：改写 `mouse_event_is_ignored` 为
      「滚轮翻译通过」+「点击/Moved 仍丢弃（测试名钉住
      add-tool-card-click 边界）」两断言，另有 alt×5 行数断言，
      `cargo test event` 全绿

## 3. 捕获生命周期（terminal.rs 四插入点）

- [x] 3.1 四点成对落地：`take_terminal()`（`ratatui::init()` 后
      Enable）、`restore_now()`（`ratatui::restore()` 后 Disable——
      收口 finish/Drop/panic 三路）、`suspend_for_child()`
      （`LeaveAlternateScreen` 前 Disable）、`resume_from_child()`
      （`EnterAlternateScreen` 后 Enable）；`restore_early()` 与
      `enter_embedded()` 不动；验证：`cargo build` 通过，
      `grep -c` 确认 Enable/Disable 各 2 处且均紧邻 alt-screen 切换
      （PTY 序列断言在 5.3）

## 4. 分发与路由

- [x] 4.1 `mod.rs` 事件 batch 新增 `UiEvent::Wheel(lines)` 臂：
      浮层打开 → 忽略（不改 scroll、不置 need_draw）；负 →
      `scroll_up_lines(abs)`、正 → `scroll_down_lines`；`need_draw=true`
      + `log_event` 记行数；验证：单测①正常滚动行数正确
      ②浮层打开时滚动不改 `self.scroll` ③补全菜单打开时仍滚消息区，
      `cargo test` 对应用例全绿

## 5. 文档与集成验证

- [x] 5.1 `Docs/tui-usage.md`：操作说明补滚轮（行粒度、快速连滚加速
      上限 6 行/事件、Alt 5 倍、浮层忽略），已知差异补"开启鼠标捕获后
      原生鼠标选择需 Shift+拖拽透传（select-to-copy 属挂账）"；
      验证：文档含滚轮条目与 Shift 选择差异字样
- [x] 5.2 `Docs/roadmap.md`：滚动增强挂账条目更新——滚轮落地，剩余
      **滚动条（暂不做）**/搜索改列；验证：roadmap 不再列滚轮为待办、
      滚动条仍明确挂在账上
- [x] 5.3 PTY 手工验证（注入 SGR 序列 + pyte 断言，参考
      `Docs/tui-usage.md`）：①启动输出含 `?1006h`、退出输出含
      `?1006l`（捕获对称）②上滚 `\x1b[<64;x;yM` ×N 视口上移且输入区在
      ③下滚 `<65` 回底后流式新内容恢复贴底跟随 ④浮层打开时滚轮视口不动
      ⑤慢速单发 alt+滚轮 `<72` 上移行数为普通滚轮的 5 倍（本机 darwin
      本地 = 1 行/事件，对比 1 vs 5）；验证：五项断言全过
- [x] 5.4 全量门禁：`cargo test` 全绿 + `cargo clippy --all-targets`
      零告警 + `openspec validate add-mouse-wheel-scroll --strict` 通过；
      验证：三命令输出即证据

## Workflow follow-up

- 实现完成后走 `/opsx-archive` 归档，随归档同步主规格
  （`openspec/specs/tui-app` 新增「鼠标滚轮滚动」需求）。
- 后继 `add-tool-card-click`（捕获已开，其 change 不再碰 terminal.rs）。
