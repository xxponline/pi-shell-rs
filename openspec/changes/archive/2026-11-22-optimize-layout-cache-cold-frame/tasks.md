# Tasks

（路线：bench 先行——先钉红灯预算，再等价重构，最后宿主复测。）

## 1. 二次定位与 bench 先行

- [x] 1.1 view::render 内部二次分解（wrap/高亮/布局/paint 分段计时，临时
  探针）：确认 cells=1 暖帧 9ms 的构成与冷帧 533ms 的重排范围。
  **验证**：分段数值写入 design.md Open Questions 答案
- [x] 1.2 常驻基准钉预算：`bench_layout_cache_invalidation`（条目增删触发
  冷帧）+ `bench_warm_frame_cells1`（cells=1 暖帧），先行红灯。
  **验证**：冷帧预算 <30ms、暖帧预算 <3ms，跑红（现状 533ms/9ms）

  > **注记（红字基线 2026-11-22）**：两基准均带预算断言先行红灯（预算为
  > 宿主 release 口径；debug 档 ×6 宽松阈值防抖，`budget_factor`）。
  > release / debug 实测：
  > | 基准 | 现状 | 预算 | 判定 |
  > | --- | --- | --- | --- |
  > | `bench_layout_cache_invalidation` 冷失效帧（旧条目 tool_end → [k..n] 全量重排 600 条） | **649ms / 4906ms** | 30ms | RED |
  > | 同基准 条目增帧（消息边界） | 0.8ms / 7.4ms | 30ms | GREEN |
  > | `bench_warm_frame_cells1`（胖尾条 + 流式 in-flight，cells=1 回显帧） | **5.00ms / 28.1ms** | 3ms | RED |
  > | 同基准 暖帧构成（release） | 布局 4.39（toolcard 2.43 + wrapcount 1.56）、wrap 0.23、paint 0.22、高亮 0 | — | — |
  > 冷帧 649ms 与宿主尖峰 533ms 同类同机制；暖帧 5.0ms release（宿主 8.9ms
  > 含宿主进程负载）——构成与 design.md 归因一致（工具卡渲染 + wrap 计数
  > 为主，高亮 ≈0）。红灯随组 2 转绿，断言不拆。

## 2. 等价重构

- [x] 2.1 布局缓存失效路径增量重排（只重排变化条目及必要后续），parity
  测试钉逐字节等价；滚动锚定/选区坐标一致性测试覆盖。
  **验证**：bench 冷帧达标；parity 全绿

  > **注记（实现 2026-11-22）**：`EntryLayout` 增内容指纹
  > （`entry_fingerprint`：覆盖 layout_entry 全部输入 + 展开选项生效位；
  > 计时卡无指纹恒重建保走字），`refresh` 只重排指纹失配条目，命中条目
  > 随基值平移重编号 meta 全局序号（`shift_meta`）——必要后续收敛为
  > O(1) 级整数平移；结构增删由逐下标失配自愈。整体替换/漏标/entry#0
  > 被触统一走全量指纹校验（从 0 扫），整键变化（width/theme/flags）仍
  > 全清重建。**冷失效帧 649ms → 3.6ms（重排 600 条 → 1 条，release）**。
  > 新增 parity/一致性测试 4 个：消息序号平移、思考序号平移、同批多点
  > 失效（断言只重排 2 条）、钉住态窗口/回写/hits 与 oracle 逐字节一致
  > （滚动锚定 + 选区坐标）。
- [x] 2.2 暖帧基础段 memo（视口 wrap/布局按条目缓存命中）。
  **验证**：bench cells=1 暖帧达标；既有 392 测试全绿 0 warnings

  > **注记（实现 2026-11-22）**：与 2.1 同机制落地——尾窗 K≥2 与失效
  > 范围同走指纹比对，未变条目命中即免重排（wrap 计数随 EntryLayout
  > 缓存，不再每帧重算）；视口窗口化只排窗口内行（design D3 既有）。
  > **暖帧 5.00ms → 1.08ms**（布局 4.39→0.41，toolcard/wrapcount →0；
  > release），debug 档 28.1→8.31ms（余量为 TestBackend paint/wrap）。
  > 全量 `cargo test` 399 全绿（392 基线 + 7 新增）、clippy 零告警。

## 3. 验收

- [x] 3.1 宿主复测（balabala-agent-gui 2k 最大化流式打字）：`[term-rate]
  输入→帧` ≤20ms 量级（对照 220–500ms）；`[draw-cost]` ui 段回落。
  **验证**：数值注记于本任务，并回写宿主 change 4.1 对照表

  > **注记（宿主复测 2026-11-22，release 档）**：2k 最大化 + 流式回答中
  > 打字，体感顺畅。`[term-rate]`：闲置打字 输入→帧 avg=13ms max=18ms；
  > 流式中打字 avg=11–18ms max=18–26ms（feed/s 5–20、帧KB max 20KB、
  > miss=0）——达标 ≤20ms 量级（对照 220–500ms）✅。`[draw-cost]`：回显
  > ui=0.8–0.9ms（对照 8.9ms）、流式大帧（cells=6333/11.5KB）ui=0.9ms
  > （对照 8.5–9.3ms）、全程无冷帧尖峰（对照 533ms）✅。
  > **附带定案**：前期“卡顿感”主因是宿主跑 debug 档（`cargo run`）——
  > 同场景 debug 档 paint 6–8ms/帧、输入→帧 144–248ms（render/s 3–5
  > 宿主重绘节奏拖累）；release 档整链消失。遗留一次性热点：会话载入
  > 首建 212ms（174 条目/2977 行，toolcard 203ms = 96%）——O(全文) 一次
  > （design 预期），可作后续工具卡渲染/缓存治理 follow-up。
- [x] 3.2 全量 `cargo test --lib` + bench 全绿、0 warnings；`[draw-probe]`/
  `[draw-cost]` 临时探针处置（拆除或门控）。
  **验证**：命令输出对账

  > **注记（对账 2026-11-22）**：`cargo test --lib` debug/release 均 **398
  > 全绿**（392 基线 + 4 parity/一致性 + 2 常驻基准；临时诊断
  > `diag_render_segments` 已拆除）；`cargo clippy --all-targets` **0 告警**。
  > 4 基准双档全绿：`bench_layout_cache_invalidation` 冷失效 6.9ms/30ms、
  > `bench_warm_frame_cells1` 1.11ms/3ms（release；debug 19.8/8.39 vs ×6=180/18），
  > 既有流式尾条基准顺带受益（land 帧 113ms→19ms）。**探针处置（门控）**：
  > `[draw-probe]`/`[draw-cost]`/`[draw-seg]` 与 backend 分解统计统一由
  > `PI_RENDER_PROBE=1` 开启（默认零开销旁路/零日志；`render_probe::enabled`）；
  > `render_probe` 分段计量保留为常驻基准归因基础设施。清点：
  > `rg '"\[draw' src/` = 3 处输出点，全部在 `enabled()` 门控内。
