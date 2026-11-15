# Tasks

## 1. 选区状态与纯函数（mod.rs）

- [x] 1.1 `SelectionState`（anchor/focus/granularity/initial_range/
      press_active/dragged/last_click/auto_scroll）入 `App`；纯函数
      `selection_bounds`（排序/同点为空）、显示列切分 `slice_line_cols`
      （unicode-width 逐字累加、宽字符整字不劈，design D1/D2）。
      验证：`cargo test`——单测 `selection_bounds_orders_and_empty_is_none`
      （跨行正反序、同点 None、列切分含宽字符边界）。
- [x] 1.2 词分段与连击判定：joiner 常量抄 pi
      `["/", "-", "+", "\\", "~", "_", "."]`、`word_range_at`（可选段 +
      joiner 贯通 = pi `canJoin` 同式）、`click_granularity`（500ms 窗、
      同行同词界 → count%3+1，design D5）。验证：单测
      `word_line_granularity_matches_pi`（普通词/路径 `/a/b.txt` 整体/
      三击行/超时重置/跨行不连击）。

## 2. 分发（mod.rs）

- [x] 2.1 `Click(Down)` 分流（design D4）：浮层早退、滚动条列既有消费、
      `hit_at` 命中 Tool/Thinking → 既有点击**并清选区**；MessageStart/
      无命中 → 起选（anchor、Character 粒度、press_active）。验证：单测
      `click_routing_priority_and_selection_start`（优先级矩阵：浮层/
      滚动条/卡片/thinking/背景 MessageStart/纯文本行 + 组件按下清既有
      选区）。
- [x] 2.2 `PointerMove` 扩展 focus（Character 逐点、Word/Line 按
      initial_range 延展，design D5）；`PointerUp`：未拖动 = 纯点击清选区
      不复制、拖动 → 非空交 4.2 复制；`AsyncKind::Messages` 清选区
      （坐标指向被替换 transcript，design D7）。验证：单测
      `drag_extends_by_granularity_and_release_rules`（三粒度扩展/纯点击
      清区/Messages 重建清区）。

## 3. 边缘自动滚动（mod.rs）

- [x] 3.1 Move 时边缘方向判定（`y ≤ chat_area.y` → -1、下缘 → +1）挂
      事件循环 200ms 超时支：每拍 `scroll_up/down_lines(1)` + focus 同步
      （内容移动 Δ → focus.row ±= Δ）、离缘/抬起停止（design D5；pi 50ms
      差异已记录）。验证：单测 `edge_autoscroll_ticks_and_moves_focus`
      （方向判定/每拍一行/focus 同步/停止条件，tick 注入不真睡）。

## 4. 提取与复制（mod.rs）

- [x] 4.1 `extract_selection_text`：按现状签名重算
      `chat_lines + wrap_for_viewport(height=content_len)` 全文 → 选区行
      切片 → `slice_line_cols` → `trim_end` → `join("\n")`；空 → None
      （design D2）。验证：单测
      `extract_selection_text_multirow_and_wide_chars`（跨行 join/行尾
      去空白/中文宽字符列不劈/纯样式剥离=结构性/空选区 None/增长后旧选区
      提取不变）。
- [x] 4.2 复制接线：PointerUp 非空 → `sinks.copy_text` 优先、
      `osc52_sequence` 兜底 → 绿色 `Copied!` notice；空选区静默；
      `/copy` 零改动（design D6）。验证：单测
      `release_copies_via_dual_path_and_notifies`（OSC52 序列含 base64
      选中文本/空不提示/copied notice 出现）。

## 5. 渲染（view.rs / mod.rs draw）

- [x] 5.1 draw 闭包内 `frame.buffer_mut()` 选区加 REVERSED 修饰（=[7m，行范围经
      `chat_area`+`scroll.top` 换算、越界裁剪、宽字符两格同换，design D3）。
      验证：单测 `selection_reverses_buffer_cells`（TestBackend：选中格
      fg/bg 互换、区外不动、bold 等属性保留）。

## 6. 文档

- [x] 6.1 `Docs/tui-usage.md`：「已知差异」的原生选中条目改写为已落地
      （应用自建选区：拖选/双击词/三击行/边缘自动滚/释放即复制）+ 键位
      说明；`Docs/roadmap.md` 挂账 C2（#9 残差选区复制）销账。
      验证：grep 两处均命中、"原生鼠标选择失效"条目已改写。

## 7. 验证

- [x] 7.1 PTY 手验（`/tmp/pty_select_copy_check.py`，假 pi 注入多行消息）：
      拖选跨行 → 反视频可见（pyte fg/bg 互换断言）+ 释放捕获到
      `\x1b]52;c;` OSC52 序列（base64 解码 == 选中文本）+ `Copied!`；
      双击选词（含路径 joiner 整体）、三击选行；纯点击无选区无复制；
      thinking/工具卡行按下走组件语义并清选区；滚动条列按下不开始选区；
      中文宽字符选区不劈字；`/new`（或会话切换）后选区清除。
      验证：脚本 `N/N steps passed`。
- [x] 7.2 全量门禁：`cargo test` 全绿、`cargo clippy --all-targets`
      零告警、`openspec validate add-select-to-copy --strict` 通过。
      验证：三条命令输出（test result ok / 0 warnings / Valid）。

## Workflow follow-up

- 归档：`/opsx-archive add-select-to-copy`（tui-app ADDED 同步：40→41 需求）
- 提交两批：`feat(tui)`（代码 + Docs）→ `docs(specs)`（归档 + 主规格），按需 push
