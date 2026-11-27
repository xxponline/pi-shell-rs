# pi-shell-rs

**用 Rust 重写的 pi 终端界面（TUI）** —— 在任意通用终端里运行，通过调用 `pi --mode rpc`（无头核心）驱动对话，界面由 Rust 自己绘制。目标是与原生 pi **体验一致**，而不是逐像素一致，并在此基础上方便做自己的定制。

**A terminal UI (TUI) for pi, rewritten in Rust.** It runs in any ordinary terminal (iTerm, xterm.js, the system console, …), drives the conversation through `pi --mode rpc` (the headless core), and renders the whole interface itself. The goal is an *experience-level* match with native pi — not pixel-perfect — and a solid base for our own customization.

---

## 主要功能 / Features

- 流式对话：消息、思考块（可折叠、单块展开）、工具调用卡片（单块展开、read/write 按语言语法高亮、edit 卡执行前红绿 diff 预览）、markdown 渲染与代码高亮
- 鼠标与滚动：消息区滚动条（悬停显隐 / 拖拽定位）、滚轮翻页、拖选文本 + 双击选词/三击选行 + 选中自动复制、点击工具卡/思考块展开
- 完整键位语义：流式中 Enter 打断重定向、follow-up 排队、`!命令` 直接跑 bash、`ctrl+↑/↓` 提示跳转、`shift+PageUp/Dn` 半页等
- 输入编辑：kill-ring（`ctrl+y` / `alt+y`）、undo（`ctrl+-`）、↑/↓ 浏览本会话提交历史（草稿可还原）
- 会话管理：`/resume` 会话列表、`/tree` 会话树（搜索 / 过滤 / 分支）、`/new` `/fork` `/clone` 等
- 扩展 UI：select/confirm/input/editor 对话浮层；被动方法 notify、状态栏 statusKey 分格（ANSI 真彩解析、零转义残留）、widget、窗口标题
- 斜杠命令与补全菜单、`@文件` 补全、路径 Tab 补全、外部编辑器（Ctrl+G）、剪贴板（Ctrl+X / Ctrl+V、OSC52）
- 模型 / thinking 级别切换，token 与费用显示
- GPUI 独立窗口（可选 feature `gpui-support`）：与 TUI 同源的窗口形态、悬浮层、斜杠命令全集、light/dark/system 三模式主题

- Streaming chat: messages, collapsible & per-block-expandable thinking, tool-call cards (expandable, read/write syntax-highlighted by language, edit card pre-execution red/green diff preview), markdown rendering with syntax highlighting
- Mouse & scrolling: message scrollbar (hover / drag-to-scrub), wheel paging, drag-to-select with double-click word / triple-click line and copy-on-select, click tool cards / thinking blocks to expand
- Native keybinding semantics: Enter steers while streaming, follow-up queue, `!command` runs bash directly, `ctrl+↑/↓` prompt jumps, `shift+PageUp/Dn` half-page, and more
- Input editing: kill-ring (`ctrl+y` / `alt+y`), undo (`ctrl+-`), ↑/↓ browse this session's submission history (draft restored on exit)
- Session management: `/resume` list, `/tree` (search / filter / branch), `/new` `/fork` `/clone`, etc.
- Extension UI: select/confirm/input/editor dialog overlays; passive methods — notify, keyed status slots (ANSI truecolor parsing, zero escape residue), widget, window title
- Slash commands with a completion menu, `@file` completion, path Tab completion, external editor (Ctrl+G), clipboard (Ctrl+X / Ctrl+V, OSC52)
- Model / thinking-level switching, token & cost display
- Standalone GPUI window (optional `gpui-support` feature): window form, overlays, full slash-command set, light/dark/system themes — all same-source with the TUI

## 与原生 pi TUI 的差异 / What's different from native pi

大部分日常功能都已对齐。以下是**有意不支持或简化**的部分，以及原因。总体原则：**不改动 pi 核心**（只通过公开的 RPC 对话），凡是需要 pi 内部能力、网络上报、或超出"体验一致"目标的功能，都会砍掉或简化。

Most everyday features match native pi. The items below are **deliberately unsupported or simplified**, with the reason. The guiding principle: **never modify pi's core** (we only talk to it over its public RPC). Anything that would require pi internals, network reporting, or goes beyond "experience-level parity" gets cut or simplified.

| 功能 / Feature | 状态 / Status | 原因 / Why |
|---|---|---|
| `/login` `/logout` 交互式登录 | ❌ 不支持 | 需要走网络登录流程。改为依赖原生 pi 预登录一次（`~/.pi/agent/auth.json`），之后直接使用。<br>Interactive login requires a web auth flow. Log in once with native pi instead. |
| `/share` `/bug` | ❌ 桩提示 | 需要向外部服务上报（分享 / 反馈），超出范围。<br>They report to external services (sharing / feedback) — out of scope. |
| 完整 `/settings` 设置页 | ⚠️ 简化 | 只保留少数开关（自动压缩、自动重试、双击 Esc 动作）与默认模型等只读展示。完整设置需要读写 pi 的全部配置，RPC 没有对应接口。<br>Only a few toggles remain (auto-compact, auto-retry, double-Esc action) plus read-only display of the default model. Full settings need pi's complete config read/write, which RPC doesn't expose. |
| `/scoped-models` | ⚠️ 只读 | 可以查看但不能勾选启用/禁用模型，RPC 无写入方法。<br>View-only; RPC has no method to enable/disable models. |
| 会话树内编辑消息标签（Shift+L） | ❌ 不做 | 标签只能经 pi 扩展 API 写入，RPC 没有这个入口；pi 自身的这颗键也是未接线的。<br>Labels can only be written via pi's extension API (no RPC entry), and even pi's own key for this is unwired. |
| 扩展快捷键、扩展参数补全 | ❌ 不支持 | 需要 pi 内部的扩展运行时配合，RPC 不提供。<br>Needs pi's internal extension runtime; RPC doesn't provide it. |
| 图片渲染（kitty / sixel） | ❌ 未做 | 属于"像素级复刻"的重特性，按需后置。<br>A heavy "pixel-parity" feature, deferred. |
| 消息区文本搜索 | ❌ 未做 | 原生 pi 的 alt-screen 搜索（`ctrl+shift+f`）已列为挂账，按需后置。<br>Native pi's alt-screen search (ctrl+shift+f) is tracked as deferred. |
| Windows / Linux 专属键位 | ❌ 未做 | 目前只按 macOS 键位实现（个人项目，按需再补）。<br>Only macOS keybindings are implemented for now. |
| 右键粘贴文件列表 | ❌ 不支持 | 无右键粘贴入口；文本选区已对齐（拖选 / 双击词 / 三击行，选中自动复制）。<br>No right-click paste entry; text selection now matches (drag / double-click word / triple-click line, copy-on-select). |
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

# 可选：GPUI 独立窗口（仅特性构建存在）
# Optional: standalone GPUI window (feature-gated build)
cargo run --bin pi-shell-gpui --features gpui-support
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
| [`Docs/rpc-protocol-notes.md`](Docs/rpc-protocol-notes.md) | RPC 协议实测笔记 / RPC protocol field notes |
| [`Docs/library-and-replay.md`](Docs/library-and-replay.md) | rpc/session 库模块 API 与离线回放 / library API & offline replay |
| [`Docs/probe-usage.md`](Docs/probe-usage.md) | 协议探针用法 / protocol probe usage |
| [`openspec/specs/`](openspec/specs/) | 持久能力规格（9 项）/ durable capability specs |
