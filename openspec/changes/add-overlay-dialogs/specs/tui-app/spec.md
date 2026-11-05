# Spec Delta

## ADDED Requirements

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
