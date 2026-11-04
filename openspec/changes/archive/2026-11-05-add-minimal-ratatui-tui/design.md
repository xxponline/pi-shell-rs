# Design

## Context

阶段 0+1 已交付并归档：`rpc::RpcClient`（spawn pi、id 关联响应、事件分流）与
`session::SessionState`（弱类型事件 → `Entry`/`Block` transcript）。现有
`src/main.rs` 仍是 hello world，没有终端代码。本变更是 ratatui 首秀，只做
最小闭环：启动 pi、显示滚动对话、输入发送、流式渲染、esc 中断/退出。

已确认的事实：
- 依赖只有 serde/serde_json；无 async runtime（probe 用 std 三线程已足够）。
- ratatui 0.30 / crossterm 0.2x 可用。
- `SessionState` 目前把 `inflight: Vec<Block>` 设为私有，`message_end` 前无法
  实时读取，需要补一个只读访问器。
- 事件混在一条 stdout：`RpcClient.events()` 给出 `ClientEvent::{Event, ParseError, Closed}`，
  命令响应用 `send_command`。prompt 成功只代表受理，真正结束看 `agent_settled`
  （与探针/协议笔记一致）。

## Goals / Non-Goals

**Goals:**
- 用 std 线程 + mpsc 把键盘、RPC 事件、渲染 tick 汇到一个主事件循环。
- 渲染完全由 `SessionState`（含新增 in-flight 只读视图）驱动，布局拆成可在
  无 TTY 下单测的纯函数。
- 终端进入/退出可靠恢复，panic 也不留烂摊子。
- 接真实 pi 跑通：启动探活 → 提问 → 看流式 → 中断/退出。

**Non-Goals（阶段 3+）：**
- markdown 语法高亮、代码着色、thinking 折叠交互、工具结果卡片。
- follow_up/steer 队列交互、模型/会话选择器、会话树、extension UI 弹窗。
- 多行编辑器/历史/自动补全/外部 $EDITOR/图片。
- 不做复杂的 tokio 异步架构。

## Decisions

### 1. 依赖：ratatui + crossterm，不引 async

```toml
ratatui = "0.30"
crossterm = { version = "0.2x", features = ["event-stream"] }  # 版本按 0.30 兼容解析
```

不启用 ratatui 的其他后端特性。终端 IO 用 crossterm 直接管理（enable_raw_mode、
EnterAlternateScreen、event::poll/read）。ratatui 0.30 自带 crossterm 后端，
具体 crossterm 版本以 `cargo add` 解析的兼容组合为准（实现时锁定一次）。

**备选**：直接用 crossterm 手写布局——被否，重复造轮子；tokio+crossterm event-stream
也暂不需要，std 线程足够三类事件源。

### 2. 模块布局（app 层落在 src/，主 bin 只做装配）

```
src/
├── main.rs               # fn main: 构造 App, run()；参数极简（可能只有 --version）
├── app/
│   ├── mod.rs            # App 状态 + run 事件循环
│   ├── terminal.rs       # TerminalGuard: RAII 进入/恢复 raw mode + alt screen + panic hook
│   ├── event.rs          # 统一的 UiEvent 枚举 + 两个源线程（键盘/RPC）
│   ├── view.rs           # 纯函数: &SessionState + 编辑器缓冲 -> ratatui Frame 分区
│   └── chat.rs           # transcript/in-flight -> 可渲染的文本行（阶段2: 纯文本/占位）
├── rpc/ ...（已有）
└── session/ ...（已有，新增只读访问器）
```

保留 `src/bin/probe`；默认 package bin（src/main.rs）变为 TUI。

### 3. 事件循环：std 线程 + 单一 mpsc 到主线程

```
键盘线程:  crossterm event::read()  ── Key(...)  ──┐
                                                  ├──► mpsc<UiEvent> ──► App 主线程
RPC 线程:   client.events().recv()  ── Rpc(...) ──┘                       │
                                                                         ├─ Key: 编辑 / Enter发送 / esc
渲染:      每帧（或事件驱动 + 16ms tick poll）                           ├─ Rpc: state.handle(raw)
                                                                         └─ draw(frame)
```

- 键盘线程循环 `event::read()`，把 KeyEvent 转成 `UiEvent::Key`。
- RPC 线程持有 `client.events()` 的接收端克隆（`try_clone`）；收到
  `ClientEvent::Event(v)` → `UiEvent::Rpc(v)`；`Closed` → 退出信号；
  `ParseError` 阶段 2 只记录为一条提示。
- 主线程阻塞在 `rx.recv()`，并在需要时用短超时聚合，让流式期间持续刷新：
  采用"收到 Rpc 事件即重绘 + 一个 16~50ms 的 poll tick 兜底"足够；不必精确帧率。
- 发送 prompt：主线程调用 `client.send_command(json!({"type":"prompt","message":...}), timeout,
  RequireSuccess)` 必须**另起短线程或非阻塞**，避免等待响应阻塞 UI；由于 prompt
  响应很快（受理），用一次性 `thread::spawn` 发送并忽略其响应即可（错误经通道回报）。
  abort 同理用一次性线程发，不阻塞渲染。

### 4. SessionState 只读扩展（修改既有能力）

新增（不改任何归约逻辑）：
- `pub fn in_flight(&self) -> &[Block]`：当前累积中的块（message_end 后为空）。
- `pub fn is_in_flight(&self) -> bool`：`!inflight.is_empty()`，等价便利方法。

这满足 tui-app"实时显示 delta"的需求；现有离线测试与 transcript 语义不变，
新增针对访问器的单测（delta 期间非空、落盘后清空、空闲为空）。

渲染优先读 `in_flight()` 显示"正在生成"，`entries()` 显示历史。

### 5. 视图映射（阶段 2 从简，保证不崩）

chat.rs 把状态映射成行片段，**纯函数**以便无 TTY 单测：
- `Entry::User { text }` → 一个带前缀的行（如 `> ` 或 "You"）。
- `Entry::Assistant { blocks }` / in-flight blocks：
  - `Block::Text` → 原文（先不做 markdown，长文按宽度软换行/截断由 ratatui Paragraph wrap 处理）。
  - `Block::Thinking` → 阶段 2 折叠为单行占位（如 `<thinking N chars>`），不展开。
  - `Block::ToolCall { name, tool, .. }` → 单行（`● read …` / 成功失败标记）。
- `Entry::Notice` → 单行弱化样式。
纯函数产物建议直接是 `Vec<Line>`（ratatui text 类型），测试断言其文本内容。

布局用 `Layout` 竖直三段：`[header 固定高1, body 剩余, editor 固定高1或2]`；
body 用 `Paragraph::wrap` + 一个简单的滚动偏移（阶段 2：自动滚到底；向上键
滚动可后置，最小实现先只跟随最新内容）。

### 6. 键位（对齐 pi 的最小子集，阶段 2）

| 按键 | 行为 |
|---|---|
| 可打印字符 | 追加到输入缓冲 |
| Backspace | 删除末尾 |
| Enter | 发送 prompt（空串不发） |
| esc（流式中） | 发 abort |
| esc/q（空闲） | 退出 |
| Ctrl-C | 退出（与 pi 的"清空"不同；阶段2先做安全退出，避免 raw 模式里无法离开） |

phase 来自 `SessionState`（Streaming/Idle），用它区分 esc 语义。

### 7. 启动/错误处理

main：创建 `TerminalGuard` → `RpcClient::spawn(resolve_binary(), &[])` →
`get_state`（15s, RequireSuccess）→ 进入 run。任何一步失败：
打印到 stderr（已恢复终端后）并以非零码退出。`PI_BIN` 复用 rpc 的解析。
不做登录 UI（路线图明确排除）。

## Risks / Trade-offs

- **[crossterm/ratatui 版本组合]** → 实现时用 `cargo add` 解析兼容版本并一次锁定；
  以 `cargo build`/clippy 为准，不手写猜测版本。
- **[panic 留在 alt screen]** → TerminalGuard 的 Drop + 安装 panic hook 恢复终端后再打印。
- **[阻塞 send_command 卡住渲染]** → 发送/abort 走一次性线程，UI 永不直接阻塞在 RPC。
- **[输出极长导致渲染卡顿/布局错乱]** → 阶段 2 用 wrap + 自动到底，不追求虚拟滚动；
  超长但不崩为验收线，性能优化留阶段 3。
- **[真实终端行为难自动化测试]** → 把状态→行、键位语义、访问器做成纯逻辑单测；
  终端 IO/视觉以手工真机验证清单为准（tasks 中列明）。
- **[stdin 线程在退出后阻塞]** → 线程设为 detached，进程退出即结束；UI 循环退出即恢复终端返回。

## Migration Plan

纯新增 + 库小扩展，无数据迁移。落地顺序：依赖与 TerminalGuard → SessionState
访问器与单测 → 事件/事件循环 → 视图纯函数与单测 → 接真实 pi 手工验收。
回滚删除 app 模块、还原 main.rs 即可，不影响 probe 与已归档能力。

## Open Questions

- crossterm/ratatui 最终锁定版本由实现期 `cargo add` 决定（不影响结构与规格）。
- body 向上滚动是否本变更就要：倾向仅"自动到底"，向上回看放到阶段 3。
