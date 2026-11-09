# Spec Delta

## MODIFIED Requirements

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
