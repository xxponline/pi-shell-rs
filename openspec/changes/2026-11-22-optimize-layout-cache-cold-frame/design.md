# Design

## Context

宿主 `[draw-cost]` 分解实测（2026-11-22，change
`2026-11-20-streaming-input-echo-latency` 定位 A，257×61 最大化 + 流式）：

| 场景 | total | ui | diff | ansi | cells | bytes |
| --- | --- | --- | --- | --- | --- | --- |
| 闲档回显（cells=1） | 2.4ms | 1.6ms | 0.2ms | 0.7ms | 1 | 40B |
| 流式期纯回显（cells=1） | 10.4ms | 8.9ms | 0.2ms | 1.4ms | 1 | 40B |
| 流式大帧 | 11–13ms | 8.5–9.3ms | 0.2ms | 2.8–4.2ms | 5k–12k | 9–20KB |
| 卡顿尖峰 | 536.5ms | **533.5ms** | 0.2ms | 2.8ms | 7577 | 12KB |

口径：ui = `terminal.draw` 闭包内 `view::render`（布局/wrap/高亮构建）；
diff = total − ui − ansi（ratatui 前后 buffer 比较）；ansi = `backend.draw`
CSI/SGR 生成（VirtualBackend 统计）。探针已随宿主 change 2.3 拆除，
本仓 `[draw-probe]`/`[draw-cost]` 暂留（本 change 定位用）。

既有优化边界：
- `optimize-long-session-rendering`（4f7b6ef）：条目级布局缓存 + 视口化 wrap，
  暖帧 600ms→1.06ms——**冷帧（缓存失效全量重排）600ms 仍在**；
- `optimize-streaming-tail-rendering`（9df02dc）：尾条行级 memo，110ms→3.2ms
  ——治了尾槽重高亮，未治布局缓存冷失效与全帧基础成本。

## 归因（已定案）

- **冷帧 533ms** = 布局缓存失效 → O(transcript) 全量重排。触发条件：流式
  条目增删（缓存 key 失效）。最大化视口（257×61）wrap 量更大更贵。
- **暖帧 cells=1 的 9ms** = 每帧 view::render 的 O(transcript) 基础段
  （视口 wrap/布局未全缓存命中）。闲置小会话同 cells=1 仅 1.6ms，差值
  即会话规模项。
- diff/ansi 排除（≤4ms，与 cells/bytes 弱相关）。

## 方向（候选）

1. **失效路径增量重排**：条目增删只重排该条目及后续受影响段；保持逐字节
   等价（parity 测试）。注意滚动锚定（视口随内容推移）与选区坐标一致性。
2. **暖帧基础段 memo**：视口内 wrap 结果按 (条目, 宽度, 样式上下文) 缓存；
   cells=1 场景不动布局路径（纯画布 diff 由 ratatui 承担）。
3. bench 先行（1568a22 传统）：`bench_layout_cache_invalidation`（冷帧）+
   `bench_warm_frame_cells1`（暖帧基础）先红后绿。

## Risks / Trade-offs

- [增量重排输出偏离全量] → parity 测试钉逐字节等价（同 9df02dc 路线）。
- [缓存 key 粒度不当导致命中率低/内存涨] → bench 记缓存命中率与驻留。
- [滚动锚定漂移] → 失效重排保持 top 可见条目锚定；测试覆盖滚动中流式更新。

## Open Questions

- 冷帧能否降到 <30ms，还是只能靠"失效不再全量"消除（增量后无冷帧概念）？
- cells=1 的 9ms 中 wrap/高亮/布局各占多少（需二次分解：view::render 内部打点）？
