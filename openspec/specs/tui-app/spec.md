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

用户在输入区编辑文本并提交后，应用 MUST 通过 RPC 发送 `prompt`，并把随后到达的会话事件持续渲染到滚动消息区：用户消息、正在流式生成的 assistant 文本/思考（实时、无需等待生成结束）、以及最终落盘的消息。会话处于流式状态时提交 MUST 为 prompt 携带 `streamingBehavior:"steer"`（打断重定向），MUST NOT 发送缺省该字段的 prompt（pi 将拒绝并报错）。

#### Scenario: 输入并发送一条消息

- **WHEN** 用户输入文本并按回车提交
- **THEN** 输入区被清空，消息区出现该用户消息，应用进入流式状态，并通过 RPC 发出 prompt

#### Scenario: 流式中提交为打断重定向

- **WHEN** 会话正在流式生成时用户按回车提交新消息
- **THEN** 发出的 prompt 携带 `streamingBehavior:"steer"`，不产生错误提示，当前生成被新指令打断重定向

#### Scenario: 流式期间实时显示增量

- **WHEN** assistant 的 text/thinking delta 持续到达但尚未生成结束
- **THEN** 消息区实时显示已累积的 assistant 内容，无需等待 message_end/agent_settled

#### Scenario: 生成结束回到空闲

- **WHEN** 收到 agent_settled
- **THEN** 界面回到空闲状态，最终回复稳定显示，输入区可再次编辑
### Requirement: 中断生成

当会话处于流式状态时，用户触发中断（esc）MUST 通过 RPC 发送 `abort`，并 MUST 把排队中的消息恢复回输入框（等价 `clear_queue`：返回的 steering/followUp 全部回填、队列清空），界面随后回到可输入状态；空闲状态下 esc MUST 为无操作（不退出、不清空，对齐 pi）。

#### Scenario: 流式中中断

- **WHEN** 流式生成中用户按 esc（队列非空时）
- **THEN** 应用发送 abort 与 clear_queue，排队消息回填输入框、队列显示清空，最终回到空闲状态且终端界面不损坏

#### Scenario: 流式中无队列中断

- **WHEN** 流式生成中队列为空时用户按 esc
- **THEN** 应用发送 abort，回到空闲状态，输入区内容不变

#### Scenario: 空闲时退出

- **WHEN** 空闲状态用户按退出键（ctrl+d 空输入或 500ms 内双击 ctrl+c）
- **THEN** 程序正常退出并恢复终端

#### Scenario: 空闲时 esc 无操作

- **WHEN** 空闲状态下用户按 esc（无论输入是否为空）
- **THEN** 不发送任何命令、不退出、输入区不变
### Requirement: 基础输入编辑

输入区 MUST 是带光标的多行编辑器：支持字符插入、多行换行（`shift+enter` / `ctrl+j`）、光标移动（←→/ctrl+b/f 字符级、↑↓ 跨行、home/ctrl+a 行首、end/ctrl+e 行尾、alt+←→ 词级）、删除（backspace、delete 在非空缓冲时向后删字符、ctrl+w/alt+backspace 后删词、alt+d 前删词、ctrl+u 删至行首、ctrl+k 删至行尾）。提交时整个缓冲（含换行）作为一条消息发送。输入区高度 MUST 随行数增长但有上限，不挤压消息区导致其不可见。不提供输入历史；Tab 补全存在，其行为由 `path-completion`（纯路径）、`slash-autocomplete`（斜杠）、`file-completion`（`@` 文件）能力定义。超长内容 MUST NOT 导致界面崩溃或布局错乱。

#### Scenario: 编辑与退格

- **WHEN** 用户键入若干字符后按退格
- **THEN** 输入区反映最新文本（字符在光标处被插入或删除），其余区域正常渲染

#### Scenario: 多行换行与跨行光标

- **WHEN** 用户按 shift+enter 插入换行后继续输入，再用 ↑/↓ 移动光标
- **THEN** 输入区显示多行文本，光标在行间移动且各编辑操作作用于光标所在位置

#### Scenario: 词级与行级删除

- **WHEN** 光标位于行中时依次按 ctrl+w、ctrl+u、ctrl+k
- **THEN** 分别删除光标前一个词、删至行首、删至行尾，其余文本保持

#### Scenario: 空消息不发送

- **WHEN** 输入为空（或仅空白）时按回车
- **THEN** 不发送 prompt，界面保持空闲

#### Scenario: 超长多行不破坏布局

- **WHEN** 输入达数十行超长内容
- **THEN** 输入区按上限高度显示（可滚动/截断显示），消息区仍可见，不 panic
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

工具调用 MUST 以结构化卡片渲染，至少 read / bash / edit 三种：read 显示文件路径与内容预览，bash 显示命令与输出，edit 显示红绿 diff；卡片均显示工具名、状态（进行中/成功/失败）与耗时，失败为红色。卡片 MUST 按状态着背景面板（运行灰、成功墨绿、失败暗红），背景 MUST NOT 替代状态文字。未知工具名 MUST 走通用卡片：头行为 `name key="value"` 形式的参数摘要（超长截断，展开态逐参数分行），含状态且不 panic。`ctrl+o` MUST 切换工具输出的展开/折叠。

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

- **WHEN** 卡片处于运行中、成功、失败三种状态之一
- **THEN** 卡片背景分别为灰、墨绿、暗红色调，且状态文字（进行中/成功/失败）仍可见

#### Scenario: 未知工具头行参数摘要

- **WHEN** 出现未知工具调用，args 含 command、timeout 等参数
- **THEN** 头行显示 `name key="value"` 形式的参数摘要（超长截断），展开态逐参数分行

### Requirement: 状态行 tokens 与费用

状态行 MUST 显示模型名、token 用量（输入/输出/缓存）与费用（cost），并在流式过程中随 usage 更新刷新。费用为 0 或缺失时 MUST 优雅降级（不显示错误值、不 panic）。

#### Scenario: 流式刷新

- **WHEN** message_update 携带新的 usage
- **THEN** 状态行的 token 数与费用随之更新

#### Scenario: 无费用信息

- **WHEN** usage 缺失或 cost 为 0
- **THEN** 状态行不显示误导性费用，其余字段正常显示

### Requirement: 队列可见与 follow_up 键

界面上 MUST 可见 steering/follow_up 队列的待处理内容（队列为空时不占显著空间）；流式中 `alt+enter` MUST 把当前输入作为 follow_up 发送（排队，不打断当前生成），空闲时 `alt+enter` MUST 等同普通回车提交；`alt+up` MUST 取回全部排队消息：发送 `clear_queue` 并把返回的 steering/followUp 回填输入框、队列清空（与原生 pi 的 "restore all queued messages" 一致）；队列为空时无副作用。

#### Scenario: 排队与显示

- **WHEN** 流式中用户按 alt+enter 输入消息
- **THEN** 该消息进入 follow_up 队列并在界面上可见，当前生成不被打断

#### Scenario: 空闲时 alt+enter 等同回车

- **WHEN** 空闲状态用户在输入区有文本时按 alt+enter
- **THEN** 行为与普通回车一致：作为 prompt 提交（流式语义不适用），输入区清空

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

`ctrl+l` MUST 打开模型选择器（数据来自 `get_available_models`），上下键导航、回车应用选中模型（`set_model`）并使状态行模型名随之更新，esc 取消不做任何切换；`ctrl+p` / `shift+ctrl+p` MUST 分别循环到下一个/上一个模型（`cycle_model`）并同步状态行。模型列表为空或请求失败时 MUST 有可见反馈且不卡死。`/model` 命令 MUST 与 ctrl+l 等价地打开选择器；`/model <搜索>` 对可用模型做精确匹配（`provider/id` 或 `id`，大小写不敏感）并命中时直接 `set_model` 且给出 "Model: …" 状态提示，无命中时回退打开选择器。

#### Scenario: 打开选择并应用

- **WHEN** 用户按 ctrl+l，在列表中导航后按回车
- **THEN** 选择器关闭，RPC 发出 set_model，状态行 model 段变为新模型名

#### Scenario: 取消不切换

- **WHEN** 选择器打开时按 esc
- **THEN** 选择器关闭且不发送 set_model，状态行模型名不变

#### Scenario: 循环切换模型

- **WHEN** 用户按 ctrl+p（或 shift+ctrl+p）
- **THEN** 发送 cycle_model，状态行模型名更新；无可用模型时给出提示且不崩溃

#### Scenario: 斜杠命令入口与精确匹配

- **WHEN** 用户输入 /model 回车，或 /model gpt-5.4 回车
- **THEN** 前者打开模型选择器；后者精确命中时直接切换并显示状态提示，无命中时打开选择器
### Requirement: thinking 级别选择与循环

`shift+tab` MUST 循环 thinking 级别（`cycle_thinking_level`）并使状态行 thinking 段即时更新；MUST 可打开级别列表（`get_available_thinking_levels`）选择并应用（`set_thinking_level`），esc 取消。列表为空或请求失败时有可见反馈。`/thinking` 命令 MUST 等价地打开级别列表；`/thinking <级别>` 精确匹配（大小写不敏感）时直接应用，未知级别 MUST 显示红色错误并列出可用级别。

#### Scenario: 循环级别

- **WHEN** 用户按 shift+tab
- **THEN** 发送 cycle_thinking_level，状态行 thinking 段更新为新级别

#### Scenario: 列表选择级别

- **WHEN** 打开级别列表并选中一项回车
- **THEN** 发送 set_thinking_level，状态行更新；esc 取消时级别不变

#### Scenario: 斜杠命令入口与未知级别

- **WHEN** 用户输入 /thinking high 回车，或 /thinking bogus 回车
- **THEN** 前者直接应用该级别；后者显示红色错误并列出全部可用级别，级别不变
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

write / grep / find / ls 四种工具调用 MUST 以专用卡片渲染：write 显示目标路径与写入内容摘要（折叠态含预览行数与总行数），grep 显示模式与路径（`/pattern/ in path`）及匹配输出行（头行含 glob 与 limit 参数），find 显示模式与搜索路径及结果行（头行含 limit），ls 显示目录条目（头行含 limit）；四者均复用既有头行（状态/耗时）与 `ctrl+o` 展开折叠，结果缺失时降级为通用卡片且不 panic。

#### Scenario: grep 卡片

- **WHEN** 出现 grep 调用（pattern、path）及其结果
- **THEN** 卡片显示模式、路径与输出行（截断时有提示），而非通用 args JSON 摘要

#### Scenario: 结果缺失降级

- **WHEN** 上述任一工具的结果字段缺失或形状未知
- **THEN** 卡片仍显示头行与参数摘要，不 panic

#### Scenario: 四卡共享展开折叠

- **WHEN** 按 ctrl+o 切换
- **THEN** write/grep/find/ls 卡片与其它卡片同步在折叠/展开态间切换

#### Scenario: 头行参数完整

- **WHEN** grep 带 glob/limit、ls 带 limit 发出调用
- **THEN** 头行显示这些参数（如 `limit 50`），与 pi 头行信息一致

### Requirement: 退出与清空键位

退出与清空 MUST 对齐原生 pi：`ctrl+c` 单击=清空输入、500ms 内再次按下=退出程序；`ctrl+d` 仅当输入为空时退出（输入非空时为光标处向后删字符，归输入编辑处理）；空闲状态 esc 为无操作；`q` 为普通输入字符（不承担退出）。MUST 提供至少两种可发现的退出方式（ctrl+d 空输入、双击 ctrl+c），退出时恢复终端。

#### Scenario: 单击 ctrl+c 清空

- **WHEN** 输入非空时按一次 ctrl+c
- **THEN** 输入区被清空，程序不退出

#### Scenario: 双击 ctrl+c 退出

- **WHEN** 500ms 内连续两次按下 ctrl+c
- **THEN** 程序退出并恢复终端（无论输入是否为空）

#### Scenario: ctrl+d 仅空输入退出

- **WHEN** 输入为空时按 ctrl+d
- **THEN** 程序退出并恢复终端；输入非空时按 ctrl+d 不退出（作为向后删字符）

#### Scenario: q 为普通字符

- **WHEN** 空闲状态键入字母 q
- **THEN** q 出现在输入区，程序不退出

### Requirement: kill-ring 与 undo

kill 类删除 MUST 进入 kill-ring：`ctrl+k`（删至行尾）、`ctrl+u`（删至行首）、`ctrl+w`/`alt+backspace`（后删词）、`alt+d`（前删词）删除的文本入环；`ctrl+y` MUST 粘贴环顶内容到光标处，`alt+y` MUST 回退到上一次被 kill 的内容（yank-pop）。`ctrl+-` MUST 撤销最近一次编辑（快照回退，带上限，不越界）。kill-ring 与 undo MUST 跨换行与多行编辑正确工作。

#### Scenario: kill 后 yank

- **WHEN** 用户按 ctrl+k 删至行尾后按 ctrl+y
- **THEN** 被删文本原样插回光标处

#### Scenario: yank-pop 回退

- **WHEN** 连续两次 kill（不同文本）后按 ctrl+y 再按 alt+y
- **THEN** 第一次粘贴内容被第二次 kill 的内容替换

#### Scenario: undo 回退编辑

- **WHEN** 用户完成若干编辑（插入/删除/换行）后按 ctrl+-
- **THEN** 最近一次编辑被撤销；到达最早状态后再按无变化、不 panic

### Requirement: 斜杠命令分发

输入以 `/` 开头且命中本地命令表时，应用 MUST 在本地执行该命令、不把整行发送给模型，并在执行后清空输入框；未命中命令表的 `/xxx` 文本 MUST 作为普通消息照常发送（与 pi 一致）——**其中命中动态扩展命令的文本由 pi 的 `prompt()` 内部拦截执行**（本 TUI 不感知执行细节，仅负责按下方压缩期规则发送）。本地命令集为 pi 的全部 24 个内置命令：`/new`、`/resume`、`/tree`、`/fork`、`/clone`、`/session`、`/name <名称>`、`/compact [指令]`、`/quit`、`/model [搜索]`、`/thinking [级别]`、`/copy`、`/export [路径]`、`/import <路径>`、`/changelog`、`/hotkeys`、`/trust`、`/reload`、`/settings`、`/scoped-models`、`/share`、`/bug`、`/login`、`/logout`。空白输入按普通提交处理（不触发分发）。

#### Scenario: 命中本地命令

- **WHEN** 用户输入 /tree 并回车
- **THEN** 打开会话树浮层，该行不作为消息发送，输入框清空

#### Scenario: 未命中的斜杠文本照常发送

- **WHEN** 用户输入 /some-unknown-command 并回车
- **THEN** 该行作为普通 prompt 发送（进入消息区），行为与非斜杠文本一致

#### Scenario: 斜杠空输入

- **WHEN** 用户只输入 / 并回车
- **THEN** 作为普通 prompt 发送（/some… 未知分支），不 panic；输入全空白时仍不发送

#### Scenario: 扩展命令由 pi 拦截执行

- **WHEN** 用户输入动态扩展命令 /my-ext 并回车
- **THEN** 该行作为 prompt 发送（pi 内部拦截执行、不进模型上下文），输入框清空
### Requirement: 未支持命令提示桩

`/share`、`/bug`、`/login`、`/logout` MUST 作为本地命令拦截执行（不发送给模型、清空输入），并以黄色提示说明未实现原因（网络上报类 / 项目不做交互式登录，依赖预登录凭据），界面回到空闲。

#### Scenario: 桩命令提示

- **WHEN** 用户输入 /login 回车
- **THEN** 出现黄条提示"依赖预登录凭据，未实现交互式登录"，该行不进入消息区，输入框清空

### Requirement: 复制与信息展示命令

`/copy` MUST 取最后一条 assistant 文本（`get_last_assistant_text`）经终端转义序列写入系统剪贴板并给出成功提示，无内容时红条提示；`/hotkeys` MUST 打开键位表浮层（分组列出当前全部键位，esc/回车关闭）；`/changelog` MUST 读取 pi 包内 CHANGELOG.md 最新条目渲染进消息区，文件缺失时可见提示且不崩溃。

#### Scenario: 复制最后回复

- **WHEN** 会话已有 assistant 回复时输入 /copy 回车
- **THEN** 黄条/状态提示已复制，回复文本进入剪贴板；无回复时红条提示

#### Scenario: 键位表与更新日志

- **WHEN** 用户依次输入 /hotkeys、/changelog 回车
- **THEN** 前者弹出键位表浮层（esc 关闭），后者在消息区渲染 changelog 内容

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
### Requirement: 配置重读

`/reload` MUST 在空闲时执行本地配置重读（会话目录等设置）并给出绿色完成提示、界面重绘；流式响应或压缩进行中 MUST 以黄条拒绝并说明原因，不中断当前运行。

#### Scenario: 空闲时重读

- **WHEN** 空闲状态下输入 /reload 回车
- **THEN** 本地配置重新加载，出现完成提示，界面正常重绘

#### Scenario: 运行中拒绝

- **WHEN** 流式响应进行中输入 /reload 回车
- **THEN** 黄条提示等待当前响应完成，不发生重读，流式不受影响

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

### Requirement: bash 模式提交与运行

输入 trim 后以 `!` 起始且 `!` 后有内容时，提交 MUST 走 `bash` RPC 执行（`!!` 前缀传 `excludeFromContext: true`），该行不发送给模型、输入清空；`!` 后无内容 MUST 按普通消息照常发送。输入以 `!` 起始时编辑器边框 MUST 变色提示 bash 模式，恢复普通文本后还原。bash 运行期间再次提交 `!cmd` MUST 显示黄条警告（说明已有命令在跑、Esc 可中止）且**输入保留不清空**；运行中空闲 Esc MUST 发送 `abort_bash` 中止执行，其优先级高于双击 Esc（不计入双击窗口），且低于流式中止（流式时 Esc 仍中止生成）。运行中的输出 MUST 在消息区实时显示，命令完成后条目呈现退出码/取消态/截断提示。

#### Scenario: bang 提交执行与边框提示

- **WHEN** 用户输入 `!ls -la` 回车（此前输入 `!` 时边框已变色）
- **THEN** 发送 bash 命令（非 prompt），输入清空，边框恢复；`!!echo hi` 发送时带 excludeFromContext=true

#### Scenario: 空 bang 照常发送

- **WHEN** 用户只输入 `!` 回车
- **THEN** 作为普通 prompt 发送，不触发 bash 执行

#### Scenario: 运行中门控与保留

- **WHEN** bash 运行中用户输入 `!pwd` 回车
- **THEN** 黄条警告已有命令在跑（提示 Esc 中止），输入文本保留在编辑器中，不发送新命令

#### Scenario: Esc 中止与优先级

- **WHEN** bash 运行中按 Esc（空闲、无浮层）
- **THEN** 发送 abort_bash 且不触发双击 Esc；流式中按 Esc 时仍为中止生成而非 bash

#### Scenario: 流式输出与完成态

- **WHEN** 命令执行中持续收到输出分片、随后收到结果
- **THEN** 消息区实时增长输出文本，完成后条目显示退出码（或取消/截断状态）

### Requirement: 动态斜杠命令补全

应用 MUST 在启动时（及 `/reload` 后）经 `get_commands` RPC 获取动态命令列表
（扩展命令、提示模板、`skill:` 命令，含 name/description/source/sourceInfo）并缓存，
作为 `/` 补全菜单的候选来源之一（与内置 24 个命令合并）。合并规则对齐 pi：与内置命令
**同名的扩展命令 MUST 被过滤**（不出现且不遮蔽内置项）；描述 MUST 带源标签
（`[extension]` / `[prompt]` / `[skill]`，置于描述前）；全局设置 `enableSkillCommands`
为 false 时 MUST 过滤全部 `skill:` 候选（缺省视为 true）。拉取失败 MUST 降级为仅内置
候选且不崩溃；动态命令在菜单中的行为（模糊过滤、预选、Tab/Enter 应用）与内置一致。

#### Scenario: 动态命令出现在补全

- **WHEN** 扩展注册了 `/my-ext`（描述 "Do thing"），用户键入 `/my`
- **THEN** 菜单出现 `my-ext` 候选且描述带 `[extension]` 标签，Tab 应用为 `/my-ext `

#### Scenario: 冲突过滤与 skill 开关

- **WHEN** 扩展注册了与内置同名的 `/tree`，或 `enableSkillCommands=false` 时键入 `/ski`
- **THEN** 同名扩展项不出现在候选中；`skill:` 候选全部被过滤，菜单不显示

#### Scenario: 拉取失败降级

- **WHEN** `get_commands` 请求失败或超时
- **THEN** 补全仍可用（仅内置 24 项），界面无错误弹层

### Requirement: 外部编辑器

`ctrl+g` MUST 挂起 TUI 并以继承 stdio 启动外部编辑器编辑当前输入：编辑器命令解析
对齐 pi（全局 settings.json 的 `externalEditor` → `$VISUAL`/`$EDITOR` → 默认
`nano`（Windows `notepad`）），临时文件为 `prompt.md` 且内容为挂起时的输入全文。
编辑器以退出码 0 结束时 MUST 读回内容（去 BOM 与末尾换行）替换输入框；启动失败或
非零退出 MUST 以黄条提示原因且**输入保持不变**。编辑器结束后 MUST 恢复 TUI（退出
编辑器前的会话状态不受影响）并全量重绘；输入区非空时 MUST 直接把当前内容带入
编辑器（不丢字）。

#### Scenario: 成功编辑回填

- **WHEN** 输入框有文本，按 ctrl+g，在编辑器中修改后保存退出（exit 0）
- **THEN** TUI 恢复显示，输入框内容替换为编辑后的全文

#### Scenario: 失败保持原样

- **WHEN** 编辑器命令不存在，或编辑器以非零码退出
- **THEN** TUI 恢复显示，黄条提示失败原因，输入框内容与挂起前一致

#### Scenario: 命令解析顺序

- **WHEN** settings.json 无 `externalEditor`、`$VISUAL` 与 `$EDITOR` 均未设置
- **THEN** 使用默认编辑器（nano/notepad）启动，不 panic

### Requirement: 剪贴板复制与粘贴

`ctrl+x` MUST 把最后一条 assistant 文本复制到终端剪贴板（OSC52，复用 `/copy`
通路），无内容时红条提示。`ctrl+v` MUST 按链式回退粘贴剪贴板：优先图片（按平台
后端读取图片字节）→ 写入临时文件并在光标处插入其路径；无图片则读取文本 → 光标处
插入；剪贴板为空则无操作。平台后端 MUST 覆盖：macOS 图片经 `osascript` JXA
（NSPasteboard）、文本经 `pbpaste`；Linux 经 `wl-paste`/`xclip`；Windows 经
PowerShell；后端不存在或读取失败 MUST 静默降级到下一环（图片失败→试文本→无操作），
不得弹错误、不得阻塞超过数秒。本 TUI 无选区特性，ctrl+x 不做"复制选区"分支（记入
文档差异）。

#### Scenario: 复制最后回复

- **WHEN** 会话已有 assistant 回复时按 ctrl+x
- **THEN** 该文本进入终端剪贴板并有成功提示；无回复时红条

#### Scenario: 图片粘贴为路径

- **WHEN** 系统剪贴板含图片时按 ctrl+v
- **THEN** 图片写入临时文件，输入框光标处插入该文件路径（不插入乱码字节）

#### Scenario: 文本回退与空剪贴板

- **WHEN** 剪贴板只有文本时按 ctrl+v，或剪贴板为空
- **THEN** 文本插入光标处；为空时无操作、无错误提示

#### Scenario: 后端缺失降级

- **WHEN** 平台图片后端命令不存在（如 Linux 无 wl-paste/xclip）
- **THEN** 静默尝试文本后端，仍失败则无操作，界面无红条

### Requirement: 输入光标显示定位

输入区的终端光标位置 MUST 按**显示列**计算:光标行中光标前缀的每个字符按其
显示宽度计格(全角 CJK/emoji 计 2 格,半角计 1 格),`cx = 输入区内左边距 +
光标前缀显示宽 - 水平滚动偏移`。水平滚动偏移 MUST 以显示列为单位,可见行
MUST 按显示列截取且不切断宽字符(跨偏移点的宽字符整字保留)。编辑层的字符
序号语义不变,仅渲染换算修正。

#### Scenario: 宽字符行的光标位置

- **WHEN** 输入行为 `你好😊hahah`(光标在行尾)且输入区未发生水平滚动
- **THEN** 终端光标位于该行第 12 格(1 + 2+2+2+1+1+1+1+1),而非按字符序号
  计算的第 9 格

#### Scenario: 宽字符行的水平滚动

- **WHEN** 单行输入全为全角字符且长度超过输入区宽度,光标位于行尾
- **THEN** 水平滚动使光标仍可见(光标 x 在输入区内且 ≤ 右边界),可见文本
  不出现被拦腰截断的半个宽字符导致的错位

### Requirement: 运行中部分输出与计时

工具执行期间，卡片 MUST 实时显示已到达的部分输出（随 `tool_execution_update` 事件增长，无需等待执行结束）；MUST 显示运行计时 `Elapsed <时长>` 并每秒刷新，结束后 MUST 显示 `Took <时长>`（服务端提供 durationMs 时以服务端值为准）。时长格式 MUST 为 `4.2s`（<1 分钟）、`1m 30s`（<1 小时）、`2h 3m 4s`（≥1 小时），MUST NOT 显示裸毫秒数字。

#### Scenario: 运行中看到输出增长

- **WHEN** bash 命令执行中，多个 `tool_execution_update` 携带递增输出到达
- **THEN** 卡片输出随事件实时增长，无需等待 tool_execution_end

#### Scenario: 运行中计时刷新

- **WHEN** 工具已运行约 5 秒仍未结束
- **THEN** 卡片显示 `Elapsed 5.0s` 左右的计时并持续刷新

#### Scenario: 完成后计时与格式

- **WHEN** 工具以 durationMs=90000 结束
- **THEN** 卡片显示 `Took 1m 30s`，而非毫秒数字

### Requirement: 折叠预览与截断警告

折叠态预览 MUST 按工具保留行数并对齐 pi：bash 保留**尾部** 5 行（exit code 与最终结果在末尾），grep 15 行、find/ls 20 行、write 10 行、其余 10 行（保留头部），且 MUST 显示隐藏行数提示 `... (N earlier/more lines, ctrl+o to expand)`。输出被截断时 MUST 显示带具体数字或路径的警告而非仅省略号：bash 为 `[Full output: <path>]` 与行数/字节截断详情，read 为首行超限/行数/字节三形态，grep/find/ls 为各自 limit 警告。

#### Scenario: bash 折叠保留尾部

- **WHEN** bash 输出 100 行、卡片处于折叠态
- **THEN** 显示末尾 5 行（含 exit code）与 "... (95 earlier lines, ctrl+o to expand)" 提示

#### Scenario: 截断警告带数字

- **WHEN** bash 输出被按行截断且服务端提供了完整输出文件路径
- **THEN** 卡片显示 `[Full output: <path>]` 与 `showing X of Y lines` 具体数字

#### Scenario: 各工具 limit 警告

- **WHEN** grep 达到 matches limit 或 ls 达到 entries limit
- **THEN** 卡片显示对应的 `[Truncated: ... limit]` 警告

### Requirement: 头行参数与分类

工具头行 MUST 显示 pi 对齐的关键参数：bash 为 prompt 样式 `$ <命令>`，有 timeout 参数时附 ` (timeout Ns)`；read MUST 显示行范围（`:1-20`、`:5` 形式），折叠态对 SKILL.md、文档与资源类文件 MUST 显示 compact 分类（`[skill] <目录名>`、`read <kind> <相对路径>`）并附 expand 提示。头行 MUST 同时保留工具名、状态文字与计时信息可见。

#### Scenario: bash 头行与超时后缀

- **WHEN** 发出 bash 调用（command=ls -la，timeout=5）
- **THEN** 头行以 `$ ls -la` 样式显示并附 `(timeout 5s)`，工具名与状态仍可见

#### Scenario: read 行范围与 compact 分类

- **WHEN** read 带 offset=5 limit=20 调用，或读取 SKILL.md
- **THEN** 头行显示 `:5-24` 行范围；SKILL.md 显示 `[skill] <目录名>` 分类与 expand 提示

### Requirement: 消息区滚动回看

消息区内容超出视口高度时 MUST 支持回看：PageUp/PageDown 以整页滚动视口
（页长 = 视口高 − 4 行上下文重叠），滚动 MUST NOT 覆盖或挤压输入区。用户
上滚离开底部后，新到达的内容 MUST NOT 抢占视口位置（视口钉在原内容上）；
滚回底部时 MUST 恢复对新内容的贴底跟随。内容行数骤变（如会话切换、压缩）
导致偏移越界时 MUST clamp 到可见范围并恢复贴底跟随。

#### Scenario: 整页滚动回看

- **WHEN** 消息区有多屏历史内容，按 PageUp
- **THEN** 视口向上滚动约一屏（保留 4 行上下文重叠），输入区仍可见且可编辑

#### Scenario: 上滚后新内容不抢位

- **WHEN** 用户上滚离开底部后，流式回复持续产生新内容
- **THEN** 视口停留在原内容上不被新内容抢占；滚回底部后恢复贴底跟随

#### Scenario: 内容骤变后恢复跟随

- **WHEN** 会话切换或压缩导致消息区行数骤减、原滚动偏移越界
- **THEN** 视口 clamp 到可见范围底部并恢复贴底跟随，不出现空白或错位

### Requirement: 鼠标滚轮滚动

消息区 MUST 支持鼠标滚轮/触摸板滚动：滚轮事件按**行粒度**移动视口（上滚看
更早内容、下滚看更新内容），滚动 MUST NOT 覆盖或挤压输入区。连续快速滚动时
每事件移动行数 MUST 随速度放大（每事件最多 6 行；慢速/单次滚动为 1 行），
Alt+滚轮 MUST 以 5 倍行数滚动。上滚离开底部后的跟随语义与「消息区滚动回看」
共享：新内容不抢位、滚回底部恢复贴底跟随。浮层打开时滚轮 MUST 被忽略
（不作用于消息区）。

#### Scenario: 行粒度滚动与速度放大

- **WHEN** 慢速逐次滚动滚轮，随后快速连续滚动
- **THEN** 慢速时每事件移动 1 行，快速时每事件行数随速度放大（每事件最多
  6 行）；输入区始终可见且可编辑

#### Scenario: Alt+滚轮倍率

- **WHEN** 按住 Alt 滚动滚轮
- **THEN** 每事件移动行数为未按 Alt 时的 5 倍

#### Scenario: 滚到底恢复跟随

- **WHEN** 滚轮下滚至底部后，流式回复继续产生新内容
- **THEN** 视口恢复对新内容的贴底跟随（与键盘回底同语义）

#### Scenario: 浮层打开时忽略

- **WHEN** 任意浮层（选择器/会话树/设置）打开时滚动滚轮
- **THEN** 消息区视口不动

### Requirement: 工具卡片点击展开

已有**最终结果**的工具卡片 MUST 支持左键点击切换该卡片的展开/收起（单卡
粒度）：点击仅作用于被点卡片，其它卡片状态不变。运行中（尚无最终结果）的
卡片点击 MUST 无响应；点击非卡片行 MUST 无动作。`Ctrl+O` 全局切换 MUST
保持既有行为，且全局切换后所有卡片 MUST 跟随全局新状态（单卡点击产生的
独立状态被全局切换清除）；此后单卡点击仍可独立切换。折叠预览策略（bash
尾 5 行等）不受点击影响——收起态仍按既有折叠策略渲染。

#### Scenario: 点击展开单卡

- **WHEN** 左键点击一张已有结果的折叠卡片
- **THEN** 该卡片展开显示完整输出，其它卡片与全局展开标志不变

#### Scenario: 点击收起单卡

- **WHEN** 左键点击一张已展开的卡片
- **THEN** 该卡片恢复折叠，按既有折叠策略显示预览

#### Scenario: 运行中卡片点击无效

- **WHEN** 工具运行中（尚无最终结果）点击其卡片
- **THEN** 卡片状态与视口均不变

#### Scenario: 全局切换覆盖单卡状态

- **WHEN** 单卡点击切换后按 Ctrl+O
- **THEN** 所有卡片跟随全局新状态（单卡独立状态被清除）；随后单卡点击
  仍可独立切换

### Requirement: thinking 块单块展开

已有**非空** thinking 块 MUST 支持左键点击切换该块的展开/收起（单块粒度）：
点击仅作用于被点块，其它 thinking 块、工具卡片与全局展开标志不变。点击
thinking 之外的行 MUST 无动作（普通文本行；工具卡片行按其既有需求处理）。
`ctrl+t` 全局切换 MUST 保持既有行为（默认折叠、单行占位格式均不变），且
全局切换后所有 thinking 块 MUST 跟随全局新状态（单块点击产生的独立状态被
全局切换清除）；此后单块点击仍可独立切换。thinking 流式生成过程中点击 MUST
生效并持续跟随增量。折叠/展开的判定以块的当前生效状态（单块覆盖优先、否则
全局）为准，收起态仍按既有占位策略渲染。

#### Scenario: 点击展开单块

- **WHEN** 左键点击一行折叠的 thinking 占位
- **THEN** 该块展开显示完整思考文本，其它 thinking 块与全局展开标志不变

#### Scenario: 点击收起单块

- **WHEN** 左键点击一行已展开的 thinking 块
- **THEN** 该块恢复单行占位（含字符数），其它 thinking 块与全局展开标志不变

#### Scenario: 流式中点击生效

- **WHEN** thinking 正在流式生成（占位随 delta 增长）时点击该块
- **THEN** 该块展开并继续实时显示后续 delta，其它块与全局展开标志不变

#### Scenario: 全局切换覆盖单块状态

- **WHEN** 单块点击切换后按 `ctrl+t`
- **THEN** 所有 thinking 块跟随全局新状态（单块独立状态被清除）；随后单块
  点击仍可独立切换

#### Scenario: 点击非 thinking 行无动作

- **WHEN** 左键点击 assistant 文本行等非 thinking 行（非工具卡片行）
- **THEN** 界面与所有 thinking 块状态均不变化

### Requirement: 消息区滚动条

消息区 MUST 在右侧最一列渲染垂直滚动条（覆盖内容末列，不额外预留列），
用于指示当前可视区间在全部消息中的位置：内容总行数不超过视口高度时 MUST
不渲染；内容超出视口时，仅在发生**滚动移动**（PageUp/PageDown、鼠标滚轮、
回底等改变可视首行的动作）后瞬态显示，此后 1 秒内无滚动移动则 MUST 隐藏，
期间再次滚动 MUST 重新计时；仅内容增长（如流式追加）而未滚动 MUST NOT
触发显示。拖动条长度与位置 MUST 按内容总行数/可视首行成比例映射（含最小
高度，贴底时位于轨道底端）。滚动条字形与颜色 MUST 对齐 pi：轨道 `│`、
常态拖动条 `┃`、悬停拖动条 `█`，前景色分别对齐主题的 track/thumb 色。
指针悬停于滚动条列的轨道行内时，拖动条 MUST 加粗显示且不受 1 秒计时隐藏，
移开后恢复常态并恢复计时。该列上的左键按下 MUST 被滚动条消费（不穿透为
其下消息内容的点击）：按下即视口跳转/开始拖动，拖动中视口随指针连续变化，
抬起结束。滚动行为本身（PageUp/PageDown、滚轮语义）MUST 保持不变。

#### Scenario: 滚动后瞬态显示随后隐藏

- **WHEN** 消息区内容超出视口高度，用户执行 PageUp 或滚轮上滚
- **THEN** 滚动条立即出现在消息区最右列；1 秒内无进一步滚动则消失，
  期间再次滚动则重新计时

#### Scenario: 内容不足一屏不显示

- **WHEN** 消息区内容总行数不超过视口高度
- **THEN** 无论是否发生过滚动，滚动条均不渲染

#### Scenario: 仅内容增长不触发显示

- **WHEN** 会话处于流式追加、用户未滚动
- **THEN** 滚动条不因内容变长而出现

#### Scenario: 拖动条位置与长度按比例

- **WHEN** 内容远高于视口，用户分别停留在顶部中段与贴底
- **THEN** 拖动条长度与内容/视口比例一致（有下限），位置随可视首行
  成比例移动，贴底时位于轨道底端

#### Scenario: 悬停加粗并常显

- **WHEN** 指针移入滚动条列的轨道行内
- **THEN** 拖动条以加粗字形显示且不被 1 秒计时隐藏；指针移出后恢复
  常态字形并恢复计时

#### Scenario: 列上按下被消费并可跳转/拖动

- **WHEN** 指针位于滚动条列并按下左键（点轨道非拖动条区域）
- **THEN** 该按下不触发其下消息内容的点击，视口跳转到指针对应位置；
  按住拖动时视口随指针连续变化，抬起后结束

### Requirement: 消息区提示跳转与滚动键

消息区 MUST 支持按语义消息边界跳转与绝对定位滚动：`ctrl+↑`（或
`ctrl+shift+↑`）跳至上一条消息起点、`ctrl+↓`（或 `ctrl+shift+↓`）跳至
下一条消息起点；**消息**指用户与 assistant 条目（通知与 `!` bash 执行
不构成跳转点）。已在最早消息时上跳 MUST 归顶；已在最末消息时下跳 MUST
贴底并恢复跟随。`ctrl+home` / `ctrl+end` MUST 分别定位到消息区绝对顶部与
底部（底 = 恢复跟随）。`shift+PageUp` / `shift+PageDown` MUST 按半页滚动
（页长一半、至少 1 行）。补全菜单打开时这些键 MUST 保持既有菜单行为
（菜单整页翻页不变；`ctrl+↑/↓` 不属菜单键，消息区照常跳转）；浮层打开时
MUST NOT 发生跳转（浮层消费焦点键的既有规则）。任何跳转或滚动 MUST 视为
滚动移动（滚动条瞬态显示语义沿用「消息区滚动条」需求）。

#### Scenario: 上一条消息跳转

- **WHEN** 内容多于一屏、视口位于消息区中部，按 `ctrl+↑`
- **THEN** 视口顶移到上一条消息（用户或 assistant）的起始行；连续按到
  最早消息后再按，视口停在绝对顶部

#### Scenario: 下一条消息跳转与端点

- **WHEN** 按 `ctrl+↓` 跳过最末一条消息
- **THEN** 中间时视口顶移到下一条消息起点；已无下一条时视口贴底并恢复
  贴底跟随

#### Scenario: 一键顶底

- **WHEN** 按 `ctrl+home` 或 `ctrl+end`
- **THEN** 视口分别定位到消息区绝对顶部、绝对底部（底部恢复贴底跟随）

#### Scenario: 半页滚动

- **WHEN** 按 `shift+PageUp` 或 `shift+PageDown`
- **THEN** 视口按页长一半滚动（至少 1 行），打断/恢复跟随语义与整页
  滚动一致

#### Scenario: 菜单与浮层下的键位语义

- **WHEN** 斜杠补全菜单打开时按 `shift+PageUp`
- **THEN** 菜单整页翻页（既有行为不变，消息区不半页滚动）；按 `ctrl+↑`
  时菜单不消费该键、消息区照常跳转
- **WHEN** 任意浮层打开时按 `ctrl+↑`
- **THEN** 消息区不跳转（浮层消费焦点键）
