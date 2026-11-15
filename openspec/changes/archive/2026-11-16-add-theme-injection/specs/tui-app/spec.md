# Spec Delta

## MODIFIED Requirements

### Requirement: 工具调用卡片

工具调用 MUST 以结构化卡片渲染，至少 read / bash / edit 三种：read 显示文件路径与内容预览，bash 显示命令与输出，edit 显示红绿 diff；卡片均显示工具名、状态（进行中/成功/失败）与耗时，失败为红色。卡片 MUST 按状态着背景面板（默认深色调为运行灰、成功墨绿、失败暗红；宿主注入配色时以注入值为准），背景 MUST NOT 替代状态文字。未知工具名 MUST 走通用卡片：头行为 `name key="value"` 形式的参数摘要（超长截断，展开态逐参数分行），含状态且不 panic。`ctrl+o` MUST 切换工具输出的展开/折叠。

#### Scenario: read 卡片

- **WHEN** 重放 tool_call fixture（read 成功）或实时出现 read 调用
- **THEN** 卡片显示工具名、文件路径、内容预览与成功状态

#### Scenario: edit 卡片显示 diff

- **WHEN** 出现 edit 工具调用及其结果
- **THEN** 卡片以红/绿呈现变更行，而不是原始 JSON

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

## ADDED Requirements

### Requirement: 宿主配色注入

embed 场景宿主 MUST 能经启动配置注入完整配色（工具卡三态面板色、消息区滚动条 track/thumb 色、语法高亮主题名），未注入时 MUST 使用默认值且输出与注入前逐字节一致。宿主 MUST 能在运行时切换配色，切换后一帧内全部注入面按新值重发，语法高亮与 markdown 渲染缓存 MUST NOT 漏出旧配色。注入的语法高亮主题名无效时 MUST 回退默认主题且不 panic。独立 TUI 启动路径 MUST 保持默认外观、不受注入影响。

#### Scenario: 未注入时默认行为不变

- **WHEN** 独立 TUI 启动，或 embed 未注入配色
- **THEN** 工具卡面板、滚动条、代码高亮颜色与本需求落地前完全一致，现有测试全绿

#### Scenario: 注入配色全面生效

- **WHEN** 宿主注入浅色配色后渲染含工具卡、滚动条、代码块的会话
- **THEN** 工具卡三态面板、滚动条 track/thumb、语法高亮前景均按注入值出色

#### Scenario: 运行时切换一帧内生效

- **WHEN** 宿主在会话展示中调用运行时切换配色
- **THEN** 下一帧三个注入面全部按新配色重发，无需宿主手动清屏或重灌

#### Scenario: 切换后缓存不漏旧色

- **WHEN** 切换配色前已有渲染缓存（markdown/语法高亮结果）
- **THEN** 切换后重绘的所有行均为新配色，无旧行残留

#### Scenario: 无效主题名回退默认

- **WHEN** 宿主注入的语法高亮主题名不在内置主题集中
- **THEN** 回退默认主题渲染，不 panic，宿主可感知注入未生效
