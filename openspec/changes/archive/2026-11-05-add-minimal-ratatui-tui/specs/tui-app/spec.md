# Spec Delta

## Purpose

在通用终端中以 ratatui 提供最小但可用的交互界面：管理终端生命周期与事件循环，把键盘输入转为 RPC prompt，把无头 session 状态实时渲染为滚动对话，并支持流式显示与中断。

## ADDED Requirements

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
