# Proposal

## Why

宿主 GUI（balabala-agent-gui）2k 最大化（257×61）流式打字实测输入→帧
**220–500ms**（62×24 常用档 9–18ms 已达标）。宿主 change
`2026-11-20-streaming-input-echo-latency` 的 `[draw-cost]` 分解定位
（ui=闭包内 view::render、diff=ratatui buffer 比较余量、ansi=backend CSI 生成）：

| 场景 | total | ui | diff | ansi | cells |
| --- | --- | --- | --- | --- | --- |
| 闲档回显（cells=1） | 2.4ms | 1.6ms | 0.2ms | 0.7ms | 1 |
| 流式期纯回显（cells=1） | 10.4ms | **8.9ms** | 0.2ms | 1.4ms | 1 |
| 流式大帧 | 11–13ms | 8.5–9.3ms | 0.2ms | 2.8–4.2ms | 5k–12k |
| **卡顿尖峰** | **536.5ms** | **533.5ms** | 0.2ms | 2.8ms | 7577 |

**diff/ansi 无罪**（各 ≤4ms）；成本全在 `view::render` 的布局/wrap 构建：

- **冷帧 533ms**：流式条目增删 → 条目级布局缓存（`optimize-long-session-rendering`）
  失效 → **全量重排 O(transcript)**（该 change 的冷帧 600ms 同源，被流式条目
  更新频繁触发）；
- **暖帧基础成本 9ms**：画 1 个 cell（纯回显 40B）也要 8.9ms 构建 UI
  （闲置同 cells=1 仅 1.6ms）——每帧 O(transcript) 基线未消。

宿主侧回显 draw 排在冷帧/大重建后（event_loop 单线程）→ 输入延迟 220–500ms。
与 `optimize-streaming-tail-rendering` 的尾条 memo 互补：那次治了尾槽重高亮
（110ms→3.2ms），这次治布局缓存冷失效与全帧基础成本。

## What Changes

- **布局缓存失效路径增量重排**：条目增删只重排变化条目及其后的必要部分，
  不全量重排；输出与全量路径**逐字节等价**（parity 测试钉死，同
  `optimize-streaming-tail-rendering` 等价重构路线）。
- **暖帧基础成本下压**：cells=1 场景（纯回显）的 view::render 从 ~9ms 压至
  ≤3ms——视口化 wrap/布局的每帧全量段找出并 memo（增量排版文档归因
  `538db8f` 已指出失效边界，本次补缺）。
- **常驻基准钉预算**：冷帧场景（条目增删触发缓存失效）与 cells=1 暖帧场景
  进 bench，钉预算：冷帧 < 30ms（现状 533ms）、cells=1 暖帧 < 3ms（现状 9ms）。
- 预期收益：2k 最大化流式输入→帧 220–500ms → ≤20ms 量级（宿主验收注记）。
- **不做**：diff/ansi 层优化（分解已证无罪）；时间节流（违反"渲染 MUST 是
  纯函数式"）；event_loop draw 调度（宿主 change 已做输入优先 draw）。

## Capabilities

skip_specs（纯渲染性能优化，无 spec 级行为变化；等价重构 parity 钉死）。
