# tui-app Specification

## Purpose

在通用终端中以 ratatui 提供最小但可用的交互界面：管理终端生命周期与事件循环，把键盘输入转为 RPC prompt，把无头 session 状态实时渲染为滚动对话，并支持流式显示与中断。

## Requirements

### Requirement: 终端生命周期管理

应用 MUST 在启动时进入终端 raw mode 与 alternate screen，并在正常退出、中断或 panic 时恢复终端原有状态。渲染 MUST 使用 ratatui 输出到当前通用终端，不依赖特定终端模拟器。

#### Scenario: 干净退出后终端恢复

- **WHEN** 用户在空闲状态触发退出（如按 q 或 esc）
- **THEN** 应用恢复 canonical mode、关闭 alternate screen、显示光标并以退出码 0 结束，终端回到启动前可正常使用的状态

#### Scenario: 异常时仍恢复终端

- **WHEN** 运行期间发生致命错误或 panic
- **THEN** 终端仍被恢复（不留 raw mode/隐藏光标/残留在 alt screen），并输出可读错误信息

### Requirement: 启动与探活

应用启动时 MUST 拉起 `pi --mode rpc` 并发送 `get_state` 探活；当 pi 不可执行或未就绪（如未登录）时，MUST 在终端显示可读错误并干净退出，而不是无限等待或崩溃。

#### Scenario: 正常启动进入界面

- **WHEN** pi 可执行且已登录，启动应用
- **THEN** 应用成功进入 TUI 界面并处于空闲状态，可接受输入

#### Scenario: pi 不可用时可读报错

- **WHEN** `PI_BIN` 指向不存在的程序或 `get_state` 超时/失败
- **THEN** 应用显示明确的失败原因（包含"先用原生 pi 登录"之类的提示），恢复终端后以非零码退出

### Requirement: 发送 prompt 并流式渲染回复

用户在输入区编辑文本并提交后，应用 MUST 通过 RPC 发送 `prompt`，并把随后到达的会话事件持续渲染到滚动消息区：用户消息、正在流式生成的 assistant 文本/思考（实时、无需等待生成结束）、以及最终落盘的消息。

#### Scenario: 输入并发送一条消息

- **WHEN** 用户输入文本并按回车提交
- **THEN** 输入区被清空，消息区出现该用户消息，应用进入流式状态，并通过 RPC 发出 prompt

#### Scenario: 流式期间实时显示增量

- **WHEN** assistant 的 text/thinking delta 持续到达但尚未生成结束
- **THEN** 消息区实时显示已累积的 assistant 内容，无需等待 message_end/agent_settled

#### Scenario: 生成结束回到空闲

- **WHEN** 收到 agent_settled
- **THEN** 界面回到空闲状态，最终回复稳定显示，输入区可再次编辑

### Requirement: 中断生成

当会话处于流式状态时，用户触发中断（esc）MUST 通过 RPC 发送 `abort`，界面随后回到可输入状态；空闲状态下触发退出键 MUST 退出程序。

#### Scenario: 流式中中断

- **WHEN** 正在生成时用户按 esc
- **THEN** 应用发送 abort 命令，最终回到空闲状态且终端界面不损坏

#### Scenario: 空闲时退出

- **WHEN** 空闲状态用户按退出键
- **THEN** 程序正常退出并恢复终端

### Requirement: 基础输入编辑

输入区 MUST 至少支持文本字符输入、退格删除与回车提交；不要求多行编辑器、历史或自动补全。超长内容 MUST 不导致界面崩溃或布局错乱。

#### Scenario: 编辑与退格

- **WHEN** 用户键入若干字符后按退格
- **THEN** 输入区反映最新文本（字符被追加或删除），其余区域正常渲染

#### Scenario: 空消息不发送

- **WHEN** 输入为空（或仅空白）时按回车
- **THEN** 不发送 prompt，界面保持空闲

### Requirement: 三分区布局与状态指示

界面 MUST 至少包含滚动消息区、底部输入区和一个状态指示（顶部或底部），状态指示 MUST 区分流式与空闲，并在可用时显示会话名。消息区内容超出高度时 MUST 可滚动查看而不覆盖输入区。

#### Scenario: 布局分区稳定

- **WHEN** 渲染任意 session 状态
- **THEN** 消息区、输入区、状态区各自占据独立区域，输入区始终可见且可编辑

#### Scenario: 状态随 phase 变化

- **WHEN** phase 从空闲变为流式再回到空闲
- **THEN** 状态指示相应切换，且不会与消息内容混淆
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
### Requirement: overlay 弹层框架

界面 MUST 提供可复用的居中浮层机制：浮层绘制在主界面之上（底层内容不被破坏），打开时焦点键（方向/回车/esc/可打印字符）由浮层优先消费，关闭后底层界面与输入框恢复原状。浮层渲染 MUST 为纯函数（状态 → 帧），且浮层存在时消息区/状态行照常可见。

#### Scenario: 浮层覆盖且可恢复

- **WHEN** 任一浮层（选择器或扩展弹窗）打开后关闭
- **THEN** 关闭前后的底层界面渲染一致（消息区/状态行/输入框无残留、无错位）

#### Scenario: 焦点键被浮层消费

- **WHEN** 浮层打开时用户按方向键、回车或 esc
- **THEN** 这些键只作用于浮层（导航/确认/关闭），不进入输入框、不触发主界面键位

### Requirement: model 选择器与模型循环

`ctrl+l` MUST 打开模型选择器（数据来自 `get_available_models`），上下键导航、回车应用选中模型（`set_model`）并使状态行模型名随之更新，esc 取消不做任何切换；`ctrl+p` / `shift+ctrl+p` MUST 分别循环到下一个/上一个模型（`cycle_model`）并同步状态行。模型列表为空或请求失败时 MUST 有可见反馈且不卡死。

#### Scenario: 打开选择并应用

- **WHEN** 用户按 ctrl+l，在列表中导航后按回车
- **THEN** 选择器关闭，RPC 发出 set_model，状态行 model 段变为新模型名

#### Scenario: 取消不切换

- **WHEN** 选择器打开时按 esc
- **THEN** 选择器关闭且不发送 set_model，状态行模型名不变

#### Scenario: 循环切换模型

- **WHEN** 用户按 ctrl+p（或 shift+ctrl+p）
- **THEN** 发送 cycle_model，状态行模型名更新；无可用模型时给出提示且不崩溃

### Requirement: thinking 级别选择与循环

`shift+tab` MUST 循环 thinking 级别（`cycle_thinking_level`）并使状态行 thinking 段即时更新；MUST 可打开级别列表（`get_available_thinking_levels`）选择并应用（`set_thinking_level`），esc 取消。列表为空或请求失败时有可见反馈。

#### Scenario: 循环级别

- **WHEN** 用户按 shift+tab
- **THEN** 发送 cycle_thinking_level，状态行 thinking 段更新为新级别

#### Scenario: 列表选择级别

- **WHEN** 打开级别列表并选中一项回车
- **THEN** 发送 set_thinking_level，状态行更新；esc 取消时级别不变

### Requirement: extension UI 对话方法

收到 `extension_ui_request` 的 `select` / `confirm` / `input` / `editor` 请求时，界面 MUST 弹出对应浮层（select 显示选项列表、confirm 显示消息与确认/取消、input 显示单行输入、editor 显示可编辑的多行 prefill 文本）；用户提交 MUST 回发带原请求 id 的 `extension_ui_response`（select/input/editor 为 `value`，confirm 为 `confirmed`），取消 MUST 回发 `cancelled: true`。等待用户操作期间事件循环 MUST 继续处理其它事件（不阻塞）。

#### Scenario: select 请求往返

- **WHEN** 收到 method=select、options=[a,b] 的请求，用户选 b 回车
- **THEN** 回发 `extension_ui_response {id, value:"b"}`，浮层关闭

#### Scenario: confirm 取消

- **WHEN** 收到 method=confirm 的请求后用户按 esc
- **THEN** 回发 `extension_ui_response {id, cancelled:true}`，不发送 confirmed

#### Scenario: input 提交内容

- **WHEN** 收到 method=input 的请求，用户输入文本并提交
- **THEN** 回发 `value` 为所输入文本的响应，输入内容不泄漏到主输入框

#### Scenario: editor 多行编辑

- **WHEN** 收到 method=editor、prefill="line1\nline2" 的请求
- **THEN** 浮层显示并可编辑这两行，提交回发完整文本，取消回发 cancelled

### Requirement: extension UI 被动方法

`extension_ui_request` 的 `notify` / `setStatus` / `setWidget` / `setTitle` / `set_editor_text` 五种 method MUST 被即时应用且 MUST NOT 发送任何 `extension_ui_response`：notify 显示为可见通知，setStatus 更新状态行，setWidget 在编辑器上方显示 widget 文本（未设置时隐藏该区），setTitle 设置终端窗口标题，set_editor_text 写入主输入框。

#### Scenario: notify 显示且不回复

- **WHEN** 收到 method=notify、message="hello" 的请求
- **THEN** 界面出现该消息的通知显示，且 stdin 没有为它发出 extension_ui_response

#### Scenario: setTitle 与 set_editor_text 生效

- **WHEN** 依次收到 setTitle="proj" 与 set_editor_text="draft" 的请求
- **THEN** 终端标题变为 proj，主输入框内容变为 draft

#### Scenario: setWidget 显示与清除

- **WHEN** 收到 setWidget 带 widgetLines 的请求，随后收到 widgetLines 为 null 的请求
- **THEN** 编辑器上方先显示 widget 文本、后隐藏该区域

### Requirement: write/grep/find/ls 专用工具卡片

write / grep / find / ls 四种工具调用 MUST 以专用卡片渲染：write 显示目标路径与写入内容摘要，grep 显示模式与路径（`/pattern/ in path`）及匹配输出行，find 显示模式与搜索路径及结果行，ls 显示目录条目；四者均复用既有头行（状态/耗时）与 `ctrl+o` 展开折叠，结果缺失时降级为通用卡片且不 panic。

#### Scenario: grep 卡片

- **WHEN** 出现 grep 调用（pattern、path）及其结果
- **THEN** 卡片显示模式、路径与输出行（截断时有提示），而非通用 args JSON 摘要

#### Scenario: 结果缺失降级

- **WHEN** 上述任一工具的结果字段缺失或形状未知
- **THEN** 卡片仍显示头行与参数摘要，不 panic

#### Scenario: 四卡共享展开折叠

- **WHEN** 按 ctrl+o 切换
- **THEN** write/grep/find/ls 卡片与其它卡片同步在折叠/展开态间切换
