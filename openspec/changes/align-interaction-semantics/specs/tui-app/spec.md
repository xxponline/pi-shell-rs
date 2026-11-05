# Spec Delta

## MODIFIED Requirements

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

### Requirement: 基础输入编辑

输入区 MUST 是带光标的多行编辑器：支持字符插入、多行换行（`shift+enter` / `ctrl+j`）、光标移动（←→/ctrl+b/f 字符级、↑↓ 跨行、home/ctrl+a 行首、end/ctrl+e 行尾、alt+←→ 词级）、删除（backspace、delete 在非空缓冲时向后删字符、ctrl+w/alt+backspace 后删词、alt+d 前删词、ctrl+u 删至行首、ctrl+k 删至行尾）。提交时整个缓冲（含换行）作为一条消息发送。输入区高度 MUST 随行数增长但有上限，不挤压消息区导致其不可见。不提供输入历史与 tab 自动补全。超长内容 MUST NOT 导致界面崩溃或布局错乱。

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

## ADDED Requirements

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
