# Spec Delta

## ADDED Requirements

### Requirement: 输入历史浏览

GPUI 输入区的 Up/Down MUST 按与终端 TUI 相同的共享历史语义分派
（`EditorBuffer` 同一状态与规则）：空编辑器、浏览中、或光标位于首行且
列 == 0 等分派条件成立时 MUST 交共享层翻历史并把结果回写内建编辑器；
首行行中的首次 Up MUST 先把光标移到行首（与 TUI 一致）；非浏览态在
末行 MUST 到行尾（与 TUI 一致）；其余情形 MUST 放行内建编辑器原生光标
移动。补全菜单打开时 Up/Down MUST 仍由共享层作菜单导航；浮层打开时
MUST 遵守浮层既有键路由。历史的重建/入栈/草稿捕获与还原/退出浏览
语义与 TUI 一致；到界 MUST no-op。

#### Scenario: 空输入翻历史并可回空
- **WHEN** 输入框为空且历史非空，按 Up 载入最新条目，再按 Down 翻过最新
- **THEN** 输入框依次显示历史条目（光标在开头）、随后还原为空（草稿）

#### Scenario: 分派条件外放行原生移动
- **WHEN** 多行输入中光标不在首行时按 Up、或不在末行时按 Down（均未在浏览态）
- **THEN** 按内建编辑器原生光标移动处理，历史不翻动

#### Scenario: 菜单与浮层优先
- **WHEN** 补全菜单打开时按 Up、浮层打开时按 Up
- **THEN** 菜单选中项上移（历史不动）；浮层打开时按既有键路由处理、历史不翻动

#### Scenario: 浏览中内容变更退出
- **WHEN** 浏览历史条目时在输入框键入字符
- **THEN** 浏览态结束，条目文本就地成为普通草稿（回写内建编辑器且不再跳变）

## MODIFIED Requirements

### Requirement: 全局键位与内建编辑
全局键位 MUST 映射为现有输入语义并送入共享状态层：enter 提交（流式中携带 steer）、alt+enter 排队提交、esc 流式中断/空闲无操作、ctrl+c 清空输入区、ctrl+t 切换 thinking 全局展开、ctrl+o 切换工具卡全局展开；补全菜单打开时的导航/应用键 MUST 由共享层消费。文本编辑 MUST 交由内建编辑器处理（插入/换行/撤销/移动为其自身能力；Up/Down 的历史分派例外由「输入历史浏览」需求定义），不要求与终端 TUI 逐键一致。

#### Scenario: 全局键位语义
- **WHEN** 流式中按 enter、或按 esc / ctrl+c
- **THEN** 分别发送带 steer 的 prompt、发送 abort 并回填队列、清空输入区

#### Scenario: 展开切换键可达
- **WHEN** 空闲时按 ctrl+t 或 ctrl+o
- **THEN** 分别切换 thinking 全局展开与工具卡全局展开，消息区历史与流式内容即时重渲染，按键不落入输入编辑器

#### Scenario: 编辑交内建编辑器
- **WHEN** 用户输入多行文本并撤销
- **THEN** 缓冲内容与光标正确，提交时发送整个缓冲
