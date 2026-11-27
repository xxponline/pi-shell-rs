# Spec Delta

## ADDED Requirements

### Requirement: edit 执行前预览

edit 工具卡 MUST 在参数齐全且结果未到达（进行中）时先行展示执行前预览：
app 层异步读取目标文件、按 `edits`（或 `oldText`/`newText`）计算与结果同源
的红绿 diff（含行内高亮）先行渲染；结果到达后 MUST 以 `details.diff`
替换预览；文件不存在或 oldText 不匹配 MUST 以错误态呈现（红底头行 +
错误文案，不 panic、不发起执行之外的副作用）；会话切换/整体重建 MUST
清空预览态。预览计算 MUST NOT 阻塞事件循环。

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

### Requirement: 工具调用卡片

工具调用 MUST 以结构化卡片渲染，至少 read / bash / edit 三种：read 显示文件路径与内容预览，bash 显示命令与输出，edit 显示红绿 diff——恰 1 行改 1 行时 MUST 对该对行做词级行内分段，变更词以反视频高亮（行色保持红/绿）；卡片均显示工具名、状态（进行中/成功/失败）与耗时，失败为红色。卡片 MUST 按状态着背景面板（默认深色调为运行灰、成功墨绿、失败暗红；宿主注入配色时以注入值为准），背景 MUST NOT 替代状态文字。未知工具名 MUST 走通用卡片：头行为 `name key="value"` 形式的参数摘要（超长截断，展开态逐参数分行），含状态且不 panic。`ctrl+o` MUST 切换工具输出的展开/折叠。

#### Scenario: read 卡片

- **WHEN** 重放 tool_call fixture（read 成功）或实时出现 read 调用
- **THEN** 卡片显示工具名、文件路径、内容预览与成功状态

#### Scenario: edit 卡片显示 diff

- **WHEN** 出现 edit 工具调用及其结果
- **THEN** 卡片以红/绿呈现变更行而不是原始 JSON；恰 1 行改 1 行时行内变更词以反视频标出

#### Scenario: 未知工具不崩溃

- **WHEN** 出现名称不在已知集合中的工具调用
- **THEN** 以通用卡片显示名称与状态，不 panic

#### Scenario: ctrl+o 展开折叠

- **WHEN** 用户按 ctrl+o 一次再按一次
- **THEN** 工具输出在完整显示与折叠摘要间切换

#### Scenario: 状态背景面板

- **WHEN** 卡片处于运行中、成功、失败三种状态之一（未注入配色）
- **THEN** 卡片背景分别为灰、墨绿、暗红色调，且状态文字（进行中/成功/失败）仍可见

#### Scenario: 注入配色时面板取注入值

- **WHEN** 宿主注入了含工具卡三态面板色的配色，卡片处于三种状态之一
- **THEN** 面板背景按注入值渲染，状态文字仍可见

#### Scenario: 未知工具头行参数摘要

- **WHEN** 出现未知工具调用，args 含 command、timeout 等参数
- **THEN** 头行显示 `name key="value"` 形式的参数摘要（超长截断），展开态逐参数分行
