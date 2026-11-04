# TUI 使用说明（阶段 2：最小可见界面）

路线图阶段 2 的产物：用 ratatui 在通用终端里复刻 pi 的最小交互闭环。
协议与库层见 [`rpc-protocol-notes.md`](./rpc-protocol-notes.md)、
[`library-and-replay.md`](./library-and-replay.md)。

## 前置条件

- 本机已安装 `pi`（实测 v1.5.0），并**用原生 pi 完成过登录**
  （`~/.pi/agent/auth.json`）。本 TUI 不做交互式登录。
- 必须在**真实交互终端**里运行（需要 raw mode；非 TTY 会报
  "无法进入终端 raw/alt-screen 模式"并退出）。

## 启动

```bash
cargo run                       # 默认运行 TUI（default-run）
# 或指定：cargo run --bin pi-rust-tui
# 或先构建：cargo build && ./target/debug/pi-rust-tui
```

- `pi` 不在 PATH 时：`PI_BIN=/path/to/pi cargo run`
- 启动流程：进入 TUI → 拉起 `pi --mode rpc` → `get_state` 探活（15s）→
  进入界面。任何一步失败都会**恢复终端**后打印可读原因并以非零码退出。

## 界面布局

```
○ idle  session-name  thinking:off     ← 状态行（流式时显示 ● streaming）
┌────────────────────────────────────┐
│ You: 你的消息                       │  滚动消息区（自动跟随最新内容）
│ assistant 回复原文（纯文本）          │
│ <thinking N chars>                  │  思考块占位（阶段2不展开）
│ ● read Cargo.toml · done           │  工具调用单行状态（红=error 黄=running 绿=done）
└────────────────────────────────────┘
┌ input (enter: send · q/esc: quit) ┐  输入框
│ 输入的文本                         │
└───────────────────────────────────┘
```

## 键位

| 按键 | 行为 |
|---|---|
| 可打印字符 / 退格 | 编辑输入（阶段 2：单行语义，超长会自动换行显示） |
| Enter | 发送 prompt（空/全空白不发送）；发送后输入框清空 |
| Esc（流式中） | 发送 abort 中断生成，回到空闲 |
| Esc 或 q（空闲时） | 退出 |
| Ctrl-C | 随时安全退出（恢复终端） |

流式期间按 q **不会**退出（会作为文本进入输入框），与 pi 的"esc 中断优先"语义一致。

## 与原生 pi 的已知差异（阶段 2 预期）

- 回复是**纯文本**渲染，没有 markdown 语法高亮（阶段 3）。
- thinking 折叠、工具结果卡片、follow_up/steer 队列、模型/会话选择器
  尚未实现（阶段 3/4）。
- 消息区只自动跟随最新内容，暂不支持向上滚动回看。

## 排查

- **启动报"无法启动 pi"**：检查 `which pi` / `PI_BIN`，或先用原生 pi 登录。
- **报"pi 探活失败 (get_state)"**：模型/网络未就绪，先在原生 pi 里跑通一次。
- 退出后终端乱码/卡 raw mode：Ctrl-C 或 Ctrl-\ 退出；正常路径与 panic
  都会自动恢复（alt screen + 光标 + canonical mode）。
