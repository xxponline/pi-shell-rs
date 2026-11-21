# Tasks

（路线：bench 先行——先钉红灯预算，再等价重构，最后宿主复测。）

## 1. 二次定位与 bench 先行

- [ ] 1.1 view::render 内部二次分解（wrap/高亮/布局/paint 分段计时，临时
  探针）：确认 cells=1 暖帧 9ms 的构成与冷帧 533ms 的重排范围。
  **验证**：分段数值写入 design.md Open Questions 答案
- [ ] 1.2 常驻基准钉预算：`bench_layout_cache_invalidation`（条目增删触发
  冷帧）+ `bench_warm_frame_cells1`（cells=1 暖帧），先行红灯。
  **验证**：冷帧预算 <30ms、暖帧预算 <3ms，跑红（现状 533ms/9ms）

## 2. 等价重构

- [ ] 2.1 布局缓存失效路径增量重排（只重排变化条目及必要后续），parity
  测试钉逐字节等价；滚动锚定/选区坐标一致性测试覆盖。
  **验证**：bench 冷帧达标；parity 全绿
- [ ] 2.2 暖帧基础段 memo（视口 wrap/布局按条目缓存命中）。
  **验证**：bench cells=1 暖帧达标；既有 392 测试全绿 0 warnings

## 3. 验收

- [ ] 3.1 宿主复测（balabala-agent-gui 2k 最大化流式打字）：`[term-rate]
  输入→帧` ≤20ms 量级（对照 220–500ms）；`[draw-cost]` ui 段回落。
  **验证**：数值注记于本任务，并回写宿主 change 4.1 对照表
- [ ] 3.2 全量 `cargo test --lib` + bench 全绿、0 warnings；`[draw-probe]`/
  `[draw-cost]` 临时探针处置（拆除或门控）。
  **验证**：命令输出对账
