# Spec Delta

## ADDED Requirements

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
