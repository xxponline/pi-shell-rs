# Spec Delta

## MODIFIED Requirements

### Requirement: 斜杠命令分发

输入以 `/` 开头且命中本地命令表时，应用 MUST 在本地执行该命令、不把整行发送给模型，并在执行后清空输入框；未命中命令表的 `/xxx` 文本 MUST 作为普通消息照常发送（与 pi 一致）。本地命令集为 pi 的全部 24 个内置命令：`/new`、`/resume`、`/tree`、`/fork`、`/clone`、`/session`、`/name <名称>`、`/compact [指令]`、`/quit`、`/model [搜索]`、`/thinking [级别]`、`/copy`、`/export [路径]`、`/import <路径>`、`/changelog`、`/hotkeys`、`/trust`、`/reload`、`/settings`、`/scoped-models`、`/share`、`/bug`、`/login`、`/logout`。空白输入按普通提交处理（不触发分发）。

#### Scenario: 命中本地命令

- **WHEN** 用户输入 /tree 并回车
- **THEN** 打开会话树浮层，该行不作为消息发送，输入框清空

#### Scenario: 未命中的斜杠文本照常发送

- **WHEN** 用户输入 /some-unknown-command 并回车
- **THEN** 该行作为普通 prompt 发送（进入消息区），行为与非斜杠文本一致

#### Scenario: 斜杠空输入

- **WHEN** 用户只输入 / 并回车
- **THEN** 作为普通 prompt 发送（/some… 未知分支），不 panic；输入全空白时仍不发送

### Requirement: model 选择器与模型循环

`ctrl+l` MUST 打开模型选择器（数据来自 `get_available_models`），上下键导航、回车应用选中模型（`set_model`）并使状态行模型名随之更新，esc 取消不做任何切换；`ctrl+p` / `shift+ctrl+p` MUST 分别循环到下一个/上一个模型（`cycle_model`）并同步状态行。模型列表为空或请求失败时 MUST 有可见反馈且不卡死。`/model` 命令 MUST 与 ctrl+l 等价地打开选择器；`/model <搜索>` 对可用模型做精确匹配（`provider/id` 或 `id`，大小写不敏感）并命中时直接 `set_model` 且给出 "Model: …" 状态提示，无命中时回退打开选择器。

#### Scenario: 打开选择并应用

- **WHEN** 用户按 ctrl+l，在列表中导航后按回车
- **THEN** 选择器关闭，RPC 发出 set_model，状态行 model 段变为新模型名

#### Scenario: 取消不切换

- **WHEN** 选择器打开时按 esc
- **THEN** 选择器关闭且不发送 set_model，状态行模型名不变

#### Scenario: 循环切换模型

- **WHEN** 用户按 ctrl+p（或 shift+ctrl+p）
- **THEN** 发送 cycle_model，状态行模型名更新；无可用模型时给出提示且不崩溃

#### Scenario: 斜杠命令入口与精确匹配

- **WHEN** 用户输入 /model 回车，或 /model gpt-5.4 回车
- **THEN** 前者打开模型选择器；后者精确命中时直接切换并显示状态提示，无命中时打开选择器

### Requirement: thinking 级别选择与循环

`shift+tab` MUST 循环 thinking 级别（`cycle_thinking_level`）并使状态行 thinking 段即时更新；MUST 可打开级别列表（`get_available_thinking_levels`）选择并应用（`set_thinking_level`），esc 取消。列表为空或请求失败时有可见反馈。`/thinking` 命令 MUST 等价地打开级别列表；`/thinking <级别>` 精确匹配（大小写不敏感）时直接应用，未知级别 MUST 显示红色错误并列出可用级别。

#### Scenario: 循环级别

- **WHEN** 用户按 shift+tab
- **THEN** 发送 cycle_thinking_level，状态行 thinking 段更新为新级别

#### Scenario: 列表选择级别

- **WHEN** 打开级别列表并选中一项回车
- **THEN** 发送 set_thinking_level，状态行更新；esc 取消时级别不变

#### Scenario: 斜杠命令入口与未知级别

- **WHEN** 用户输入 /thinking high 回车，或 /thinking bogus 回车
- **THEN** 前者直接应用该级别；后者显示红色错误并列出全部可用级别，级别不变

## ADDED Requirements

### Requirement: 未支持命令提示桩

`/share`、`/bug`、`/login`、`/logout` MUST 作为本地命令拦截执行（不发送给模型、清空输入），并以黄色提示说明未实现原因（网络上报类 / 项目不做交互式登录，依赖预登录凭据），界面回到空闲。

#### Scenario: 桩命令提示

- **WHEN** 用户输入 /login 回车
- **THEN** 出现黄条提示"依赖预登录凭据，未实现交互式登录"，该行不进入消息区，输入框清空

### Requirement: 复制与信息展示命令

`/copy` MUST 取最后一条 assistant 文本（`get_last_assistant_text`）经终端转义序列写入系统剪贴板并给出成功提示，无内容时红条提示；`/hotkeys` MUST 打开键位表浮层（分组列出当前全部键位，esc/回车关闭）；`/changelog` MUST 读取 pi 包内 CHANGELOG.md 最新条目渲染进消息区，文件缺失时可见提示且不崩溃。

#### Scenario: 复制最后回复

- **WHEN** 会话已有 assistant 回复时输入 /copy 回车
- **THEN** 黄条/状态提示已复制，回复文本进入剪贴板；无回复时红条提示

#### Scenario: 键位表与更新日志

- **WHEN** 用户依次输入 /hotkeys、/changelog 回车
- **THEN** 前者弹出键位表浮层（esc 关闭），后者在消息区渲染 changelog 内容

### Requirement: 简化设置与模型范围展示

`/settings` MUST 打开简化设置浮层：提供 autoCompact 与 autoRetry 两个开关（分别经 `set_auto_compaction`、`set_auto_retry` 应用并即时生效）以及当前默认模型等配置的只读展示，esc 关闭；`/scoped-models` MUST 打开只读浮层展示当前会话模型与全部可用模型（无写入能力）。两者均为对 pi 完整设置界面的简化，行为差异 MUST 记录在 `Docs/slash-commands.md`。

#### Scenario: 简化设置开关

- **WHEN** 用户输入 /settings 回车，在浮层中切换 autoCompact 开关
- **THEN** 发送 set_auto_compaction，开关状态更新，esc 关闭浮层

#### Scenario: 只读模型范围

- **WHEN** 用户输入 /scoped-models 回车
- **THEN** 出现只读浮层列出当前模型与可用模型列表，esc 关闭，不发送任何写入命令

### Requirement: 配置重读

`/reload` MUST 在空闲时执行本地配置重读（会话目录等设置）并给出绿色完成提示、界面重绘；流式响应或压缩进行中 MUST 以黄条拒绝并说明原因，不中断当前运行。

#### Scenario: 空闲时重读

- **WHEN** 空闲状态下输入 /reload 回车
- **THEN** 本地配置重新加载，出现完成提示，界面正常重绘

#### Scenario: 运行中拒绝

- **WHEN** 流式响应进行中输入 /reload 回车
- **THEN** 黄条提示等待当前响应完成，不发生重读，流式不受影响
