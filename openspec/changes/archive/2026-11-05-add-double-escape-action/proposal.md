# Proposal

## Why

差异表 #5：pi 支持"空输入下 500ms 内双击 Esc"按设置打开会话树/分支选择器（默认 tree），这是高频的免输入导航捷径；我们的 TUI 目前空闲 Esc 是纯无操作（对齐 pi 单击语义），缺少该双击通路。4b 已落地会话树，依赖已就绪。

## What Changes

- **双击 Esc 动作**：空闲且输入为空时，500ms 内第二次 Esc 按 `doubleEscapeAction` 设置触发——`tree`（默认）打开会话树、`fork` 打开分支入口（本 TUI 的 fork = 树，与 `/fork` 同一简化）、`none` 无操作；流式 Esc（中止）、浮层/补全菜单的 Esc（关闭）优先且**不计入**双击窗口
- **设置读写**：动作从全局 `~/.pi/agent/settings.json` 的 `doubleEscapeAction` 读取（缺省 `tree`，非法值回落 tree）；`/settings` 简化浮层新增一行 `doubleEsc`，Enter 在 `tree → fork → none` 循环（对齐 pi 设置选择器的取值序）并写回该文件（保留其它键、原子写）
- **文档**：`Docs/slash-commands.md` settings 行更新、`Docs/tui-usage.md` 键位表补双击 Esc、`Docs/pi-tui-interaction.md` 差异 #5 标记完成

## Capabilities

### New Capabilities

（无——行为归属既有交互能力）

### Modified Capabilities

- `tui-app`: ADDED"双击 Esc 动作"需求（触发条件/窗口/设置三态/优先级）；MODIFIED"简化设置与模型范围展示"需求（新增 doubleEsc 循环行与 settings.json 写回）

## Impact

- `src/app/mod.rs`：空闲 Esc 分支接入 500ms 双击判定（`last_escape` 时间戳）+ 设置读取缓存
- `src/app/overlay.rs`：`Overlay::Settings` 增加 doubleEsc 行（光标行数 1→2，写回后即时生效）
- `src/app/sessions.rs`（或新 helper）：`read_double_escape_action()` / `write_double_escape_action()`——读改写 `~/.pi/agent/settings.json`（serde_json 保键、tmp+rename 原子写）
- 测试：双击窗口/三态动作/优先级单测 + 假 pi PTY；设置读写 roundtrip 单测（临时 HOME/文件路径注入）
- 不改 RPC 协议、不改 session 状态机
