# Spec Delta

## ADDED Requirements

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
