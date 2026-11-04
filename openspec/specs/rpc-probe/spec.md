# rpc-probe Specification

## Purpose

在开始构建 Rust TUI 渲染层之前，提供一个可重跑的开发期诊断能力：用脚本化场景驱动 `pi-agent --mode rpc`，抓取真实的 JSONL 事件流并按类型归档为 fixture，作为 Rust 客户端事件建模与渲染逻辑的事实依据。

## Requirements

### Requirement: 探针子进程通道

探针 MUST 以子进程方式启动 `pi-agent --mode rpc`，并建立独立的 stdin / stdout / stderr 通道。stdin 用于发送 JSONL command，stdout 按行读取 JSON 对象，stderr MUST 不与 stdout 混合且 MUST 被完整转发到运行产物目录。

#### Scenario: 正常启动并读到首条响应

- **WHEN** 探针启动 `pi-agent --mode rpc` 并发送一条 `get_state` command
- **THEN** stdout 在超时时间内返回一行可解析的 JSON，其 `type` 为 `response` 且 `command` 为 `get_state`

#### Scenario: stderr 不污染 stdout

- **WHEN** pi-agent 在运行期间向 stderr 写入任何内容（日志、警告、trace）
- **THEN** 探针的 stdout JSONL 解析 MUST NOT 因 stderr 输出而失败，且 stderr 原文 MUST 出现在运行产物目录的独立文件中

#### Scenario: pi-agent 不可用时明确报错

- **WHEN** 系统上找不到 `pi-agent` 可执行文件或启动立即失败
- **THEN** 探针 MUST 以非零退出码结束，并在终端与运行产物中给出明确的失败原因，而不是挂起等待

### Requirement: Command 发送与 id 关联

探针 MUST 为每一条发出的 command 自动附加一个调用方可选指定或自动生成的唯一 `id`，并据此把后续到达的 `response` 关联回发起场景步骤。事件（无 `id` 的顶层消息）MUST NOT 被当作响应处理。

#### Scenario: 响应按 id 路由

- **WHEN** 探针在短时间内连续发送两条带不同 `id` 的 command
- **THEN** 每条 `response` 都能根据其 `id` 被归属到正确的发起步骤，即使响应到达顺序与发送顺序不同

#### Scenario: 无 id 事件不进入响应通道

- **WHEN** 等待某条 command 的响应期间收到 `agent_start`、`message_update` 等无 `id` 事件
- **THEN** 这些事件 MUST 被原样记录为事件流，且 MUST NOT 完成或拒绝该 command 的等待

### Requirement: 场景脚本驱动

探针 MUST 支持通过场景定义按顺序执行一系列动作，动作类型至少包含：发送 command、等待特定事件、等待 `agent_settled`、等待固定时长、以及回复下一个 `extension_ui_request`。一个场景 MUST 独立运行并产生独立的产物目录。

#### Scenario: 场景按序执行

- **WHEN** 运行一个包含「发送 prompt → 等待 agent_settled → 发送 get_state」的场景
- **THEN** 三个动作严格按顺序执行，且在 `agent_settled` 出现前不会发送 `get_state`

#### Scenario: 场景步骤超时失败

- **WHEN** 某一步等待的事件在配置的超时时间内未出现
- **THEN** 该场景 MUST 以失败结束，已抓取的事件仍然保留在产物目录，失败原因写入 summary

#### Scenario: 回复扩展 UI 请求

- **WHEN** 场景声明「对下一个 method 为 select 的 extension_ui_request 返回固定 value」
- **THEN** 探针 MUST 在收到匹配请求时发送对应的 `extension_ui_response`，并在事件流中同时记录请求与回复

### Requirement: 事件流持久化与分类

每个场景运行 MUST 产出一个目录，包含：带时间戳与方向标记的原始 JSONL 全量记录、stderr 原文、以及一份 summary。summary MUST 至少统计每种顶层 `type` 的出现次数，并对 `message_update.assistantMessageEvent.type` 与 `tool_execution_*` 做二级分类计数。原始 JSON 字段 MUST NOT 被探针重写或丢弃。

#### Scenario: 原始行可回放

- **WHEN** 场景运行结束后查看原始 JSONL 文件
- **THEN** 文件中的每一行业务行都 MUST 是独立合法的 JSON，且与 pi-agent 实际 stdout 内容一致（允许额外附加时间戳、方向、序号等探针元数据，但原始消息体必须完整保留）

#### Scenario: summary 分类计数准确

- **WHEN** 一个场景产生 3 条 `message_update`（其中 2 个 text_delta、1 个 toolcall_start）和 1 条 `tool_execution_start`
- **THEN** summary 中相应分类的计数分别为 `message_update=3`、`text_delta=2`、`toolcall_start=1`、`tool_execution_start=1`

### Requirement: 内置关键路径场景

探针 MUST 内置一组覆盖关键协议路径的可运行场景，至少包括：纯文本对话、触发一次工具调用、thinking 流式输出、生成中 steer、排队 follow_up、手动 compact、失败的工具调用、扩展 UI 请求。内置场景 MUST 能以单条命令逐个或全部执行。

#### Scenario: 逐个运行内置场景

- **WHEN** 用户通过场景名运行任意一个内置场景
- **THEN** 探针执行该场景并在成功时以退出码 0 结束，产物目录包含完整 JSONL 与 summary

#### Scenario: 全部内置场景批量运行

- **WHEN** 用户要求运行全部内置场景
- **THEN** 探针依次执行每个场景并分别产出目录；任一失败不阻止后续场景执行，最终报告每个场景的成败

### Requirement: Fixture 归档与体积控制

场景产物 MUST 归档到仓库内固定位置，供后续 Rust 渲染层测试读取。归档策略 MUST 保证 fixture 可复现地作为测试输入，并对明显的大字段（例如超长 tool result、图片 base64）定义可配置的截断或省略规则，且截断 MUST 在 summary 中可见。

#### Scenario: 归档产物可被测试加载

- **WHEN** 后续 Rust 测试读取归档目录下的某个场景 JSONL
- **THEN** 测试能逐行解析出带方向的消息记录，并据此重放 pi-agent 的输出

#### Scenario: 大字段被显式截断

- **WHEN** 某条消息包含超过配置阈值的字符串字段
- **THEN** 归档行中该字段按截断规则处理，且 summary 记录被截断的字段路径与原始长度

### Requirement: 文档缺口回填

探针运行产物 MUST 被用于回填 `Docs/rpc-protocol-notes.md` 第 8 节列出的每个待验证问题；每个问题 MUST 被标注为「已验证（附 fixture 场景名）」或「与类型定义不符（附差异说明与 fixture 引用）」，不能在没有运行证据的情况下被关闭。

#### Scenario: 关闭一个待验证问题

- **WHEN** 某场景的 JSONL 给出了某个待验证问题的确凿证据
- **THEN** 文档对应条目更新为已验证状态，并引用提供证据的场景名；无法复现的问题保持未决并说明原因
