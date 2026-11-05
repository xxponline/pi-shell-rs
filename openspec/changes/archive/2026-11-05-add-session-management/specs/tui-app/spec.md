# Spec Delta

## ADDED Requirements

### Requirement: 斜杠命令分发

输入以 `/` 开头且命中本地命令表时，应用 MUST 在本地执行该命令、不把整行发送给模型，并在执行后清空输入框；未命中命令表的 `/xxx` 文本 MUST 作为普通消息照常发送（与 pi 一致）。本变更的本地命令集：`/new`、`/resume`、`/tree`、`/fork`、`/clone`、`/session`、`/name <名称>`、`/compact [指令]`、`/quit`。空白输入按普通提交处理（不触发分发）。

#### Scenario: 命中本地命令

- **WHEN** 用户输入 /tree 并回车
- **THEN** 打开会话树浮层，该行不作为消息发送，输入框清空

#### Scenario: 未命中的斜杠文本照常发送

- **WHEN** 用户输入 /some-unknown-command 并回车
- **THEN** 该行作为普通 prompt 发送（进入消息区），行为与非斜杠文本一致

#### Scenario: 斜杠空输入

- **WHEN** 用户只输入 / 并回车
- **THEN** 作为普通 prompt 发送（/some… 未知分支），不 panic；输入全空白时仍不发送
