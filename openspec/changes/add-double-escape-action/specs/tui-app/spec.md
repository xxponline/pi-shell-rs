# Spec Delta

## ADDED Requirements

### Requirement: 双击 Esc 动作

空闲（非流式）且输入框为空时，500ms 内第二次 Esc MUST 按 `doubleEscapeAction` 设置触发动作：`tree`（默认）打开会话树浮层、`fork` 打开分支入口（本 TUI 的 fork 与 `/fork` 同为树入口）、`none` 无操作。设置 MUST 自全局 `~/.pi/agent/settings.json` 读取（缺失或非法值回落 `tree`）。流式 Esc（中止）、浮层与补全菜单的 Esc（关闭）优先于双击判定，且这些 Esc **不计入**双击窗口；输入非空时双击不触发（与 pi 一致）。

#### Scenario: 双击打开会话树

- **WHEN** 空闲且输入为空，用户在 500ms 内连按两次 Esc
- **THEN** 第二次 Esc 打开会话树浮层；此前单次 Esc 无任何可见效果

#### Scenario: 超时与非空输入不触发

- **WHEN** 两次 Esc 间隔超过 500ms，或输入框有文本时双击 Esc
- **THEN** 不打开任何浮层，输入与界面保持原样

#### Scenario: none 与优先级

- **WHEN** 设置为 `none` 时双击 Esc，或流式中/浮层打开时按 Esc
- **THEN** none 无动作；流式 Esc 仍中止生成、浮层 Esc 仍关闭浮层，均不进入双击判定

## MODIFIED Requirements

### Requirement: 简化设置与模型范围展示

`/settings` MUST 打开简化设置浮层：提供 autoCompact 与 autoRetry 两个开关（分别经 `set_auto_compaction`、`set_auto_retry` 应用并即时生效）、`doubleEsc` 动作行（Enter 在 `tree → fork → none` 循环并写回全局 `~/.pi/agent/settings.json` 的 `doubleEscapeAction`，保留文件内其它键、原子写，立即对双击生效）以及当前默认模型等配置的只读展示，esc 关闭；`/scoped-models` MUST 打开只读浮层展示当前会话模型与全部可用模型（无写入能力）。两者均为对 pi 完整设置界面的简化，行为差异 MUST 记录在 `Docs/slash-commands.md`。

#### Scenario: 简化设置开关

- **WHEN** 用户输入 /settings 回车，在浮层中切换 autoCompact 开关
- **THEN** 发送 set_auto_compaction，开关状态更新，esc 关闭浮层

#### Scenario: doubleEsc 动作循环并写回

- **WHEN** 在设置浮层对 doubleEsc 行按 Enter（当前为 tree）
- **THEN** 行值变为 fork 并写入 settings.json（其它键保持不变），再次 Enter 变为 none，循环回到 tree

#### Scenario: 只读模型范围

- **WHEN** 用户输入 /scoped-models 回车
- **THEN** 出现只读浮层列出当前模型与可用模型列表，esc 关闭，不发送任何写入命令
