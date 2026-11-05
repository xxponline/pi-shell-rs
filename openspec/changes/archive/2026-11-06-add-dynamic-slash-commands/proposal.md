# Proposal

## Why

研究（pi 源码）确认：扩展命令、skill 命令（`/skill:name`）、提示模板的**执行全部发生在
pi 的 `prompt()` 内部**——我们把未知 `/xxx` 当 prompt 发送时 pi 已会拦截执行，但 TUI 侧
存在三个体验缺口：补全菜单看不见这些动态命令、压缩期把扩展命令误排队（pi 的扩展命令
绕过压缩检查立即执行）、`enableSkillCommands=false` 时仍会列出 skill。本变更补齐动态
命令的发现与语义对齐。

## What Changes

- **动态命令发现**：启动时（及 `/reload`）调用 `get_commands` RPC 缓存
  `{name, description, source: extension|prompt|skill, sourceInfo}`，`/` 补全候选 =
  内置 24 + 动态命令
- **补全对齐 pi**：扩展名与内置冲突时**过滤扩展**（pi 同规则）；描述加源标签
  （`[extension]` / `[prompt]` / `[skill]`，对照 pi `prefixAutocompleteDescription`）；
  `enableSkillCommands` 为 false 时过滤 `skill:` 候选（读全局 settings.json，默认 true）
- **分发语义对齐 pi**：提交命中**动态扩展命令**时压缩期也**立即发送 prompt**
  （pi 的 `_tryExecuteExtensionCommand` 先于压缩检查——本地队列不能拦它）；
  skill/模板命令压缩期仍走本地排队（pi 的展开在压缩检查之后，直接发会被拒绝）；
  流式路径不变（steer prompt 到 pi 后扩展先执行）
- **文档**：`Docs/slash-commands.md` 新增"动态命令（扩展/skill/模板）"一节、
  tui-usage 补充说明

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `tui-app`: MODIFIED"斜杠命令分发"（动态命令命中规则与压缩期立即发送语义）；
  ADDED"动态斜杠命令补全"（get_commands 缓存、合并、冲突过滤、源标签、skill 开关过滤）
- `session-management`: MODIFIED"手动 compact 流程"（压缩期排队增加扩展命令例外）

## Impact

- `src/app/mod.rs`：启动/`/reload` 拉 `get_commands` 缓存（`AsyncKind::Commands`）；
  `submit_as` 压缩门控前增加"动态扩展命令立即发送"分支（复用 pi 的 source 分类）
- `src/app/autocomplete.rs`：候选合并 + 冲突过滤 + 源标签（纯逻辑可单测）
- `src/app/settings_io.rs`：`enableSkillCommands` 读取（同一 settings.json）
- `src/app/commands.rs`：动态命令名查询入口（分发判定用）
- 文档三处；不改 RPC 协议（`get_commands` 已存在）
