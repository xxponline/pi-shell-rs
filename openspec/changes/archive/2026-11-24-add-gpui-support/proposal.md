# Proposal

## Why

宿主 GUI（balabala-agent-gui，gpui-kit 应用）当前经 "ratatui → ANSI → vt100 → 画字符格"
管线嵌入本仓 TUI，该管线绕经 ANSI 序列化/解析两层，且与宿主共享渲染需跨框架搬运字节；
同时独立运行场景也缺少一个原生窗口形态。本变更新增 **GPUI 构建模式**：以 `gpui-kit`
原生渲染的 `GPUIAgentTerminal` 组件直连会话状态，先在独立窗口自证，为后续宿主直嵌
（替代 ANSI 管线）留门。

## What Changes

- 新增编译期特性 `gpui-support`（默认关闭）：关闭时构建产物与现有行为**逐字节同源**，
  不依赖 `gpui-kit` / `kameo`，现有测试全绿即回归网；开启时才编入 GPUI 模块树。
- 新增核心薄层 `RpcActor`（kameo actor，仅 `gpui-support` 构建）：拥有 `RpcClient`，
  命令走 ask、事件走 tell 扇出；**只做 RPC 路由，不做状态归约**。
- 新增 `GPUIAgentTerminal` 组件（`gpui-kit` 0.9.x，**内建控件优先**）：订阅事件、
  自带 `session/` 纯逻辑归约、三分区布局——消息区用内建虚拟滚动 + 内建 markdown，
  输入区用内建多行编辑器（TextArea/Editor），状态行用内建 status_bar；全局键位
  （enter/alt+enter/esc/ctrl+c）经映射喂共享语义层，编辑键交内建编辑器自理。
- 新增独立窗口 bin `pi-shell-gpui`（`required-features = ["gpui-support"]`）：
  创建窗口内嵌 `GPUIAgentTerminal`。
- **v1 语义对齐取最小子集**（对齐 `tui-app` 现有体验的 P0 部分）：启动探活、prompt
  流式、esc 中断/ctrl+c 清空、alt+enter 队列与 steer、多行编辑器（内建编辑器的
  插入/换行/撤销等能力，功能语义正确即可、不逐键对齐 TUI）、
  斜杠命令查讯型子集、三分区布局、markdown+代码高亮、thinking 折叠、read/bash/edit
  工具卡与通用卡兜底、状态行 tokens/cost、错误/重试提示、消息区滚动回看。其余
  （动态补全、其余工具卡、overlay 选择器、会话树、选区、滚动条、bash 模式、
  外部 `$EDITOR`、配色注入）后续 change 逐步补齐。
- 对共享代码（`rpc/`、`session/`、`app/`）**只做加法**：新增方法/导出，不改现有行为。

## Capabilities

### New Capabilities

- `gpui-agent-terminal`: GPUI 构建模式下的原生 Agent 终端组件——特性门控（关则零依赖
  零行为变化）、kameo 薄层 RPC 路由（ask 命令/tell 事件、无状态归约）、组件的会话
  渲染与输入语义（P0 最小子集对齐）、独立窗口运行形态、内建控件渲染模型（虚拟滚动
  消息区 + 内建 markdown、内建多行编辑器、状态栏）。

### Modified Capabilities

（无——ratatui 路径与既有 8 项能力行为不变；共享代码仅加法。）

## Impact

- **依赖**：`gpui-kit = "0.9.0"`（optional，features 含 syntect/test-support）、
  `kameo = "0.22.2"`（optional）；新 feature `gpui-support`。默认构建依赖面不变；
  Cargo.lock 会出现新条目（Cargo 机制，锁全特性图）但不下载不编译。
- **代码**：新增 `src/gpui_agent/{actor,terminal,window}.rs`（cfg 闸门）、
  `src/bin/gpui_window.rs`；`src/lib.rs` 增加条件模块声明；共享模块仅加法扩展。
- **构建/CI**：测试矩阵扩为 `cargo test`（现状）+ `cargo test --features gpui-support`；
  clippy 同理。
- **下游**：balabala-agent-gui 现有 ANSI 嵌入管线不受影响（并存策略）；后续接入时
  以 `features = ["gpui-support"]` 依赖本仓并直嵌组件（框架同为 gpui-kit，类型相认）。
- **路线图**：`Docs/roadmap.md` 需补记 GPUI 构建模式的定位（形态 B 之外新增原生窗口
  形态，归档时同步）。
