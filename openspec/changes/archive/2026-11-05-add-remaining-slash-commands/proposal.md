# Proposal

## Why

4b 已建斜杠命令设施并落地 9 个会话命令，但 pi 的 24 个内置命令还有 15 个未实现，且输入 `/` 后没有任何补全提示——用户必须背命令名，与 pi 的体验级一致目标差距明显（差异表 #7）。本变更补齐剩余命令（按方案 A：可行项真实现、settings/scoped-models 简化、网络与登录类出提示桩）并完整复刻 pi 的斜杠补全菜单。

## What Changes

- **斜杠补全菜单（新）**：输入 `/` 即弹出候选列表，模糊过滤命令名 + 描述列（`参数提示 — 说明`）；↑↓/PageUp/PageDown 导航、Esc 只关菜单、Tab 应用补全、Enter 应用补全并直接提交、自动预选最佳匹配（精确 > 前缀首项）、最多 5 条可滚动（`(i/n)` 计数）；`/model `、`/thinking ` 有参数补全
- **命令集 9 → 24**：分发表补齐全部内置命令，命中的 `/xxx` 仍拦截清空输入、未命中的照常发模型
- **真实现（9 个）**：
  - `/model [搜索]`：无参开选择器；精确匹配直接 `set_model` 并提示；无匹配回退选择器
  - `/thinking [级别]`：无参开级别列表；精确匹配直接应用；未知级别红条列出可用项
  - `/copy`：`get_last_assistant_text` → 剪贴板（OSC52）
  - `/export [路径]`：`.jsonl` 本地复制会话文件；`.html` 走 `export_html` RPC；无参默认 jsonl
  - `/import <路径.jsonl>`：确认后复制进当前项目会话目录并 `switch_session`
  - `/changelog`：读 pi 包内 CHANGELOG.md，渲染进消息区
  - `/hotkeys`：键位表浮层
  - `/trust`：写 `~/.pi/agent/trust.json` 信任当前项目
  - `/reload`：本地配置重读 + 界面重绘提示
- **简化（2 个）**：`/settings`（仅 autoCompact/autoRetry 开关 + 当前配置只读展示，RPC 无全量 settings 方法）；`/scoped-models`（只读展示当前/可用模型，RPC 无 scoped models 方法）
- **提示桩（4 个）**：`/share`、`/bug`（网络上报）、`/login`、`/logout`（项目约束"不做交互式登录"）——执行后黄条提示原因，不发给模型
- **交付文档**：`Docs/slash-commands.md`——24 个命令的实现/简化/桩状态、行为差异与原因

## Capabilities

### New Capabilities

- `slash-autocomplete`: 斜杠命令补全菜单的触发、候选过滤、渲染、键位与应用行为

### Modified Capabilities

- `tui-app`: 斜杠命令分发表扩展至 24 个（含桩提示）；model/thinking 选择器增加斜杠入口；新增 copy/changelog/hotkeys/trust/reload 与简化 settings/scoped-models 的行为需求
- `session-management`: 新增会话导出与导入（/export、/import）需求

## Impact

- `src/app/commands.rs`：命令表 9 → 24（含 description/argumentHint 元数据），新增桩臂
- `src/app/editor.rs` + `view.rs`：补全菜单状态（触发/过滤/选中/滚动）与输入区下方渲染
- `src/app/overlay.rs` + `mod.rs`：hotkeys/settings/scoped-models 浮层，export/import/trust 异步链
- `src/rpc/client.rs`：复用已有 `export_html`、`get_last_assistant_text`（无需新 RPC）
- 新增本地文件操作：会话 jsonl 复制、`trust.json` 读写、CHANGELOG.md 定位
- `Docs/slash-commands.md`（新）、`Docs/tui-usage.md`、`Docs/pi-tui-interaction.md`（差异表 #7 收尾）
- 不改 RPC 协议、不改 session 状态机归约
