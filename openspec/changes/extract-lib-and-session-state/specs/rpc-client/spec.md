# Spec Delta

## Purpose

以库形式提供 `pi --mode rpc` 的连接与消息收发能力：管理子进程生命周期、分离 stdin/stdout/stderr、按命令 id 关联响应、把无 id 事件与响应分流，供诊断探针和未来 TUI 共同复用。

## ADDED Requirements

### Requirement: 库提供 RPC 子进程连接

连接层 MUST 作为库模块提供，能够启动 `pi --mode rpc`（可执行路径可用 `PI_BIN` 覆盖）并建立互相独立的 stdin / stdout / stderr 通道。stdout 按行读取 JSON，stderr MUST NOT 与 stdout 混合，且 MUST 能被调用方单独获取。

#### Scenario: 库客户端启动并收发响应

- **WHEN** 库客户端启动 `pi --mode rpc` 并发送一条 `get_state` 命令
- **THEN** 调用方能在超时内收到对应 `type=response`、`command=get_state` 的成功响应

#### Scenario: 找不到可执行文件时快速失败

- **WHEN** `PI_BIN` 指向不存在的路径并尝试启动
- **THEN** 启动 MUST 返回包含原因的错误而不是挂起，调用方能据此退出或提示

#### Scenario: stderr 独立于 stdout

- **WHEN** 子进程同时向 stdout 写 JSON、向 stderr 写文本
- **THEN** 调用方在事件通道只得到 stdout 的 JSON，stderr 文本从独立通道获得，二者不串扰

### Requirement: 命令按 id 关联响应

客户端 MUST 为发出的命令关联唯一 id（调用方指定或自动生成），并让调用方能够按该 id 等待对应响应。无 id 的顶层消息（会话事件、extension_ui_request 等）MUST NOT 被当作某条命令的响应。

#### Scenario: 响应按 id 路由

- **WHEN** 调用方连续发出两条命令，响应以任意顺序到达
- **THEN** 每个调用都只收到与自己 id 匹配的响应，不被其它 id 或无 id 事件唤醒

#### Scenario: 事件不满足响应等待

- **WHEN** 调用方正在等待某命令响应，期间到达 `agent_start`、`message_update` 等无 id 消息
- **THEN** 这些消息被分派给事件流，命令的响应等待保持未完成直到匹配 id 的响应到达

### Requirement: 非 JSON 行作为证据保留

stdout 上无法解析为 JSON 的行 MUST NOT 导致客户端崩溃或被静默丢弃；MUST 以显式的"解析错误"形态（保留原文）提供给调用方。

#### Scenario: 脏行不致命

- **WHEN** stdout 出现一行非 JSON 文本，随后又是合法 JSON
- **WHEN** 调用方仍能收到那条合法 JSON，且脏行以保留原文的形式可见

### Requirement: 探针迁移为库客户端

现有 `probe` 二进制 MUST 改为使用库的 RPC 连接与收发能力，移除其私有重复实现，且其命令行行为、产物格式与既有测试保持不变。

#### Scenario: 迁移后探针行为不变

- **WHEN** 用库版本的探针运行既有离线测试与一个内置场景
- **THEN** 现有探针测试全部通过，场景产物（transcript/summary/meta）结构与迁移前一致
