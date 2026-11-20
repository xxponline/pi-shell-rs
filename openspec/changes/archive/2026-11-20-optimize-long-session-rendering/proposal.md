# Proposal

## Why

长会话（实测 5.9MB / 1496 条目）下每帧渲染是 O(全文) 的：`chat_lines_themed` 全量
排版 + `wrap_for_viewport` 全文三遍（wrap / collect_hits / bg 补齐）+ `draw` 开头
一次整个 `SessionState` 的深拷贝发布，导致输入→帧延迟随会话增长从 149ms 爬到
275ms（约 3.6fps），宿主 GUI 中滚动/打字体感"卡死"。归因核查（2026-11-2x）已
修正 `Docs/transcript-incremental-rendering.md` 中"事件积压死亡螺旋"的错误机制：
`event_loop` 每轮 drain 合批 + 单次重绘，队列天然有界；真实问题是单帧成本随内容
增长。根治方案是条目级布局缓存 + 增量排版。

## What Changes

- **条目级布局缓存（D1）与序号/行号前缀组合（D2）**：布局产物按条目缓存，
  每帧只重排新增/变更条目，全局序号改"条目内局部计数 + 基数组合"；每帧成本
  从 O(全文) 降到 O(新/变更条目 + 视口行)，可见行为逐字不变（等价重构）。
- **变更检测前置（P1 内）**：session 层以 `dirty_from`/`rev`（纯 usize，
  不引入层依赖）标记变异点，配合尾部易失槽（in-flight 块 + 运行中 bash）
  与尾部窗口（K≥2 条，覆盖 tool_end 迟到乱序），驱动增量重建。
- **视口化 wrap（D3，P2）**：以 wrap 计数前缀和定位视口逻辑行区间，只 wrap
  可见段；全文路径保留供选区文档使用。
- **BREAKING**：删除 `EmbeddedApp::session_state()` 与每帧 `SessionState`
  深拷贝发布（`state_slot`）。全仓证据为三个零：宿主（balabala-agent-gui
  全部源码）零引用、本仓 src/tests/examples 零调用、Docs 与 specs 零契约；
  该发布持锁克隆 5.9MB 状态，纯滚动帧照付，P1 后将成为单帧最大成本项。
- **验收设施**：缓存路径 vs 全量路径逐字节 parity 单测（空会话/单条目/流式
  中途/override 切换/宽度变化/中部变异全谱覆盖）+ 基于 `replay_jsonl` 离线
  回放 5.9MB 基准会话的计时断言（阈值宽松防 CI 抖动）。
- **文档同步**：实现归档时同步修订 `Docs/transcript-incremental-rendering.md`
  的机制归因（死亡螺旋→每帧 O(全文)、热路径补漏项）与验收指标。

**不在本方案内**：宿主侧滚轮事件合并（方案 A，归 balabala-agent-gui）；
RPC 协议与会话 JSONL 存储格式；`virtual_terminal`/ANSI 输出管线；"卡死
不自恢复"的复现调查（暂缓，另行立项）；D6 内存上限策略的第 2/3 档（P3 可选，
按实测触发）。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

（无。本变更为等价重构：可见行为不变，tui-app 既有滚动/渲染规格不改；
`session_state` 句柄无规格覆盖，删除属实现面收缩；性能预算随 parity 单测与
基准断言落入测试代码，不新增规格需求。变更经 `skip_specs: true` 显式豁免
spec delta，符合"行为不变则规格不变"。）

## Impact

- **代码**：`src/app/chat.rs`（条目布局缓存）、`src/app/wrap.rs`（视口化
  wrap 入口）、`src/app/view.rs` / `src/app/mod.rs`（渲染读缓存、
  state_slot 移除）、`src/session/transcript.rs`（`dirty_from`/`rev` 埋点，
  纯 usize）、`src/app/embed.rs`（API 删除）。
- **API**：`EmbeddedApp::session_state()` 删除（**BREAKING**）；宿主仓
  balabala-agent-gui 以 git 依赖钉版本且零调用点，编译零破坏（已核实）。
- **内存**：5.9MB 会话的全量 `EntryLayout` 缓存估算 6–18MB，P1 以基准实测
  校准，超预算时按 D6 第 2 档（近窗口缓存 + LRU）触发 P3。
- **依赖**：无新增第三方依赖。
