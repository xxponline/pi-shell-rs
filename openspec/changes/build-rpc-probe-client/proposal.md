# Proposal

## Why

Rust TUI 客户端的所有渲染决策都依赖 pi-agent RPC 模式实际输出的 JSONL 事件流。`Docs/rpc-protocol-notes.md` 已从类型定义整理出完整协议参考，但第 8 节列出了 8 个只有真正跑一遍才能回答的缺口（例如 `message_start` 的初始形状、`toolcall_start` 展平后的 wire 格式、`agent_end.messages` 是否全量、stderr 是否混入 stdout 等）。在写 Rust 事件循环与渲染状态机之前，必须先用一个最小 client 把真实事件流抓下来、按类型归档，避免基于猜测建模。

## What Changes

- 新增一个**独立、一次性/可重跑的 RPC 探针子模块**（放在 `src/bin/probe/`，作为 cargo bin target），不改动未来主 TUI 二进制的代码路径。
- 探针负责：
  - spawn `pi-agent --mode rpc`，建立 stdin/stdout JSONL 通道，stderr 单独转发到文件。
  - 提供一个**场景脚本**机制，按顺序发送一组 command（如 `get_state` → `prompt` → 等待 `agent_settled`）。
  - 给每条 stdin command 生成 id，把 stdout 每一行（含时间戳、方向、原始 JSON）落到 fixtures 目录。
  - 按顶层 `type`（`response` / 各事件 / `extension_ui_request` / `extension_error`）分类统计，生成一份 run summary。
- 内置一组覆盖关键路径的场景（对应 `rpc-protocol-notes.md` 第 9 节）：
  1. 纯文本对话
  2. 触发工具调用（read/bash 类）
  3. thinking 模式下的流式 delta
  4. 生成中 `steer`
  5. 排队 follow_up
  6. 手动 `compact`
  7. 失败工具调用
  8. 扩展 UI 请求（需要一个最小测试扩展配合，见 design）
- 把抓到的原始 JSONL 作为 **fixture 归档**，供后续 Rust 渲染层作为测试输入。
- 更新 `Docs/rpc-protocol-notes.md` 第 8 节：对每个待验证问题标注"已验证 / 与类型定义不符（附 fixture 引用）"。
- 明确**不在本变更范围**：不实现任何 TUI 渲染、不引入 ratatui/crossterm、不实现完整 RPC SDK（只做探针所需的最小行读写与场景调度）。

## Capabilities

### New Capabilities

- `rpc-probe`: 通过脚本化场景驱动 `pi-agent --mode rpc`，抓取、分类、归档真实 JSONL 事件流的开发期诊断能力。fixture 产物是 Rust TUI 事件循环建模的事实依据。

### Modified Capabilities

（无——当前 specs 目录为空，本变更是首个 capability。）

## Impact

- **代码**：新增 `src/bin/probe/main.rs`（或 `probe/` 子模块）、`Cargo.toml` 增加第二个 bin target 与少量依赖（`serde` / `serde_json` / `tokio` 或 std 线程，具体选型见 design）。主库代码暂不增加。
- **fixtures**：新增 `tests/fixtures/probe/<scenario>/`（原始 JSONL + summary.json），`.gitignore` 不排除但可能体积偏大，需要在 design 里定归档策略（是否截断 tool result 大字段）。
- **文档**：回填 `Docs/rpc-protocol-notes.md` 第 8 节。
- **运行时依赖**：本机已安装的 `pi-agent` 可执行文件，且已提前完成 `~/.pi/agent/auth.json` 登录。探针不做交互式 auth。
- **测试扩展**：场景 8 需要一个最小 TS 扩展调用 `ctx.ui.select`，放在 `probe/fixture-extensions/`（只用于本地探针运行，不安装到 `~/.pi/agent/extensions/`）。
- **无破坏性变更**：尚未发布任何能力。
