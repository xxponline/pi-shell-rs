# Spec Delta

## ADDED Requirements

### Requirement: 输入区简约边框
输入区 MUST 呈现为纯矩形边框：浅色（lighting）模式下黑色、直角、无阴影；未聚焦时边框为 1px，获得焦点（可输入）时 MUST 加粗为 2px。输入区 MUST NOT 渲染框架内建的底色、圆角与聚焦光环。外观改动 MUST NOT 影响内建编辑语义（插入 / 换行 / 撤销 / 提交）。

#### Scenario: 常态外观
- **WHEN** 输入区未获得焦点
- **THEN** 黑色 1px 直角边框、白色底（与消息区同底），无圆角、无内建装饰

#### Scenario: 聚焦加粗
- **WHEN** 输入区获得焦点（可输入）
- **THEN** 边框加粗为 2px，其余外观与常态一致

#### Scenario: 编辑语义不变
- **WHEN** 聚焦态下执行插入、换行、撤销与提交
- **THEN** 行为与内建编辑器既有语义一致（外观改动不引入行为差异）
