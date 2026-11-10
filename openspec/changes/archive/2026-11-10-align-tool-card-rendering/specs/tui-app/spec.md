# Spec Delta

## MODIFIED Requirements

### Requirement: 工具调用卡片

工具调用 MUST 以结构化卡片渲染，至少 read / bash / edit 三种：read 显示文件路径与内容预览，bash 显示命令与输出，edit 显示红绿 diff；卡片均显示工具名、状态（进行中/成功/失败）与耗时，失败为红色。卡片 MUST 按状态着背景面板（运行灰、成功墨绿、失败暗红），背景 MUST NOT 替代状态文字。未知工具名 MUST 走通用卡片：头行为 `name key="value"` 形式的参数摘要（超长截断，展开态逐参数分行），含状态且不 panic。`ctrl+o` MUST 切换工具输出的展开/折叠。

#### Scenario: read 卡片

- **WHEN** 重放 tool_call fixture（read 成功）或实时出现 read 调用
- **THEN** 卡片显示工具名、文件路径、内容预览与成功状态

#### Scenario: edit 卡片显示 diff

- **WHEN** 出现 edit 工具调用及其结果
- **THEN** 卡片以红/绿呈现变更行，而不是原始 JSON

#### Scenario: 未知工具不崩溃

- **WHEN** 出现名称不在已知集合中的工具调用
- **THEN** 以通用卡片显示名称与状态，不 panic

#### Scenario: ctrl+o 展开折叠

- **WHEN** 用户按 ctrl+o 一次再按一次
- **THEN** 工具输出在完整显示与折叠摘要间切换

#### Scenario: 状态背景面板

- **WHEN** 卡片处于运行中、成功、失败三种状态之一
- **THEN** 卡片背景分别为灰、墨绿、暗红色调，且状态文字（进行中/成功/失败）仍可见

#### Scenario: 未知工具头行参数摘要

- **WHEN** 出现未知工具调用，args 含 command、timeout 等参数
- **THEN** 头行显示 `name key="value"` 形式的参数摘要（超长截断），展开态逐参数分行

### Requirement: write/grep/find/ls 专用工具卡片

write / grep / find / ls 四种工具调用 MUST 以专用卡片渲染：write 显示目标路径与写入内容摘要（折叠态含预览行数与总行数），grep 显示模式与路径（`/pattern/ in path`）及匹配输出行（头行含 glob 与 limit 参数），find 显示模式与搜索路径及结果行（头行含 limit），ls 显示目录条目（头行含 limit）；四者均复用既有头行（状态/耗时）与 `ctrl+o` 展开折叠，结果缺失时降级为通用卡片且不 panic。

#### Scenario: grep 卡片

- **WHEN** 出现 grep 调用（pattern、path）及其结果
- **THEN** 卡片显示模式、路径与输出行（截断时有提示），而非通用 args JSON 摘要

#### Scenario: 结果缺失降级

- **WHEN** 上述任一工具的结果字段缺失或形状未知
- **THEN** 卡片仍显示头行与参数摘要，不 panic

#### Scenario: 四卡共享展开折叠

- **WHEN** 按 ctrl+o 切换
- **THEN** write/grep/find/ls 卡片与其它卡片同步在折叠/展开态间切换

#### Scenario: 头行参数完整

- **WHEN** grep 带 glob/limit、ls 带 limit 发出调用
- **THEN** 头行显示这些参数（如 `limit 50`），与 pi 头行信息一致

## ADDED Requirements

### Requirement: 运行中部分输出与计时

工具执行期间，卡片 MUST 实时显示已到达的部分输出（随 `tool_execution_update` 事件增长，无需等待执行结束）；MUST 显示运行计时 `Elapsed <时长>` 并每秒刷新，结束后 MUST 显示 `Took <时长>`（服务端提供 durationMs 时以服务端值为准）。时长格式 MUST 为 `4.2s`（<1 分钟）、`1m 30s`（<1 小时）、`2h 3m 4s`（≥1 小时），MUST NOT 显示裸毫秒数字。

#### Scenario: 运行中看到输出增长

- **WHEN** bash 命令执行中，多个 `tool_execution_update` 携带递增输出到达
- **THEN** 卡片输出随事件实时增长，无需等待 tool_execution_end

#### Scenario: 运行中计时刷新

- **WHEN** 工具已运行约 5 秒仍未结束
- **THEN** 卡片显示 `Elapsed 5.0s` 左右的计时并持续刷新

#### Scenario: 完成后计时与格式

- **WHEN** 工具以 durationMs=90000 结束
- **THEN** 卡片显示 `Took 1m 30s`，而非毫秒数字

### Requirement: 折叠预览与截断警告

折叠态预览 MUST 按工具保留行数并对齐 pi：bash 保留**尾部** 5 行（exit code 与最终结果在末尾），grep 15 行、find/ls 20 行、write 10 行、其余 10 行（保留头部），且 MUST 显示隐藏行数提示 `... (N earlier/more lines, ctrl+o to expand)`。输出被截断时 MUST 显示带具体数字或路径的警告而非仅省略号：bash 为 `[Full output: <path>]` 与行数/字节截断详情，read 为首行超限/行数/字节三形态，grep/find/ls 为各自 limit 警告。

#### Scenario: bash 折叠保留尾部

- **WHEN** bash 输出 100 行、卡片处于折叠态
- **THEN** 显示末尾 5 行（含 exit code）与 "... (95 earlier lines, ctrl+o to expand)" 提示

#### Scenario: 截断警告带数字

- **WHEN** bash 输出被按行截断且服务端提供了完整输出文件路径
- **THEN** 卡片显示 `[Full output: <path>]` 与 `showing X of Y lines` 具体数字

#### Scenario: 各工具 limit 警告

- **WHEN** grep 达到 matches limit 或 ls 达到 entries limit
- **THEN** 卡片显示对应的 `[Truncated: ... limit]` 警告

### Requirement: 头行参数与分类

工具头行 MUST 显示 pi 对齐的关键参数：bash 为 prompt 样式 `$ <命令>`，有 timeout 参数时附 ` (timeout Ns)`；read MUST 显示行范围（`:1-20`、`:5` 形式），折叠态对 SKILL.md、文档与资源类文件 MUST 显示 compact 分类（`[skill] <目录名>`、`read <kind> <相对路径>`）并附 expand 提示。头行 MUST 同时保留工具名、状态文字与计时信息可见。

#### Scenario: bash 头行与超时后缀

- **WHEN** 发出 bash 调用（command=ls -la，timeout=5）
- **THEN** 头行以 `$ ls -la` 样式显示并附 `(timeout 5s)`，工具名与状态仍可见

#### Scenario: read 行范围与 compact 分类

- **WHEN** read 带 offset=5 limit=20 调用，或读取 SKILL.md
- **THEN** 头行显示 `:5-24` 行范围；SKILL.md 显示 `[skill] <目录名>` 分类与 expand 提示
