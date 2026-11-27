# Spec Delta

## ADDED Requirements

### Requirement: 输入历史浏览

输入区焦点时 MUST 支持上下箭头浏览提交历史：历史 = 当前会话（分支）
transcript 内的 user 消息，经幂等重建维护（启动/树导航/reload 先清后灌，
提交时追加；上限 100、相邻同文去重——修正 pi 渲染回灌无清除导致的重复
入栈缺陷）。Up 在首条可视行且（空编辑器 | 浏览中 | 光标列==0）时 MUST
进入/上翻历史，否则 MUST 回行首；Down 在末条可视行且浏览中时 MUST 下翻，
翻过最新 MUST 还原进入前的草稿（含光标与滚动），非浏览态在末行 MUST 到
行尾。首次进入 MUST 捕获草稿；内容变更（打字/粘贴/删除）与提交 MUST
退出浏览（草稿丢弃、浏览文本留驻为普通草稿），纯光标移动 MUST NOT 退出；
到界 MUST no-op。补全菜单打开时 Up/Down MUST 仍归菜单。

#### Scenario: 空输入直入历史
- **WHEN** 输入区为空且历史非空，按 Up、再按 Up、到最旧后再按 Up
- **THEN** 依次载入最新、次旧条目（光标在开头），到最旧后无变化

#### Scenario: 首按回行首、次按进历史
- **WHEN** 光标位于首行行中（列 > 0）时按 Up，随后再按 Up
- **THEN** 第一次光标回到行首且不载入历史，第二次载入更旧条目

#### Scenario: 草稿捕获与还原
- **WHEN** 有未提交草稿时按 Up 进入浏览，再按 Down 逐条翻回直到翻过最新条目
- **THEN** 进入浏览前的草稿（文本与光标位置）完整还原

#### Scenario: 内容变更退出浏览
- **WHEN** 浏览历史条目时键入一个字符
- **THEN** 浏览态结束，该条目文本就地成为普通草稿（不再随 Up/Down 跳变）

#### Scenario: 幂等重建无重复
- **WHEN** 树导航或 reload 重新渲染会话后（此前同会话已提交过消息），按 Up 翻历史
- **THEN** 历史恰为当前 transcript 的 user 消息各一条、顺序正确、无重复

#### Scenario: 补全菜单优先
- **WHEN** 补全菜单打开时按 Up
- **THEN** 菜单选中项上移，历史不翻动

## MODIFIED Requirements

### Requirement: 基础输入编辑

输入区 MUST 是带光标的多行编辑器：支持字符插入、多行换行（`shift+enter` / `ctrl+j`）、光标移动（←→/ctrl+b/f 字符级、↑↓ 跨行、home/ctrl+a 行首、end/ctrl+e 行尾、alt+←→ 词级）、删除（backspace、delete 在非空缓冲时向后删字符、ctrl+w/alt+backspace 后删词、alt+d 前删词、ctrl+u 删至行首、ctrl+k 删至行尾）。提交时整个缓冲（含换行）作为一条消息发送。输入区高度 MUST 随行数增长但有上限，不挤压消息区导致其不可见。输入历史行为由「输入历史浏览」需求定义；Tab 补全存在，其行为由 `path-completion`（纯路径）、`slash-autocomplete`（斜杠）、`file-completion`（`@` 文件）能力定义。超长内容 MUST NOT 导致界面崩溃或布局错乱。

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
