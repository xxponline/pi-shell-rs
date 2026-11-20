# Design

## Context

定向量测证据（临时诊断 `diag_stream_tail_frame_cost`，已随 change 转正）：

```
背景 1500 条/5.8MB (bench 同款合成): 冷建 595ms | 暖帧 1.09ms | draw 2.68ms
流式尾条 (互异代码块, ~200B/delta, 每 delta 一帧):
  tail 5KB  → refresh 2.4ms  draw 3.2ms  frame 5.7ms
  tail 10KB → refresh 3.9ms  draw 4.2ms  frame 8.1ms
  tail 15KB → refresh 37.8ms draw 5.4ms  frame 43.2ms   ← syntect 放大器启动
  tail 25KB → refresh 63.1ms draw 7.1ms  frame 70.2ms
  tail 40KB → refresh 100ms  draw 10ms   frame 110ms (avg 53ms, max 113ms)
message_end 落盘后: 整帧 10ms
对照 (重复代码块, 高亮缓存命中): 40KB 尾条 refresh 仅 10.7ms
```

复现：`cargo test diag_stream_tail -- --ignored --nocapture`。宿主 GUI 实测
输入→帧 52–113ms @长上下文（小会话 8–19ms）与上表逐点吻合。

机制：易失尾槽（inflight）**每帧全量重排** = O(尾条)；其中 markdown 解析
后对**内容互异/持续变化**的代码块逐帧重跑 syntect 高亮 = 逐帧缓存 miss
（×10 放大器；重复块对照组即其下限）。handle（0.01ms）、windowed wrap
（0.6ms）、paint（3–10ms）无罪。

约束（tui-app 规格原文）："代码块 MUST 做语法高亮"、"渲染 MUST 是纯函数式
（相同状态与宽度产出相同内容）"——后者否决一切时间型方案。

## Goals / Non-Goals

**Goals:**

- 流式帧成本 O(尾条) → O(新增行)：40KB 互异代码块尾条整帧 **< 5ms**
  （现状 ~110ms）。
- 输出与现全量路径**逐字节等价**（含流式中间态）——零可见行为变化，
  skip_specs 成立。
- 流式尾条场景成为常驻基准（补上原 bench 的暖帧口径盲区）。

**Non-Goals:**

- 时间节流/延迟高亮（违反"纯函数式"；且等价路线已足够）。
- `bash_lines` 工具输出流的同类问题（机制同源，follow-up 复用本设计）。
- 会话日志（agent journal）→ probe 回放格式转换器（工具债，另立）。
- 宿主侧节拍优化（GUI 25ms poll + 50ms 泵叠加，宿主仓另行处理）。

## Decisions

**D1 尾条行级 memo（核心）。** 易失尾槽内部细分到**完结行**粒度：
每完结行缓存 `(渲染 spans, 行末 markdown 块上下文, 行末 syntect
HighlightState)`；delta 追加只推进到新的完结行（逐行 memo），**开放行
（未收到换行的行）每帧单独重排**——单行成本 O(行)，与尾条长度无关。
markdown 块上下文（fence 开闭、列表/引用续行）与 HighlightState 都是
"前缀决定行内渲染"的确定性行推进状态机——syntect 的高亮 API 本身即
逐行推进（`highlight_line` 返回新状态），全量路径与增量路径是**同一算法**，
memo 纯记忆化。实施首日 spike 核对 `HighlightState: Clone` 便宜可用
（预期成立）；若个别语法状态不可廉价克隆，回退方案 = 每块只缓存行
spans、按需从块首重推进（仍 O(块) 每帧，作为兜底不采用为终态）。

**D2 等价性纪律（parity 逐字节）。** 沿用 `layout_cache` 既有纪律：
流式场景逐 delta 断言"memo 路径 vs 全量 oracle（`chat_lines_themed` +
单片 wrap）逐字节相等"，覆盖：互异代码块、嵌套列表/引用、thinking、
畸形/未闭合 fence、CJK 混排。宽度/flags/theme/override 失效沿用
`optimize-long-session-rendering` D4 失效矩阵（全量/条目级，不做行级
跨键复用）。

**D3 常驻基准钉预算（红灯先行）。** `diag_stream_tail_frame_cost` 转正为
`bench_streaming_tail_under_budget`（命名对齐 `bench_warm_frame_under_budget`）：
1500 条背景 + 40KB 互异代码块尾条逐 delta 整帧计时，断言 **max < 5ms**
（宽裕系数防 CI 抖动；现状 110ms 先红）。同步注记行级 memo 的缓存内存
增量（预期 <2MB，超 D6 预算 2 倍触发 P3 信号）。

**D4 交付顺序 = 基准先行。** 组1 先转正基准（红灯）→ 组2 实现行级 memo
（parity + 预算转绿）→ 组3 终验。基准红灯即回归防线，防止后续改动
再走回 O(尾条)。

## Risks / Trade-offs

- [行级 memo 在 markdown 嵌套边界（fence 内 fence、列表懒续行）状态
  推进错判] → parity 测试专设边界用例逐字节兜底；任何等价断言失败即
  该行回退全量重排（memo 失败安全，正确性优先）。
- [syntect 状态 Clone 成本实测不如预期] → D1 兜底方案已定；spike 先行。
- [memo 内存增长] → 只对易失尾槽存行级状态（落盘条目仍走 EntryLayout
  条目级）；基准内实测注记 + D6 预算。
- [并发/嵌套流式（thinking+text 双块并行）推进复杂度] → 行级状态按
  block 维度分槽推进（每 block 独立上下文状态机），parity 覆盖双块
  交错用例。

## Migration Plan

无数据迁移。组1 红灯基准合入后，组2/组3 依序落地，各组独立 commit、
独立 revert 回上一组状态；宿主复测（[term-rate] 输入→帧 ≤20ms）作为
验收注记记录于组3 提交信息。

## Open Questions

无——等价路线已定（非延迟高亮），spike 核对项与兜底方案见 D1。
