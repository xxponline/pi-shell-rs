# Tasks

## 1. 基准转正（记录现状基线；红字本地跑，断言随组 2 合入）

- [x] 1.1 临时诊断 `diag_stream_tail_frame_cost` 转正为 `bench_streaming_tail_under_budget`
  （命名对齐 `bench_warm_frame_under_budget`；保留分段计时 eprintln，格式并入
  `[bench]` 风格；去掉"用后删"标注）。**本组不加预算断言**（AGENTS 全绿提交纪律：
  红灯断言随组 2 修复一并合入）；本地跑出红字基线（40KB 互异代码块尾条
  frame≈110ms）记入提交信息。验证：`cargo test bench_streaming_tail -- --nocapture`
  输出完整分段基线；全量测试 382+ 全绿
- [x] 1.2 基准覆盖流式中间态 parity 计数注记（每 25 delta 打印 memo/oracle
  等价计数占位，组 2 接真断言）。验证：`cargo test bench_streaming_tail -- --nocapture`
  输出含 parity 计数行
- [x] 1.3 组 1 验收：`cargo test` + `cargo clippy --all-targets` 全绿；
  组 1 独立 commit

## 2. 尾条行级增量布局与高亮 memo（design D1–D3）

- [x] 2.1 spike：核对 syntect `HighlightState` 逐行推进 API 与 Clone 成本；
  结论回写 design D1（若不可廉价克隆则启用兜底方案并记录）。验证：spike
  结论写入 design.md 的 D1 段
- [x] 2.2 尾槽行级 memo 实现：markdown 块上下文 + `HighlightState` 按**完结行**
  前缀推进缓存（按 block 分槽，双块交错独立推进）；开放行每帧单行重排；
  沿用 D4 失效矩阵（宽度/flags/theme 全量、override 条目级）。验证：`cargo build`
  通过 + 新增函数中文注释（功能/输入/输出）齐全
- [x] 2.3 parity 测试：流式逐 delta "memo 路径 vs 全量 oracle 逐字节相等"，
  边界用例含互异代码块、嵌套列表/引用、thinking、畸形/未闭合 fence、
  CJK 混排、双块交错；等价断言失败处该行回退全量重排（失败安全）。
  验证：新增 parity 测试全绿
- [x] 2.4 预算断言合入：`bench_streaming_tail_under_budget` 钉 **max frame < 5ms**
  （现状 ~110ms 红 → 转绿）+ 行级 memo 缓存内存注记（超 D6 预算 2 倍 = P3 信号）。
  验证：基准断言通过且输出含内存注记
- [x] 2.5 组 2 验收：`cargo test` + `cargo clippy --all-targets` 全绿（382 既有
  零改动 + 新增对账）；组 2 独立 commit

## 3. 终验

- [ ] 3.1 回归：`bench_warm_frame_under_budget` 原值不劣化（暖帧 <5ms、
  draw <50ms 断言原绿）；PTY 长会话冒烟（1829 条消息）无卡死。验证：两处
  命令输出原绿
- [ ] 3.2 宿主口径验收注记：`[term-rate] 输入→帧 ≤20ms` 复测口径与复测结果
  （复测在宿主仓 balabala-agent-gui）写入组 3 提交信息/文档注记。验证：提交
  信息含宿主口径注记
- [ ] 3.3 组 3 验收：全量 `cargo test` + `cargo clippy --all-targets` 全绿；
  组 3 独立 commit

## Workflow follow-up

- 评审满足后 `/opsx-archive` 归档（skip_specs，无 delta 同步）。
- follow-up 立项候选：`bash_lines` 工具输出流行级 memo 复用；会话日志 →
  probe 回放格式转换器；宿主 GUI 泵节拍优化（宿主仓）。
