# Design

## Context

- `EditorBuffer::col()` 返回**字符序号**(编辑层语义,正确且不动)。
- `view.rs` 输入区渲染把该序号直接当格列:
  `cx = area.x + 1 + (cur_col - col_off)`;`col_off = (cur_col+1) - inner_w`
  也是字符数;可见行 `chars().skip(col_off).take(inner_w)` 同样按字符切。
  ASCII 下字符数==显示宽,行为正确;宽字符下光标/滚动整体左偏。
- `unicode-width` 已是依赖(`wrap.rs` 消息渲染在用,语义可对齐)。
- 现有 view 单测(`render_with_input` + TestBackend)可直接断言光标坐标与
  可见文本,无需新测试设施。

## Goals / Non-Goals

**Goals:**

- 宽字符行光标落在正确显示列;水平滚动按显示列;可见截取不切半宽字符。
- 纯 ASCII 行为逐位不变(换算在 ASCII 下退化为原式)。

**Non-Goals:**

- 不改 `EditorBuffer` 的字符序号语义与任何编辑操作。
- 不改消息区/wrap 渲染(已是宽度感知)。
- 不处理组合字符(combining)按 0 格的精细排版 — 保持与编辑层一致按 1 格计。

## Decisions

**D1: 光标行前缀显示宽求和。** `prefix_w = line.chars().take(cur_col)
.map(width.max(1)).sum()`,`cx = area.x + 1 + prefix_w - col_off`。
备选"改 EditorBuffer 存显示列" — 否决:编辑逻辑(插删/移动)按字符更简单
正确,渲染换算是唯一需要宽度的地方。

**D2: `col_off` 改显示列,截取经辅助函数 `slice_display(line, skip_w,
take_w) -> (片段, 实际起始列)`。** 跳过"整字结束列 ≤ skip_w"的字符,
跨偏移的宽字符整字保留(可见区最多左移 1 格);截取按累计显示宽 ≤ take_w,
不切半宽字符。返回实际起始列供 `cx` 消除跨偏移误差,保证光标与文本相对
位置严格一致。

**D3: 越界兜底保持原样。** `cx.min(area.x + area.width - 2)` 夹取保留,
`prefix_w.saturating_sub` 防下溢。

## Risks / Trade-offs

- [跨偏移保留宽字符 → 可见区比理论偏移左 1 格] → D2 返回实际起始列,
  光标相对位置仍精确;仅理论滚动位置差 1 格,无错位。
- [ASCII 回归] → 现有 view 单测全 ASCII,必须全绿作为等价性证据。
- [其他字符→格换算点遗漏] → 本 change 只覆盖输入区(view.rs);overlay/
  补全等如另有同类换算,留待发现后单独处理(不在本 change 范围)。

## Open Questions

- 无。
