# Proposal

## Why

装了 `pi-token-speed` 扩展后，状态栏 TPS 显示为乱码：扩展经 `setStatus` 上报的 `statusText` 内嵌 ANSI 转义序列（theme SGR + 24-bit 真彩），而 ratatui 渲染路径只滤掉 `0x1b` 字节、不解析序列，`[38;2;...m` 等可打印残留直接可见。同时 `handle_extension_request` 丢弃了协议字段 `statusKey`，多扩展状态互相覆盖、任一扩展清空会清掉全部——pi 的语义是按 key 分格并存。两处都在同一个 setStatus 处理分支上，一并修复。

## What Changes

- 新增最小 SGR 解析器（纯函数）：把带 ANSI 的字符串转为 `Vec<Span>`，覆盖插件实际产出的子集——`38/48;2;r;g;b` 真彩、`38/48;5;n` 256 色、标准 8/16 色、bold/dim/italic/underline 及各 reset、`0` 全复位；未知 CSI/OSC/APC 序列按 pi `ansiCodeLength` 的边界语义跳过不显示。
- 状态存储由单槽 `Option<String>` 改为按 `statusKey` 分格的有序映射（`BTreeMap<String, String>`），对齐 pi：多扩展并存、按 key 字母序拼接、`statusText: null` 只清指定 key。
- 状态行状态段改为"解析后的 spans 序列"渲染，保留扩展设置的颜色（不再整段套洋红）。
- 状态段截断改为按剥离转义后的**可见文本**宽度计算（现有 `truncate` 按字符数把转义残留也算进去，会提前截断）；清理序列残留的零宽空格 `​`。
- 无 `statusKey` 字段的请求（旧探针 fixture、简易客户端）落入默认格，行为与现状一致（不报错）。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `tui-app`: 「extension UI 被动方法」需求中 setStatus 的语义细化——从"更新状态行"扩展为按 `statusKey` 分格管理、状态文本含 ANSI 时 MUST 解析渲染而非显示转义残留；并新增对应回归场景。

## Impact

- 代码：`src/app/mod.rs`（`handle_extension_request` 的 `setStatus` 分支、`status_text` 字段、`status_grid_line`）、`src/app/view.rs`（`header_line` 状态段装配与截断）、新增 SGR 解析纯函数模块（随附无 TTY 单测）。
- 前端：ratatui 与 GPUI 双前端同源受益（均经 `view::header_line`）。
- 依赖：不新增 crate，手写子集解析器。
- 协议：不改 RPC 收发，只改消费端；`Docs/rpc-protocol-notes.md` 的 setStatus 字段表已有 `statusKey`，无需变更。
- 规格：`openspec/specs/tui-app/spec.md` 的「extension UI 被动方法」需求随归档同步更新。
