# Spec Delta

## REMOVED Requirements

### Requirement: 菜单应用与键位
**Reason**: 回车语义反转（BREAKING：`@` 菜单回车由"应用并提交"改为"只应用不提交"），且 openspec 不支持场景改名，场景"回车应用并提交"必须更名，故整条需求退役重立。
**Migration**: 行为由新增需求"菜单键位与应用"承接；除回车改为只应用不提交、主列改按 label 展示外，与原需求一致。

## ADDED Requirements

### Requirement: 菜单键位与应用

菜单键位 MUST 复用 `slash-autocomplete` 的菜单约定（↑↓ 移动选中、Tab 应用、Esc 只关菜单、继续输入即时重过滤）；回车 MUST 只应用选中项并关闭菜单、MUST NOT 提交该行（对齐 pi：仅斜杠菜单回车 fall-through 到提交）。应用时 MUST 保留 `@` 前缀：选中**文件**补一个空格结束 token；选中**目录**不补空格且光标置于目录名后（可继续下钻）；`~/`、`./`、绝对路径写法 MUST 保留；含路径分隔符的候选 MUST 自动包上 `@"..."` 引号；光标落位于插入文本之后。应用后 MUST 关闭菜单。菜单主列按 label 展示的行为见 `path-completion` 的"菜单主列展示"。

#### Scenario: 选择文件应用

- **WHEN** 菜单选中 `src/main.rs` 按 Tab
- **THEN** 输入变为 `@src/main.rs `（文件后带空格），光标在空格后，菜单关闭

#### Scenario: 选择目录续写

- **WHEN** 菜单选中目录 `src/` 按 Tab
- **THEN** 输入变为 `@src/`（无空格），光标在 `/` 后，菜单关闭，可继续输入子路径

#### Scenario: 回车只应用不提交

- **WHEN** `@` 菜单打开且回车被按下
- **THEN** 选中候选应用到输入框、菜单关闭，该行不被提交，输入框保留文本等待用户再次回车
