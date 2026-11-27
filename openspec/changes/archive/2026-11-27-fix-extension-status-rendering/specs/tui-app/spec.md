# Spec Delta

## MODIFIED Requirements

### Requirement: extension UI 被动方法

`extension_ui_request` 的 `notify` / `setStatus` / `setWidget` / `setTitle` / `set_editor_text` 五种 method MUST 被即时应用且 MUST NOT 发送任何 `extension_ui_response`：notify 显示为可见通知，setStatus 按 `statusKey` 更新状态栏中该 key 的状态格（`statusText` 为 null 只清除该格，多 key 并存并按 key 字母序显示；缺省 `statusKey` 的请求落入默认格），setWidget 在编辑器上方显示 widget 文本（未设置时隐藏该区），setTitle 设置终端窗口标题，set_editor_text 写入主输入框。

#### Scenario: notify 显示且不回复

- **WHEN** 收到 method=notify、message="hello" 的请求
- **THEN** 界面出现该消息的通知显示，且 stdin 没有为它发出 extension_ui_response

#### Scenario: setTitle 与 set_editor_text 生效

- **WHEN** 依次收到 setTitle="proj" 与 set_editor_text="draft" 的请求
- **THEN** 终端标题变为 proj，主输入框内容变为 draft

#### Scenario: setWidget 显示与清除

- **WHEN** 收到 setWidget 带 widgetLines 的请求，随后收到 widgetLines 为 null 的请求
- **THEN** 编辑器上方先显示 widget 文本、后隐藏该区域

#### Scenario: statusKey 分格并存与独立清除

- **WHEN** 依次收到 setStatus{statusKey:"a", statusText:"AAA"}、setStatus{statusKey:"b", statusText:"BBB"}、setStatus{statusKey:"a", statusText:null}
- **THEN** 状态栏先显示 AAA 与 BBB 并存，最后 a 格被清除而 b 格仍显示 BBB

#### Scenario: 缺省 statusKey 落默认格

- **WHEN** 收到不带 statusKey 字段、statusText="busy" 的 setStatus 请求
- **THEN** 状态栏显示 busy，不报错、不 panic

## ADDED Requirements

### Requirement: 扩展状态文本 ANSI 解析渲染

状态栏扩展状态文本（setStatus 的 `statusText`）MUST 被解析为样式化片段渲染：可识别的 SGR 序列（含 24-bit 真彩、256 色、标准 8/16 色、bold/dim/italic/underline 及其 reset）映射为对应样式；未知的 CSI/OSC/APC 序列 MUST 被跳过而不显示；任何转义序列残留（如 `[38;2;...m`）MUST NOT 出现在可见文本中。状态段的宽度截断 MUST 按剥离序列后的可见文本计算，零宽填充字符（如 U+200B）MUST NOT 占用显示宽度。

#### Scenario: 真彩状态文本无残留且保色

- **WHEN** 收到 statusText 为 `\x1b[38;2;0;255;136m25.0 tok/s\x1b[0m` 的 setStatus 请求
- **THEN** 状态栏显示可见文本 `25.0 tok/s` 并带对应真彩前景色，不出现 `[38;2;` 等转义残留

#### Scenario: 未知序列跳过不 panic

- **WHEN** statusText 含未覆盖的 CSI 序列（如 `\x1b[2J`）或 OSC 超链接序列
- **THEN** 未知序列被跳过，其余可见文本正常显示，界面不 panic、不错位

#### Scenario: 截断按可见宽度计算

- **WHEN** 状态段可见文本超出状态行可用宽度
- **THEN** 截断发生在可见文本上（含转义序列的字符串不因序列字符被提前截断），截断处有省略标记
