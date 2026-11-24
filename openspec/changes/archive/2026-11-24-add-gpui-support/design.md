# Design

## Context

- 现有架构：`app (ratatui) → session (纯逻辑) → rpc (std 三线程) → pi`，零 tokio；
  embed 路径经 `VirtualBackend → ANSI → 宿主 vt100` 渲染（roadmap 技术决策与三轮
  渲染性能优化均围绕此管线）。
- 宿主 balabala-agent-gui 是 gpui-kit 0.9.x 应用，已有 kameo 0.22 actor 中间层与
  GPUI→crossterm 按键映射表（`keystroke_to_keyevents`）；其 ANSI 嵌入管线在本变更
  中**并存不退役**。
- 复用底子（实测确认）：`session/`（状态归约/transcript）、`app/autocomplete.rs`
  （补全候选）、`app/commands.rs`（命令元数据与分发）、`view::header_line`（状态行
  装配）、`app/editor.rs`（仅作 App 侧文本镜像与斜杠清空语义）；`wrap.rs`/
  `layout_cache.rs` 抽核仅供 ratatui 路径，GPUI 改用内建控件后不消费。
- 约束（本变更定死）：默认关闭的编译特性；关 = 与现状逐字节同源、零新依赖；
  对共享代码只许加法。

## Goals / Non-Goals

**Goals:**
- `gpui-support` 特性门控的 GPUI 构建模式，关闭时零 gpui/kameo 依赖、行为不变。
- kameo 薄层 RPC 路由（仅 GPUI 构建编入），核心不做状态归约。
- `GPUIAgentTerminal`：内建控件优先原生渲染 + P0 最小子集功能语义对齐。
- 独立窗口 bin；为将来 balabala 直嵌留门（框架同为 gpui-kit）。

**Non-Goals:**
- 不动 ratatui 路径与 embed/ANSI 管线（宿主现有管线继续服役）。
- 不做 P0 之外的对齐项（动态补全、write/grep/find/ls 卡、overlay 选择器、会话树、
  选区、滚动条/跳转、bash 模式、外部 `$EDITOR`、配色注入）——后续 change。
- 不改 `session`/`rpc` 归约与协议语义；不引 tokio 进 ratatui 路径。
- 渲染不做与 ratatui 的像素级/cell 级一致（见 D2；spec 已改为功能语义对齐）。

## Decisions

### D1 框架 = gpui-kit 0.9.x（而非 Zed 官方 gpui）

并存策略下"宿主接入后置"，但接入时两份不同 GPUI 框架的 `Entity<T>`/`Render`
在同进程互不相认——选官方 gpui 等于把将来的接入变成推倒重来。gpui-kit 另有：
宿主仓现成集成模式可抄、`test-support`（无头窗口 + ElementSnapshot）提供语义对齐
测试基座。代价：文档较薄，读宿主源码补。

**替代**：Zed gpui——文档好但锁死接入，否决。

### D2 渲染模型 = gpui-kit 内建控件优先（有内建就用内建）

- 消息区：内建虚拟滚动组件（回看/贴底/跳底）承载会话条目；助手文本用内建
  markdown 渲染（syntect 代码高亮随 kit 特性启用）；工具卡/thinking/错误提示
  无内建对应物，按条目自绘为简单元素。
- 输入区：内建 TextArea/Editor（自带光标/选区/撤销/换行）；全局键位在包装层
  拦截送共享语义层，其余编辑键交编辑器自理。
- 状态行：内建 status_bar 分段承载 phase/model/tokens/cost/notice。
- 放弃与 ratatui 的 cell 级换行/坐标一致（spec 相应修订）；`wrap`/`layout_cache`
  抽核仅服务 ratatui 路径，GPUI 不再消费。
- 取舍：不再保证与终端 TUI 像素级观感一致，换取少自绘代码、虚拟滚动性能与
  内建控件成熟的交互（IME/选区/撤销）。

**替代（已否决）**：等宽字符网格自绘——已按原方案实现过一版（逐行 StyledText +
cell 度量），帧开销与自绘代码量都偏大，且与“优先内建”的方向冲突。

### D3 架构与依赖方向

```
pi --mode rpc
     ^ JSONL
     v
RpcActor (kameo, cfg(feature))          ← 核心薄层：仅路由
  ask: CommandRequest → Result<Value>   （按 id 匹配响应/超时）
  tell: SessionEvent 扇出给订阅者
  拥有 RpcClient（std 泵线程原样复用，不改）
     ^                    ^
     | kameo msg          | kameo msg
GPUIAgentTerminal                  （future: 宿主内嵌同组件）
  - 自带 SessionState 归约（session/ 纯逻辑复用；App 作共享状态核）
  - 全局键位映射（enter/esc/ctrl+c/alt+enter → App::handle_key）
  - 内建控件：虚拟滚动消息区 + markdown / TextArea / status_bar
  - autocomplete/commands/header_line 纯逻辑复用
```

- **状态归约归前端**：各前端连各自 pi 进程，状态本不同源；核心保持"薄"。
- **ratatui 路径完全不经过 kameo**（用户拍板：kameo 只属 GPUI 构建模式）——
  它继续直用 `RpcClient`，`spawn_event_sources` 一行不动。
- **RpcClient 只加法**：`RpcActor` 通过既有 `command_sender()`/`take_events()`
  组装，不改泵线程。
- **关闭权归会话句柄**（实施期细化）：`RpcSession` 持有 `RpcClient`（shutdown /
  Drop 回收子进程，幂等），actor 持其 `CommandSender` 子集——避免 Drop 路径
  依赖 kameo 弱引用生命周期；"薄层"语义不变（路由归 actor，无归约）。
- **命令应答不阻塞 actor**（实施期细化）：`CommandRequest` 走 kameo
  `DelegatedReply` + 独立线程执行阻塞的 `send_command`，事件扇出不被慢命令拖住。

### D4 特性门控形态

```toml
gpui-kit = { version = "0.9.0", features = ["syntect"], optional = true }
kameo    = { version = "0.22.2", optional = true }
tokio    = { version = "1", features = ["rt","macros","sync","time"], optional = true }
[features]
gpui-support = ["dep:gpui-kit", "dep:tokio", "dep:kameo"]   # tokio 仅为 kameo 运行时与 #[tokio::test]（实施期补充）
[[bin]]
name = "pi-shell-gpui"; required-features = ["gpui-support"]
```

模块树 `src/gpui_agent/{actor,terminal,window}.rs` 整体 `#[cfg]`；`lib.rs` 仅加
条件 `pub mod`。关特性时该树编译期不存在 = 与现状同源。
Cargo.lock 会出现新条目（Cargo 锁全特性图，无法避免），但不下载不编译。
测试矩阵：现状列 + `--features gpui-support` 列。

**替代**：独立 crate 隔离——过度，且 lib 内复用（session/editor）要跨 crate 重导，
否决。

### D5 输入通路（全局键位拦截 + 内建编辑）

- 编辑：文本编辑交内建 TextArea/Editor（插入/换行/撤销/词移由其自理），组件
  不把编辑键送入 `App::handle_key`。
- 全局键位：包装层对 enter / alt+enter / esc / ctrl+c（以及补全菜单打开时的
  导航/应用键）经既有 GPUI→crossterm 映射喂 `App::handle_key`，复用提交/steer/
  中断/清空/补全导航语义；命中即 `stop_propagation`，内建编辑器不消费。
- 文本桥接：语义动作前后 TextArea ↔ `App.editor` 双向同步（提交读全文、斜杠
  命令清空/队列回填写回编辑器），保证斜杠分发与回填等功能语义正确。
- 滚轮：交内建虚拟滚动自理，不再走 `UiEvent::Wheel` 注入。

### D6 共享代码纪律

对 `rpc/`、`session/`、`app/` 只许新增 API/新方法，不改既有导出行为；每组加法
必须有新测试且旧测试全绿（spec「共享代码仅加法演进」）。

## Risks / Trade-offs

- [内建控件与 TUI 的键位/观感差异] → spec 已改为功能语义对齐（不再要求 cell
  一致）；验收按功能清单（提交/中断/清空/渲染内容）走无头快照与假 pi 断言。
- [内建控件后仍有条目组装与工具卡自绘成本] → P0 砍到最小子集，条目元素配
  ElementSnapshot 测试；每事件一帧的重绘开销由泵合并通知 + 渲染探针治理
  （task 5.5）。
- [双测试矩阵，CI 时间增长] → gpui 列先只跑 `gpui_agent` 单测 + 组件对齐测试，
  不重复跑 ratatui 套件。
- [kameo/tokio 进入依赖图（仅特性开时）] → 与宿主已验证的同进程模式一致
  （宿主 gpui+kameo 共存）；ratatui 路径零接触。
- [内建控件消费面扩大后与共享层耦合] → 共享层仍只加法（App 语义方法
  `pub(crate)` 化），GPUI 侧网格遗留随 task 5.6 清理，ratatui 路径不受影响。
- [Cargo.lock 膨胀被误认为"引入了依赖"] → 文档注明：lock 记录全特性图，
  默认构建不下载不编译。

## Migration Plan

1. 特性骨架 + 空模块树（编译门控自证：两列矩阵全绿）。
2. RpcActor + 独立窗口空壳（探活、事件扇出通路打通）。
3. 复用模块落地（session 归约、App 状态核、autocomplete/commands 接入组件）。
4. P0 内建控件渲染面 + 全局键位拦截与文本桥接。
5. PTY/无头验收（P0 清单逐项），归档时补记 roadmap。

回滚：删特性与 `gpui_agent/` 树即回现状（共享代码仅加法，无行为耦合）。

## Open Questions

- 宿主接入（P2）时的契约细节（尺寸量测、配色注入如何映射到组件 API）——接入
  change 再答，不阻塞 P0。
- ratatui 路径长期去留——P0 自证后再议。
