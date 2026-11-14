# Tasks

## 1. 状态载体与几何纯函数（scroll.rs / wrap.rs）

- [x] 1.1 `ChatScroll` 增 `content_len: usize`（Default=0；wrap 回写 drain 前
      总行数；15 处字面量机械补字段）；`ChatScroll::set_top_abs(top)` 纯函数
      （clamp + `following = (top == max_top)`，jump/scrub 共用，design D1/D5）。
      验证：`cargo test scroll:: wrap::` 全绿——新单测
      `content_len_updated_each_wrap`、`set_top_abs_clamps_and_restores_follow`
      （中段钉住/越界 clamp/贴底恢复跟随三态）。
- [x] 1.2 `wrap.rs`：`scrollbar_geometry(content_len, viewport, top) -> Option<ScrollbarGeometry>`
      直接移植 pi 公式（`thumb_h = max(min(2,h), round(h*h/content))`、
      比例 thumbTop、None=内容不超视口）+ track/thumb 双色 okhsl→RGB 常量
      （走 tool_panel_bg 同款换算，注释注明 dark.json 来源，design D2）。
      验证：单测几何向量——不足一屏 None、最小 thumb=2、贴底 thumb 在底端、
      中段位置按比例、宽/窄视口边界。

## 2. 绘制（wrap.rs / view.rs）

- [x] 2.1 `wrap_for_viewport` 增 `scrollbar: Option<ScrollbarOverlay{hovered: bool}>`
      参数并在窗口化+bg 补齐后绘制末列（截断显示宽至 width-1 再补
      「空格×n + 字符」span——CJK 双格尾字整字丢弃；fg=token 色、清 bg；
      字形 `│` / `┃` / hovered `█`，design D2）。验证：`cargo test wrap::`
      ——新单测 `scrollbar_paints_last_column_with_glyphs_and_clears_bg`
      （常态/悬停字形、None 不画、工具卡 bg 行末格让位、中文尾字整字丢弃）；
      既有调用点（view + wrap 测试）随签名适配。
- [x] 2.2 `view.rs`：`ViewUi` 增 `scrollbar: Option<ScrollbarOverlay>`
      （derive(Default) 不破）接 `render_chat`→wrap；mod draw 暂传 `None`
      （4.1 接状态）。验证：`cargo test view::`——新单测
      `render_paints_scrollbar_when_overlay_present`（Some → 末列 │、None →
      无）。

## 3. 事件转发（event.rs）

- [x] 3.1 `Moved → UiEvent::PointerMove{x,y}`、`Drag(Left) → PointerDrag{x,y}`、
      `Up(Left) → PointerUp{x,y}`；`Down/Up(Right)`、`ScrollLeft/Right` 仍丢
      （design D4）。验证：单测——既有边界测试按滚轮先例翻转改名
      `pointer_events_forwarded_for_scrollbar_and_click_boundary`
      （三类转发断言 + 仍丢清单），`cargo test event` 全绿。

## 4. 状态机与分发（mod.rs）

- [x] 4.1 瞬态状态（`scrollbar_visible/activity/hovered/drag`）+ `mark_scroll_activity()`
      （`ScrollPage` 臂与 `handle_wheel` 调用；内容增长与 Messages 重置不触发）
      + 200ms 超时支单沿判隐（`scrollbar_should_hide` 纯函数：`visible &&
      !hovered && activity≥1s`）+ draw 按 `visible` 传 `Some(ScrollbarOverlay
      {hovered})`（design D3）。验证：单测——`scrollbar_shows_on_scroll_and
      _hides_after_1s`（状态迁移，Instant 注入不真睡）、hover 压制判隐、
      增长不触发、`scrollbar_should_hide` 真值表。
- [x] 4.2 pointer 分发：draw 存上帧 `chat_area`；`PointerMove` hover 判定
      （geometry 命中列+轨道行 → 仅翻转时重绘；enter 揭示 `visible=overflow`、
      leave re-arm 计时）；`Click` 列上**消费不进 handle_click**（on_thumb 记
      grabOffset、否则立即 jump + 起 drag）；`Drag` scrub（y→thumbOffset→
      `set_top_abs`）；`PointerUp` 结束 drag；浮层打开三类全忽略（design D5/D6）。
      验证：单测——`scrollbar_column_down_consumes_and_jumps`（列上按下不触发
      卡片/文本点击、点轨道跳转、scrub 比例映射、浮层忽略、非列上 Down 卡片
      点击回归）。

## 5. 文档

- [x] 5.1 `Docs/tui-usage.md` 补滚动条条目（瞬态 1s/悬停/拖拽、列上按下不
      穿透）；`Docs/roadmap.md` 挂账项更新（滚动条可视化/悬停/拖拽 → 已落地，
      剩余 alt-screen 搜索等）。验证：grep 两处均命中。

## 6. 验证

- [x] 6.1 PTY 手验（`/tmp/pty_scrollbar_check.py`）：内容超视口 PageUp → 条
      出现 → ≥1.5s 后隐；顶部/贴底 thumb 位置（比例）；hover（SGR Moved 注入）
      → `█` 常显、移出后计时；列上 Down+Drag → 跳转/拖动跟手；中文内容压
      末列无半字。验证：脚本 `N/N steps passed`（屏幕断言含 `│`/`┃`/`█`）。
- [x] 6.2 全量门禁：`cargo test` 全绿、`cargo clippy --all-targets` 零告警、
      `openspec validate add-message-scrollbar --strict` 通过。验证：三条命令
      输出（test result ok / 0 warnings / Valid）。

## Workflow follow-up

- 归档：`/opsx-archive add-message-scrollbar`（tui-app ADDED 同步：38→39 需求）
- 提交两批：`feat(tui)`（代码 + Docs）→ `docs(specs)`（归档 + 主规格），按需 push
