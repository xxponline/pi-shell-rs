# Spec Delta

## ADDED Requirements

### Requirement: assistant 文本 markdown 渲染

聊天区 MUST 把 assistant 消息的文本按 markdown 渲染：标题、粗体、斜体、行内代码、代码块（含边框与语言标签）、列表、引用、链接（显示文本并可感知目标地址）。代码块 MUST 做语法高亮。渲染 MUST 是纯函数式的（相同状态与宽度产出相同内容），且超长/畸形 markdown 不得导致 panic 或布局错乱。

#### Scenario: 结构元素着色

- **WHEN** assistant 消息含标题、粗体、行内代码与列表
- **THEN** 各元素以不同样式（颜色/修饰符）呈现，且文本内容与原文一致

#### Scenario: 代码块带语言与高亮

- **WHEN** assistant 消息含 ```rust 代码块
- **THEN** 代码块有边框/缩进与语言标签，代码行带语法着色

#### Scenario: 畸形与超长内容不崩溃

- **WHEN** 输入未闭合围栏、嵌套残缺或极长的 markdown 文本
- **THEN** 渲染不 panic、不进入死循环，内容以可读形式呈现

### Requirement: thinking 折叠切换

thinking 块 MUST 默认折叠为单行占位；`ctrl+t` MUST 在折叠与展开间切换全局可见性，展开时以弱化样式显示完整思考文本。切换 MUST 即时生效于历史与进行中的 thinking 块。

#### Scenario: 默认折叠

- **WHEN** 对话中存在非空 thinking 块且未按过 ctrl+t
- **THEN** 每个 thinking 块显示为单行占位（含字符数），不展开全文

#### Scenario: 展开与再次折叠

- **WHEN** 用户按 ctrl+t 后再按一次
- **THEN** 第一次后完整思考文本以弱化样式可见，第二次后恢复单行占位

### Requirement: 工具调用卡片

工具调用 MUST 以结构化卡片渲染，至少 read / bash / edit 三种：read 显示文件路径与内容预览，bash 显示命令与输出，edit 显示红绿 diff；卡片均显示工具名、状态（进行中/成功/失败）与耗时，失败为红色。未知工具名 MUST 走通用卡片（名称 + 参数摘要 + 状态）且不 panic。`ctrl+o` MUST 切换工具输出的展开/折叠。

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

### Requirement: 状态行 tokens 与费用

状态行 MUST 显示模型名、token 用量（输入/输出/缓存）与费用（cost），并在流式过程中随 usage 更新刷新。费用为 0 或缺失时 MUST 优雅降级（不显示错误值、不 panic）。

#### Scenario: 流式刷新

- **WHEN** message_update 携带新的 usage
- **THEN** 状态行的 token 数与费用随之更新

#### Scenario: 无费用信息

- **WHEN** usage 缺失或 cost 为 0
- **THEN** 状态行不显示误导性费用，其余字段正常显示

### Requirement: 队列可见与 follow_up 键

界面上 MUST 可见 steering/follow_up 队列的待处理内容（队列为空时不占显著空间）；`alt+enter` MUST 把当前输入作为 follow_up 发送（流式中排队），`alt+up` MUST 取回全部排队消息：发送 `clear_queue` 并把返回的 steering/followUp 回填输入框、队列清空（与原生 pi 的“restore all queued messages”一致）；队列为空时无副作用。

#### Scenario: 排队与显示

- **WHEN** 流式中用户按 alt+enter 输入消息
- **THEN** 该消息进入 follow_up 队列并在界面上可见，当前生成不被打断

#### Scenario: 取回全部排队消息

- **WHEN** 队列非空时用户按 alt+up
- **THEN** 发送 clear_queue，返回的排队消息全部回填输入框，队列显示清空；队列空时按 alt+up 无变化

### Requirement: 自动重试与运行错误提示

`auto_retry_start` / `auto_retry_end` 事件 MUST 渲染为对话流中的通知行（含尝试次数与错误摘要）；assistant 的运行级错误（message_end 携带 errorMessage） MUST 以红色 error 通知呈现。两类提示 MUST 不打断流式状态机的正常归约。

#### Scenario: 重试通知

- **WHEN** 收到 auto_retry_start（attempt=1, maxAttempts=3, errorMessage=...）
- **THEN** 对话流出现含"重试 1/3"与错误摘要的通知行，auto_retry_end 后显示结果

#### Scenario: 运行级错误红色显示

- **WHEN** assistant message_end 携带非空 errorMessage
- **THEN** 对话流出现红色加粗的 [error] 通知行，且 turn_end 重复该消息不产生重复通知
