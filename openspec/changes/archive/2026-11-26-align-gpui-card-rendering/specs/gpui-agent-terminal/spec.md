# Spec Delta

## MODIFIED Requirements

### Requirement: 内建控件渲染模型
GPUIAgentTerminal MUST 优先以宿主框架内建控件渲染：消息区 MUST 以内建虚拟滚动承载会话条目、助手文本以内建 markdown 渲染（代码高亮随框架特性生效）；仅在无内建对应物处（如工具卡）自绘。卡片与通知的**内容语义**（头行参数、折叠策略、截断、状态行文案等）MUST 来自共享层的中性语义模型，GPUI 侧 MUST NOT 另存一份语义副本。MUST 展示 markdown、thinking 折叠、工具卡、错误/重试提示；不要求与终端 TUI 的换行位置或 cell 坐标一致。

#### Scenario: 会话内容渲染
- **WHEN** 同一会话在 GPUI 组件中打开
- **THEN** 助手 markdown（含代码高亮）、thinking 折叠态、工具卡与错误提示均可见，未知工具不 panic

#### Scenario: 虚拟滚动回看
- **WHEN** 长会话向上滚动
- **THEN** 可回看更早内容，新事件到达不打断回看位置

#### Scenario: 语义与 TUI 同源
- **WHEN** 同一工具调用分别渲染于终端 TUI 与 GPUI 窗口
- **THEN** 两者的头行参数、折叠保留行数、提示行文案、受保护尾部内容一致（换行位置可不同）

### Requirement: 全局键位与内建编辑
全局键位 MUST 映射为现有输入语义并送入共享状态层：enter 提交（流式中携带 steer）、alt+enter 排队提交、esc 流式中断/空闲无操作、ctrl+c 清空输入区、ctrl+t 切换 thinking 全局展开、ctrl+o 切换工具卡全局展开；补全菜单打开时的导航/应用键 MUST 由共享层消费。文本编辑 MUST 交由内建编辑器处理（插入/换行/撤销/移动为其自身能力），不要求与终端 TUI 逐键一致。

#### Scenario: 全局键位语义
- **WHEN** 流式中按 enter、或按 esc / ctrl+c
- **THEN** 分别发送带 steer 的 prompt、发送 abort 并回填队列、清空输入区

#### Scenario: 展开切换键可达
- **WHEN** 空闲时按 ctrl+t 或 ctrl+o
- **THEN** 分别切换 thinking 全局展开与工具卡全局展开，消息区历史与流式内容即时重渲染，按键不落入输入编辑器

#### Scenario: 编辑交内建编辑器
- **WHEN** 用户输入多行文本并撤销
- **THEN** 缓冲内容与光标正确，提交时发送整个缓冲

## ADDED Requirements

### Requirement: 工具卡语义对齐
工具卡 MUST 以与终端 TUI 相同语义渲染：头行含 `●` 状态点、工具名、pi 对齐关键参数（bash 为 `$ 命令`（含 timeout）、read 为行范围、grep 为 `/模式/ in 路径` 等）、状态文字与计时（运行中 `Elapsed`、结束 `Took`）；结果体按工具折叠（bash/powershell 尾 5 行、grep 15、find/ls 20、其余头 10 行）并显示 `... (N more lines, ctrl+o to expand)` 类提示行；bash 的 exit 状态行与各工具截断警告 MUST 不参与折叠、始终可见；面板底色随状态三态（运行灰/失败暗红/成功墨绿）。已有最终结果的卡片 MUST 支持点击切换该卡展开；运行中卡片点击无响应。

#### Scenario: 头行与计时
- **WHEN** 一条 bash 调用执行结束（durationMs=830）
- **THEN** 头行显示 `● bash · $ <命令> · done · Took 830ms` 语义的内容，运行中显示 `running` 与走字的 `Elapsed`

#### Scenario: 折叠与提示
- **WHEN** bash 结果 40 行、处于折叠态
- **THEN** 显示尾部 5 行 + 隐藏行数提示行，exit 状态行仍在其后可见

#### Scenario: 单卡点击展开
- **WHEN** 已有最终结果的工具卡被左键点击
- **THEN** 仅该卡切换展开/收起，其它卡与全局展开标志不变

### Requirement: edit 卡 diff 渲染
edit 工具卡 MUST 以红绿行文本呈现 diff：`-` 前缀旧行红色、`+` 前缀新行绿色、`@@` 行青色（结果无 diff 字段时按 old/new 行级对比降级）。渲染 MUST 使用内建文本样式，MUST NOT 引入框架的文件级 diff 查看组件。

#### Scenario: 结果含 diff
- **WHEN** edit 结果 details.diff 存在
- **THEN** 卡体逐行按前缀着色呈现，界面不引入独立滚动的 diff 面板

#### Scenario: diff 降级
- **WHEN** edit 结果无 diff 字段但有 old/new
- **THEN** 行级对比产出红绿行，卡片不 panic

### Requirement: thinking 块语义对齐
thinking 块 MUST 与终端 TUI 同语义：空块 MUST NOT 占行（折叠态也不显示占位符）；非空块折叠为 `<thinking N chars>` 占位、展开显示完整正文（弱化样式）；每个非空块 MUST 支持点击切换该块展开，全局 ctrl+t 切换时 MUST 清单块级覆盖并即时生效于历史与流式块。

#### Scenario: 空块不占行
- **WHEN** assistant 消息含一个 text 为空的 thinking 块
- **THEN** 消息区不为该块渲染任何行

#### Scenario: 单块点击展开
- **WHEN** 非空 thinking 块被左键点击
- **THEN** 仅该块切换展开/收起，全局标志与其它块不变

### Requirement: 通知行内联渲染
通知（错误/重试/普通提示）MUST 以对话流内联行渲染，含 `[kind]` 前缀：error 红色加粗、retry 黄色、其余弱化斜体；MUST NOT 以自动消失的 toast 弹窗形态呈现。

#### Scenario: 错误通知
- **WHEN** 会话到达 error 类通知
- **THEN** 消息区出现 `[error] ...` 红色加粗行，不弹出浮层

### Requirement: bash 条目语义对齐
`!` bash 条目 MUST 与终端 TUI 同语义：输出保留**尾部** 500 行、超出时在前部显示 `…（已截断 N 行）`；命令行黄色加粗、`（不进上下文）` 标记（excludeFromContext）可见；状态行区分运行中（`⟳ 运行中…（esc 中止）`）、`cancelled`、`exit 0`（绿）、`exit N`（N≠0 红粗）、`done`；`full_output_path` 与 `truncated` 标记可见。

#### Scenario: 长输出截断
- **WHEN** bash 条目输出 600 行
- **THEN** 显示尾部 500 行，前部含 `…（已截断 100 行）`，状态行按 exit_code 着色

#### Scenario: 运行中尾槽可见
- **WHEN** `!cmd` 执行期间收到输出分片
- **THEN** 消息区尾部出现命令行与 `⟳ 运行中…（esc 中止）` 行、输出实时增长，完成后收敛为带 exit 状态行的落盘条目（尾槽消失）

### Requirement: 队列条可见
组件 MUST 在输入区上方渲染待处理队列条：`[queued]`（本地排队）、`[steer]`（steering 队列）、`[queue]`（follow_up 队列）前缀行，最多 2 行、超出显示 `…N more`；队列全空时 MUST NOT 占位。

#### Scenario: 流式排队可见
- **WHEN** 流式中 alt+enter 提交一条 follow_up
- **THEN** 输入区上方出现 `[queue] ...` 行，队列清空后该行消失

### Requirement: extension widget 面板
收到 `setWidget` 的 widgetLines 数据时 MUST 渲染带 ` widget ` 标题边框的面板（青色文字）；收到清除数据时 MUST 移除面板；数据缺失或为 null 时 MUST 清空而非报错。

#### Scenario: widget 设置与清除
- **WHEN** RPC 事件携带 widgetLines 数组、随后携带 null
- **THEN** 面板先出现后消失，消息区与输入区不被挤压至不可见

### Requirement: 运行中计时走字
存在运行中工具时，组件 MUST 每秒触发一次重绘使 `Elapsed` 计时走字；无运行中工具时 MUST NOT 周期重绘。

#### Scenario: 运行中走字
- **WHEN** 一个工具处于 Running 且已运行 3 秒
- **THEN** 卡片计时显示约 3s 并持续递增

#### Scenario: 空闲不空转
- **WHEN** 无运行中工具
- **THEN** 不产生周期性重绘
