# Proposal

## Why

宿主 GUI（balabala-agent-gui）长会话打字实测输入→帧 **52–113ms**（小会话
8–19ms），体感"长上下文仍有延迟"。定向量测（`diag_stream_tail_frame_cost`，
1500 条/5.8MB 背景 + 40KB 流式增长尾条）归因：**易失尾槽每帧全量重排
O(尾条) × 互异代码块 syntect 逐帧重高亮（约 ×10 放大）**——尾条 40KB 时
整帧 **110ms（avg 53ms）**，与宿主实测逐点吻合；而无变化暖帧仅 1.09ms
（`optimize-long-session-rendering` 的 1.06ms 基准恰好未覆盖流式尾槽路径）。
91% 帧成本在尾槽重建，其中九成是重高亮；handle 0.01ms、wrap 0.6ms、
paint 3–10ms 全部无罪。

## What Changes

- **流式尾条行级增量布局与高亮 memo**（等价重构）：markdown 块上下文 +
  syntect `HighlightState` 按**完结行**前缀推进缓存，开放行每帧仅重排单行；
  输出与全量路径**逐字节等价**（parity 测试钉死）——同时满足规格的
  "代码块 MUST 做语法高亮"与"渲染 MUST 是纯函数式"两条约束（memo 是纯
  记忆化，不改变任何可见输出）。
- **常驻基准钉预算**：临时诊断 `diag_stream_tail_frame_cost` 转正为常驻
  基准（流式尾条场景，原 bench 的暖帧口径遗漏处），钉预算断言：
  40KB 互异代码块尾条的整帧 **< 5ms**（现状 ~110ms，先行红灯）。
- 预期收益：流式帧 110ms → ~3–5ms；宿主 [term-rate] 输入→帧口径
  预期回落至 ≤20ms（验收注记，复测在宿主仓）。
- **不做**：时间节流（违反"渲染 MUST 是纯函数式"）；未闭合块延迟高亮
  （无需——等价路线已拆掉放大器）；会话日志→probe 回放格式转换器
  （工具债，另行处理）；`bash_lines` 工具输出流同机制问题（follow-up）。

## Capabilities

### New Capabilities

（无——本 change 是性能等价重构。）

### Modified Capabilities

（无——可见行为零变化：输出与现路径逐字节等价。tui-app 的"markdown 渲染"
（代码块高亮、纯函数式）与"流式渲染"（实时显示）约束全部原样满足；
`.openspec.yaml` 已声明 `skip_specs: true`。）

## Impact

- 代码：`src/app/layout_cache.rs`（易失尾槽行级 memo：`EntryLayout` 增
  行级前缀状态）、`src/app/markdown.rs`/高亮路径（HighlightState 逐行推进
  接入）、`src/app/wrap.rs`（尾条局部 wrap 配合）；基准转正
  `src/app/layout_cache.rs` tests。
- 测试：parity 测试扩展到流式尾条逐 delta 等价；新常驻基准含预算断言；
  既有 382 测试零改动（等价重构纪律，与 `optimize-long-session-rendering`
  同款）。
- 性能面：流式帧成本从 O(尾条 × 块数) 降为 O(新增行)；缓存内存增加
  行级状态（预计 <2MB @40KB 尾条，基准内实测注记）。
- 约定：中文函数注释（功能/输入/输出）、`cargo test` + `cargo clippy
  --all-targets` 全绿后提交（AGENTS.md）。
