# Spec Delta

## ADDED Requirements

### Requirement: 工具卡片语法高亮

read 与 write 工具卡片展示的文件内容 MUST 按文件扩展名对应的语法着色，
着色风格与聊天区 markdown 代码块一致（同一主题）。扩展名无对应语法或
高亮失败时 MUST 降级为纯文本——内容、行数与折叠行为不变、MUST NOT 报错。
折叠态预览与展开态 MUST 着色一致。着色 MUST 仅作用于前景：工具卡三态
背景面板底色在高亮行上 MUST 保留。edit 卡片的 diff 行着色（红/绿/青）
与其它工具卡片输出 MUST 保持现状（本需求不含 diff 行内变更高亮）。

#### Scenario: read 卡片按语言着色

- **WHEN** read 卡片显示 `.py` 文件内容（折叠预览或展开态）
- **THEN** 关键字/字符串等与普通文本以不同前景色可见区分，风格与聊天区
  代码块一致，且卡片背景面板底色不丢

#### Scenario: 未知扩展名降级纯文本

- **WHEN** read 或 write 卡片的内容文件扩展名无对应语法（如 `.xyzzy`）
- **THEN** 内容照常完整显示为纯文本，行数不变、无报错

#### Scenario: 折叠与展开着色一致

- **WHEN** 同一 read 卡片先折叠（头 10 行预览）后展开
- **THEN** 两态的可见行着色一致（同一语言同一主题）

#### Scenario: write 卡片同样着色且 edit 保持现状

- **WHEN** write 卡片显示新文件内容
- **THEN** 按其扩展名着色；edit 卡片的 `-N`/`+N`/`@@` 行级红绿青着色
  与改动前一致
