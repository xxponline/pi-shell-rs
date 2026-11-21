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

- 冷帧能否降到 <30ms，还是只能靠“失效不再全量”消除（增量后无冷帧概念）？
- cells=1 的 9ms 中 wrap/高亮/布局各占多少（需二次分解：view::render 内部打点）？

## Open Questions 答案（任务 1.1 二次分解实测，2026-11-22）

临时探针（`render_probe`：布局/高亮/工具卡/wrap 计数/wrap/paint 分段 +
重排范围计数；`[draw-seg]`/`[diag]` 输出，任务 3.2 处置）实测，257×61
TestBackend（宿主 2k 最大化同宽高），两档数据 = release / debug：

**冷帧重排范围（问题 2 前半）**：失效下标 k → `[k..n]` **全部重排**
（O(transcript)）；尾部条目增（消息边界）只重排尾窗 2 条，不冷。

| 场景 | 触发（脏下标） | 重排范围 | 整帧（release/debug） |
| --- | --- | --- | --- |
| 合成 1502 条：旧条目 tool_end 命中 entry#1 | 1 | from=1 entries=1502 lines=109742 | 105ms / 606ms |
| 真实会话 1020 条：同上 | 1 | from=1 entries=1019 lines=14066 | **1430ms / 3976ms** |
| 消息边界条目增（land+user push+新流式槽） | 尾（n-2） | from=末尾 entries=2 lines=3 | 0.5ms / 5ms |
| 漏标降级（安全网） | 0 | from=0 全量 | 1444ms / 4038ms |

**归因补记（触发面修正）**：O(transcript) 冷帧不是“尾部条目增删”（实测
尾增仅 2 条 ≈ 0.5ms），而是**失效下标落在低位**：tool_end 迟到命中旧条目
（`apply_tool` 回填）、turn_end 回填旧 assistant 条目、漏标降级等。
流式 agentic run 里旧条目工具卡频繁更新 + 条目不断新增，命中下标随会话
增长越来越深 → 频繁触发 `[k..n]` 全量重排（宿主尖峰同机制；533ms 与本次
1430ms 量级差来自会话形态/缓存冷暖）。全量重建成本 **98% 在工具卡渲染**
（真实会话 cold_build：layout=1443ms 中 toolcard=1414ms、wrapcount=19ms、
高亮段仅 6ms）——read/write 卡预览全文语法高亮走 HL_CACHE（容量 64、满即
整体 clear）：互异预览 >64 即 thrash，全部重高亮。

**暖帧 cells=1 构成（问题 2 后半）**：构成 = **布局段为主**（尾窗 K=2 +
in-flight Other 槽每帧全量重排），其中工具卡渲染 + 逐行 wrap 计数占大头；
wrap（命中扫描+窗口化）与 paint 少量；**高亮段 ≈ 0**（命中缓存只克隆）。
所谓“会话规模项”实为**尾条目体量项**（O(尾窗条目)，非 O(transcript) 扫描
——瘦尾真实会话暖帧仅 0.9ms）：

| 场景（cells=1 无事件帧） | 整帧 | 布局（toolcard/wrapcount） | wrap | paint | 高亮 |
| --- | --- | --- | --- | --- | --- |
| 真实会话瘦尾（尾窗 38 行） | 0.9 / 7.8 | 0.1（0/0） / 0.3 | 0.3 / 1.6 | 0.4 / 4.5 | ≈0 |
| 胖尾（尾窗 2 条 2224 行 + 6 张 read 卡 8KB 预览） | 3.1 / 24 | 2.1（0.4/1.4） / 13（2/9） | 0.2 / 1.7 | 0.7 / 8 | ≈0 |
| 宿主形态（胖尾 + 流式 in-flight 工具卡/40KB 文本） | 2.6 / 30 | 2.2（0.4/1.4） / 20（2.5/15） | 0.1 / 1.8 | 0.2 / 8 | ≈0 |

宿主 ui=8.9ms 与本表同机制：尾条目更肥 + 系统负载即到该量级（本表工具卡/
wrap 计数项同比例放大）。

**冷帧问题答案**：**<30ms 可达**——失效路径增量重排（只重排变化条目）后
任何失效帧都是 O(条目)，无“全量冷帧”概念；初始冷建/键变/漏标降级仍为
O(transcript)（真实会话 release ≈1.4s），其瓶颈是工具卡高亮缓存 thrash，
随任务 2.2 的按条目 memo/缓存命中治理一并收敛。
