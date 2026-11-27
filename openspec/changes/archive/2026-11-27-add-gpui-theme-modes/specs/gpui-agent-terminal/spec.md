# Spec Delta

## ADDED Requirements

### Requirement: 独立窗口主题三模式
GPUI 独立窗口主题 MUST 支持 light / dark / system 三模式：启动默认 system，
跟随操作系统外观并随其变化热切换；light / dark 固定对应模式。mode MUST 为
窗口内存态——不持久化、不读写 pi `settings.json` 的 `theme` 键，重启回默认。
渲染 MUST 由随 mode 解析的两套调色板驱动：语义 token（工具卡三态、通知、
bash/exit、滚动条、文字角色）与 pi 内置 `light.json` / `dark.json` 同源，
面板底色类维持自有值（浅色现值、深色自有值）；工具卡 MUST 随模式取色
（浅色浅卡深字、深色深卡浅字）。框架内建控件 MUST 与所选 mode 同步切换；
宿主嵌入时以框架全局主题注入为准。

#### Scenario: 启动默认跟随系统外观
- **WHEN** 运行 `pi-shell-gpui` 独立窗口且未手动选择模式
- **THEN** mode 为 system，窗口渲染随操作系统外观的深浅变化热切换

#### Scenario: 手动切换立即生效且不落盘
- **WHEN** 在 `/settings` 选择 light 或 dark
- **THEN** 面板、工具卡与内建控件立即按该模式重绘；选择不写入任何文件，重启后回到 system

#### Scenario: 浅色模式工具卡为浅卡
- **WHEN** mode 为 light 且渲染工具卡
- **THEN** 面板与卡片为浅色系（pi light 主题同源）、卡上文字为深色系，不再呈现白面板加深卡的混血

#### Scenario: 深色模式全套深色
- **WHEN** mode 为 dark 且渲染三分区、补全菜单与工具卡
- **THEN** 面板为深色系、工具卡深卡浅字，语义色与 pi dark 主题一致

## MODIFIED Requirements

### Requirement: 输入区简约边框
输入区 MUST 呈现为纯矩形边框：浅色模式下黑色、深色模式下浅色系（与 pi dark
主题边框语义一致），直角、无阴影；未聚焦时边框为 1px，获得焦点（可输入）时
MUST 加粗为 2px。输入区 MUST NOT 渲染框架内建的底色、圆角与聚焦光环。
外观改动 MUST NOT 影响内建编辑语义（插入 / 换行 / 撤销 / 提交）。

#### Scenario: 常态外观
- **WHEN** 输入区未获得焦点
- **THEN** 浅色模式下黑色 1px 直角边框、白色底（与消息区同底）；深色模式下浅色系 1px 直角边框、深色底（与消息区同底），均无圆角、无内建装饰

#### Scenario: 聚焦加粗
- **WHEN** 输入区获得焦点（可输入）
- **THEN** 边框加粗为 2px，其余外观与常态一致

#### Scenario: 编辑语义不变
- **WHEN** 聚焦态下执行插入、换行、撤销与提交
- **THEN** 行为与内建编辑器既有语义一致（外观改动不引入行为差异）

## REMOVED Requirements

### Requirement: 独立窗口浅色模式固定
**Reason**: 独立窗口升级为三模式主题（默认 system、双调色板 pi 同源、随
OS 外观热切换），钉死浅色的过渡形态被完整主题系统取代。
**Migration**: 由「独立窗口主题三模式」需求约束；原「启动即浅色、不随系统
外观变化」与「完整主题系统（深浅热切换、token 化）不在本能力范围」的条款废止。
