# pi-shell-rs

**用 Rust 重写的 pi 终端界面（TUI）** —— 在任意通用终端里运行，通过调用 `pi --mode rpc`（无头核心）驱动对话，界面由 Rust 自己绘制。目标是与原生 pi **体验一致**，而不是逐像素一致，并在此基础上方便做自己的定制。

**A terminal UI (TUI) for pi, rewritten in Rust.** It runs in any ordinary terminal (iTerm, xterm.js, the system console, …), drives the conversation through `pi --mode rpc` (the headless core), and renders the whole interface itself. The goal is an *experience-level* match with native pi — not pixel-perfect — and a solid base for our own customization.

---

## 主要功能 / Features

- 流式对话：消息、思考块（可折叠）、工具调用卡片、markdown 渲染与代码高亮
- 完整键位语义：流式中 Enter 打断重定向、follow-up 排队、`!命令` 直接跑 bash 等
- 会话管理：`/resume` 会话列表、`/tree` 会话树（搜索 / 过滤 / 分支）、`/new` `/fork` `/clone` 等
- 斜杠命令与补全菜单、`@文件` 补全、外部编辑器（Ctrl+G）、剪贴板（Ctrl+X / Ctrl+V）
- 模型 / thinking 级别切换，token 与费用显示

- Streaming chat: messages, collapsible thinking blocks, tool-call cards, markdown rendering with syntax highlighting
- Native keybinding semantics: Enter steers while streaming, follow-up queue, `!command` runs bash directly, and more
- Session management: `/resume` list, `/tree` (search / filter / branch), `/new` `/fork` `/clone`, etc.
- Slash commands with a completion menu, `@file` completion, external editor (Ctrl+G), clipboard (Ctrl+X / Ctrl+V)
- Model / thinking-level switching, token & cost display

## 与原生 pi TUI 的差异 / What's different from native pi

大部分日常功能都已对齐。以下是**有意不支持或简化**的部分，以及原因。总体原则：**不改动 pi 核心**（只通过公开的 RPC 对话），凡是需要 pi 内部能力、网络上报、或超出"体验一致"目标的功能，都会砍掉或简化。

Most everyday features match native pi. The items below are **deliberately unsupported or simplified**, with the reason. The guiding principle: **never modify pi's core** (we only talk to it over its public RPC). Anything that would require pi internals, network reporting, or goes beyond "experience-level parity" gets cut or simplified.

| 功能 / Feature | 状态 / Status | 原因 / Why |
|---|---|---|
| `/login` `/logout` 交互式登录 | ❌ 不支持 | 需要走网络登录流程。改为依赖原生 pi 预登录一次（`~/.pi/agent/auth.json`），之后直接使用。<br>Interactive login requires a web auth flow. Log in once with native pi instead. |
| `/share` `/bug` | ❌ 桩提示 | 需要向外部服务上报（分享 / 反馈），超出范围。<br>They report to external services (sharing / feedback) — out of scope. |
| 完整 `/settings` 设置页 | ⚠️ 简化 | 只保留少数开关（自动压缩、自动重试、双击 Esc 行为）。完整设置需要读写 pi 的全部配置，RPC 没有对应接口。<br>Only a few toggles remain (auto-compact, auto-retry, double-Esc). Full settings need pi's complete config read/write, which RPC doesn't expose. |
| `/scoped-models` | ⚠️ 只读 | 可以查看但不能勾选启用/禁用模型，RPC 无写入方法。<br>View-only; RPC has no method to enable/disable models. |
| 会话树内编辑消息标签（Shift+L） | ❌ 不做 | 标签只能经 pi 扩展 API 写入，RPC 没有这个入口；pi 自身的这颗键也是未接线的。<br>Labels can only be written via pi's extension API (no RPC entry), and even pi's own key for this is unwired. |
| 扩展快捷键、扩展参数补全 | ❌ 不支持 | 需要 pi 内部的扩展运行时配合，RPC 不提供。<br>Needs pi's internal extension runtime; RPC doesn't provide it. |
| 图片渲染（kitty / sixel） | ❌ 未做 | 属于"像素级复刻"的重特性，按需后置。<br>A heavy "pixel-parity" feature, deferred. |
| Windows / Linux 专属键位 | ❌ 未做 | 目前只按 macOS 键位实现（个人项目，按需再补）。<br>Only macOS keybindings are implemented for now. |
| 复制选区、粘贴文件列表 | ❌ 不支持 | 本 TUI 没有文本选区概念；剪贴板只做"图片 → 路径 / 文本"链。<br>This TUI has no text selection; clipboard handles only the image→path / text chain. |
| `@路径` 解析为附件 | ⚠️ 略有差异 | 在消息里按纯文本送入上下文（pi 会解析成附件）。<br>Sent as plain text into context (pi would resolve it as an attachment). |
| `/copy` 剪贴板 | ⚠️ 略有差异 | 走终端的 OSC52 协议；终端禁用 OSC52 时无法复制。<br>Uses the terminal's OSC52 protocol; won't work if the terminal disables it. |

此外还有一些小的简化（如部分选择器不支持 PageUp/PageDown 分页、`/export` 无参默认导出 JSONL 而非 HTML 等），细节见 [`Docs/`](Docs/) 下的文档。

There are also a few small simplifications (some pickers lack PageUp/PageDown paging, `/export` defaults to JSONL instead of HTML, etc.). See the docs under [`Docs/`](Docs/) for details.

## 快速开始 / Quick start

```bash
# 需要先用原生 pi 登录一次（生成 ~/.pi/agent/auth.json）
# Log in once with native pi first (creates ~/.pi/agent/auth.json)

cargo run                     # 启动 TUI / start the TUI
PI_BIN=/path/to/pi cargo run  # 指定 pi 可执行文件 / custom pi binary
```

需要本机已安装 `pi` 命令行工具。更多用法（键位表、bash 模式、外部编辑器等）见 [`Docs/tui-usage.md`](Docs/tui-usage.md)。

Requires the `pi` CLI to be installed. For keybindings, bash mode, external editor, etc., see [`Docs/tui-usage.md`](Docs/tui-usage.md).

## 文档 / Docs

| 文档 / Doc | 内容 / Contents |
|---|---|
| [`Docs/roadmap.md`](Docs/roadmap.md) | 产品定位与路线图 / positioning & roadmap |
| [`Docs/tui-usage.md`](Docs/tui-usage.md) | 使用说明与键位 / usage & keybindings |
| [`Docs/slash-commands.md`](Docs/slash-commands.md) | 斜杠命令对照表 / slash command reference |
| [`Docs/pi-tui-interaction.md`](Docs/pi-tui-interaction.md) | 与原生 pi 的交互对照 / interaction diff vs native pi |
