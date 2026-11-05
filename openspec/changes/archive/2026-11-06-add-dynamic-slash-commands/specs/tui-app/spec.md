# Spec Delta

## ADDED Requirements

### Requirement: 动态斜杠命令补全

应用 MUST 在启动时（及 `/reload` 后）经 `get_commands` RPC 获取动态命令列表
（扩展命令、提示模板、`skill:` 命令，含 name/description/source/sourceInfo）并缓存，
作为 `/` 补全菜单的候选来源之一（与内置 24 个命令合并）。合并规则对齐 pi：与内置命令
**同名的扩展命令 MUST 被过滤**（不出现且不遮蔽内置项）；描述 MUST 带源标签
（`[extension]` / `[prompt]` / `[skill]`，置于描述前）；全局设置 `enableSkillCommands`
为 false 时 MUST 过滤全部 `skill:` 候选（缺省视为 true）。拉取失败 MUST 降级为仅内置
候选且不崩溃；动态命令在菜单中的行为（模糊过滤、预选、Tab/Enter 应用）与内置一致。

#### Scenario: 动态命令出现在补全

- **WHEN** 扩展注册了 `/my-ext`（描述 "Do thing"），用户键入 `/my`
- **THEN** 菜单出现 `my-ext` 候选且描述带 `[extension]` 标签，Tab 应用为 `/my-ext `

#### Scenario: 冲突过滤与 skill 开关

- **WHEN** 扩展注册了与内置同名的 `/tree`，或 `enableSkillCommands=false` 时键入 `/ski`
- **THEN** 同名扩展项不出现在候选中；`skill:` 候选全部被过滤，菜单不显示

#### Scenario: 拉取失败降级

- **WHEN** `get_commands` 请求失败或超时
- **THEN** 补全仍可用（仅内置 24 项），界面无错误弹层

## MODIFIED Requirements

### Requirement: 斜杠命令分发

输入以 `/` 开头且命中本地命令表时，应用 MUST 在本地执行该命令、不把整行发送给模型，并在执行后清空输入框；未命中命令表的 `/xxx` 文本 MUST 作为普通消息照常发送（与 pi 一致）——**其中命中动态扩展命令的文本由 pi 的 `prompt()` 内部拦截执行**（本 TUI 不感知执行细节，仅负责按下方压缩期规则发送）。本地命令集为 pi 的全部 24 个内置命令：`/new`、`/resume`、`/tree`、`/fork`、`/clone`、`/session`、`/name <名称>`、`/compact [指令]`、`/quit`、`/model [搜索]`、`/thinking [级别]`、`/copy`、`/export [路径]`、`/import <路径>`、`/changelog`、`/hotkeys`、`/trust`、`/reload`、`/settings`、`/scoped-models`、`/share`、`/bug`、`/login`、`/logout`。空白输入按普通提交处理（不触发分发）。

#### Scenario: 命中本地命令

- **WHEN** 用户输入 /tree 并回车
- **THEN** 打开会话树浮层，该行不作为消息发送，输入框清空

#### Scenario: 未命中的斜杠文本照常发送

- **WHEN** 用户输入 /some-unknown-command 并回车
- **THEN** 该行作为普通 prompt 发送（进入消息区），行为与非斜杠文本一致

#### Scenario: 斜杠空输入

- **WHEN** 用户只输入 / 并回车
- **THEN** 作为普通 prompt 发送（/some… 未知分支），不 panic；输入全空白时仍不发送

#### Scenario: 扩展命令由 pi 拦截执行

- **WHEN** 用户输入动态扩展命令 /my-ext 并回车
- **THEN** 该行作为 prompt 发送（pi 内部拦截执行、不进模型上下文），输入框清空
