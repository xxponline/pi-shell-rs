# Spec Delta

## ADDED Requirements

### Requirement: edit 执行前预览

edit 工具卡 MUST 在参数齐全且结果未到达（进行中）时先行展示执行前预览：
app 层异步读取目标文件、按 `edits`（或 `oldText`/`newText`）计算与结果同源
的红绿 diff（含行内高亮）先行渲染；结果到达后 MUST 以 `details.diff`
替换预览；文件不存在或 oldText 不匹配 MUST 以错误态呈现（红底头行 +
错误文案，不 panic）；会话切换/整体重建 MUST 清空预览态。预览计算 MUST
NOT 阻塞事件循环，MUST NOT 引入框架的文件级 diff 查看组件。

#### Scenario: 先行展示与结果替换
- **WHEN** edit 调用参数齐全、结果未到，随后结果携带 details.diff 到达
- **THEN** 进行中先显示预览 diff（红绿 + 行内高亮），结果到达后卡片以结果 diff 呈现，无重复/残留

#### Scenario: 预览错误态
- **WHEN** 目标文件不存在或 oldText 在文件中不匹配
- **THEN** 卡头呈错误态（红底）与错误文案，结果到达后按结果正常渲染，全程不 panic

#### Scenario: 会话重建清空
- **WHEN** 切换会话或树导航导致 transcript 整体重建
- **THEN** 此前的预览态全部清空，不串到新会话的卡片上

## MODIFIED Requirements

### Requirement: edit 卡 diff 渲染
edit 工具卡 MUST 以红绿行文本呈现 diff：`-` 前缀旧行红色、`+` 前缀新行绿色、`@@` 行青色（结果无 diff 字段时按 old/new 行级对比降级）。恰 1 行改 1 行（连续 `-` 块与 `+` 块各恰好一行）时 MUST 对该对行做词级行内分段：共同前导缩进不染、首个 removed/added 段的前导空白保留不染、变更词以反视频高亮（行色保持红/绿）；多行块 MUST 维持整行着色；降级路径产出单行修改对时 MUST 同样分段。渲染 MUST 使用内建文本样式，MUST NOT 引入框架的文件级 diff 查看组件。

#### Scenario: 结果含 diff
- **WHEN** edit 结果 details.diff 存在
- **THEN** 卡体逐行按前缀着色呈现，界面不引入独立滚动的 diff 面板

#### Scenario: 单行修改对行内高亮
- **WHEN** details.diff 中一段恰为 1 行 `-` 紧邻 1 行 `+`（如只改一个词）
- **THEN** 两行保持红/绿，行内仅变更词反视频、共同缩进与未变词不染

#### Scenario: 多行块维持整行
- **WHEN** 连续多行 `-` 与多行 `+` 相邻
- **THEN** 各行整行红/绿，不做行内分段

#### Scenario: diff 降级
- **WHEN** edit 结果无 diff 字段但有 old/new
- **THEN** 行级对比产出红绿行且单行修改对同染行内高亮，卡片不 panic
