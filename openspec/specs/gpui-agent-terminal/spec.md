# gpui-agent-terminal Specification

## Purpose

GPUI 构建模式下的原生 Agent 终端组件：以默认关闭的编译特性门控零成本引入，经 kameo 薄层路由交互 RPC，以 gpui-kit 内建控件原生渲染会话并支持独立窗口运行，v1 与现有终端 TUI 的 P0 功能语义对齐。

## Requirements

### Requirement: GPUI 构建模式的编译期门控
构建 MUST 由默认关闭的特性 `gpui-support` 门控。特性关闭时，构建 MUST NOT 依赖任何 GPUI 渲染框架或 actor 运行时，产物行为与未引入本变更时一致；特性开启时才编入 GPUI 模块树与独立窗口 bin。

#### Scenario: 默认构建不受影响
- **WHEN** 以默认特性集执行 `cargo build` / `cargo test` / `cargo clippy --all-targets`
- **THEN** 不编译 GPUI 模块树、不链接 GPUI/actor 依赖，现有测试全部通过且行为不变

#### Scenario: 开启特性构建 GPUI 模式
- **WHEN** 以 `--features gpui-support` 构建与测试
- **THEN** GPUI 模块与 `pi-shell-gpui` bin 被编入，GPUI 路径测试随矩阵执行

### Requirement: 核心薄层 RPC 路由
GPUI 构建模式 MUST 提供单一 RPC 路由层：拥有 RPC 客户端，交互命令以请求-应答形态转发并返回结果，会话事件以通知形态扇出给订阅方；该层 MUST NOT 承担会话状态归约。

#### Scenario: 命令经路由层往返
- **WHEN** 前端经路由层发送 prompt / abort / set_model 等命令
- **THEN** 路由层转发至 RPC 客户端、按 id 匹配响应并把结果交还请求方

#### Scenario: 事件扇出
- **WHEN** RPC 客户端收到无 id 的会话事件
- **THEN** 路由层把事件原样通知全部订阅前端，状态归约由前端各自完成

### Requirement: 内建控件渲染模型
GPUIAgentTerminal MUST 优先以宿主框架内建控件渲染：消息区 MUST 以内建虚拟滚动承载会话条目、助手文本以内建 markdown 渲染（代码高亮随框架特性生效）；仅在无内建对应物处（如工具卡）自绘。MUST 展示 markdown、thinking 折叠、工具卡、错误/重试提示；不要求与终端 TUI 的换行位置或 cell 坐标一致。

#### Scenario: 会话内容渲染
- **WHEN** 同一会话在 GPUI 组件中打开
- **THEN** 助手 markdown（含代码高亮）、thinking 折叠态、工具卡与错误提示均可见，未知工具不 panic

#### Scenario: 虚拟滚动回看
- **WHEN** 长会话向上滚动
- **THEN** 可回看更早内容，新事件到达不打断回看位置

### Requirement: 全局键位与内建编辑
全局键位 MUST 映射为现有输入语义并送入共享状态层：enter 提交（流式中携带 steer）、alt+enter 排队提交、esc 流式中断/空闲无操作、ctrl+c 清空输入区；补全菜单打开时的导航/应用键 MUST 由共享层消费。文本编辑 MUST 交由内建编辑器处理（插入/换行/撤销/移动为其自身能力），不要求与终端 TUI 逐键一致。

#### Scenario: 全局键位语义
- **WHEN** 流式中按 enter、或按 esc / ctrl+c
- **THEN** 分别发送带 steer 的 prompt、发送 abort 并回填队列、清空输入区

#### Scenario: 编辑交内建编辑器
- **WHEN** 用户输入多行文本并撤销
- **THEN** 缓冲内容与光标正确，提交时发送整个缓冲

### Requirement: 启动探活与错误提示
组件宿主启动时 MUST 拉起 `pi --mode rpc` 并以 `get_state` 探活；pi 不可执行或未就绪时 MUST 展示可读错误并干净结束，不悬挂、不崩溃。

#### Scenario: pi 缺失
- **WHEN** 指定的 pi 二进制不存在
- **THEN** 展示可读错误信息，窗口/组件不崩溃

#### Scenario: 探活失败
- **WHEN** `get_state` 超时或返回失败
- **THEN** 展示可读错误并终止已拉起的子进程

### Requirement: prompt 流式与 steer 语义
用户提交 MUST 经路由层发送 `prompt` 并持续渲染流式中的用户、assistant、thinking 内容；会话流式中提交 MUST 携带 `streamingBehavior:"steer"`，MUST NOT 发送缺省该字段的 prompt。

#### Scenario: 流式渲染
- **WHEN** prompt 提交后会话事件持续到达
- **THEN** 消息区实时增量渲染，无需等待生成结束

#### Scenario: 流式中提交
- **WHEN** 会话处于流式状态时用户提交
- **THEN** prompt 携带 steer 字段发送

### Requirement: 中断、清空与队列恢复
流式中 esc MUST 发送 `abort` 并把排队消息恢复回输入框；空闲时 esc MUST 为无操作（不退出、不清空）；ctrl+c MUST 清空输入区。

#### Scenario: 流式中 esc
- **WHEN** 生成进行中按 esc
- **THEN** 发送 abort，排队消息回填输入框，回到可输入状态

#### Scenario: 空闲 esc
- **WHEN** 空闲状态按 esc
- **THEN** 界面无变化（不退出、不清空）

#### Scenario: ctrl+c 清空
- **WHEN** 输入区非空时按 ctrl+c
- **THEN** 输入缓冲被清空，会话不中断

### Requirement: 多行编辑器（内建）
输入区 MUST 为内建多行编辑器，具备字符插入、多行换行、光标移动、删除与撤销能力（功能语义正确即可）；提交时发送整个缓冲。

#### Scenario: 编辑操作正确
- **WHEN** 用户执行插入、换行、删除、撤销操作
- **THEN** 缓冲与光标状态正确

#### Scenario: 提交整个缓冲
- **WHEN** 多行输入被提交
- **THEN** 发送的 message 为完整多行文本

### Requirement: 三分区布局与状态行
组件 MUST 渲染滚动消息区、输入区与状态指示（model、thinking、tokens/cost）；输入区高度随行数增长但有上限，MUST NOT 挤压消息区致其不可见。

#### Scenario: 布局分区
- **WHEN** 会话有流式内容且输入区为多行
- **THEN** 消息区可见、状态行显示 model 与 tokens，输入区不遮挡消息区

### Requirement: 消息区渲染面（P0）
组件 MUST 渲染 assistant markdown（含代码块高亮）、thinking 折叠态、read/bash/edit 工具卡片与未知工具通用卡（不 panic）、错误与重试提示；MUST 支持消息区滚动回看。

#### Scenario: 工具卡渲染
- **WHEN** 会话包含 read/bash/edit 调用与未知工具调用
- **THEN** 三种卡片按现有语义渲染，未知工具走通用卡且界面不崩溃

#### Scenario: 滚动回看
- **WHEN** 用户向上滚动查看长会话
- **THEN** 可回看更早内容，新事件到达不强制打断回看位置

### Requirement: 斜杠命令查讯型子集（v1）
v1 MUST 支持两类斜杠命令：查讯型（其 TUI 语义为展示查询结果，如 /changelog、/model、/thinking）与可开浮层型（其 TUI 语义为打开浮层，如 /tree、/settings、/hotkeys、/trust、/session、/import、/scoped-models）；两子集之外的已知命令 MUST 给出未支持提示桩，MUST NOT 静默失败；未知命令按普通文本发送。

#### Scenario: 查询命令
- **WHEN** 输入查讯型斜杠命令并提交
- **THEN** 结果按现有 TUI 相同方式展示

#### Scenario: 可开浮层命令
- **WHEN** 输入可开浮层型斜杠命令（无参）并提交
- **THEN** 打开对应浮层，与终端 TUI 一致

#### Scenario: 未支持命令
- **WHEN** 输入两子集之外的已知斜杠命令
- **THEN** 显示未支持提示桩，不静默失败


### Requirement: 独立窗口运行形态
`gpui-support` 构建 MUST 提供独立窗口入口：创建窗口并内嵌 GPUIAgentTerminal 组件；关闭窗口 MUST 退出会话并回收 pi 子进程，进程干净结束。

#### Scenario: 启动独立窗口
- **WHEN** 运行 `pi-shell-gpui`
- **THEN** 打开包含组件的窗口，会话建立且可交互

#### Scenario: 关闭窗口
- **WHEN** 用户关闭窗口
- **THEN** 事件循环退出、pi 子进程被回收，进程干净结束

### Requirement: 共享代码仅加法演进
为 GPUI 路径扩展共享模块（`rpc/`、`session/`、`app/`）时 MUST 只新增 API 与行为分支，MUST NOT 改变既有导出的行为；既有测试套件 MUST 保持全绿。

#### Scenario: 共享模块回归
- **WHEN** 本变更加入共享模块的加法扩展后执行默认特性测试
- **THEN** 全部既有测试通过，未开启 `gpui-support` 的行为与变更前一致

### Requirement: 独立窗口浅色模式固定
独立窗口启动 MUST 显式固定浅色主题模式（不订阅系统外观切换）；组件内硬编码面板底色 MUST 为浅色系，使内建控件文字在浅色主题下清晰可读。完整主题系统（深浅热切换、token 化）不在本能力范围。

#### Scenario: 启动即浅色
- **WHEN** 运行 `pi-shell-gpui` 独立窗口
- **THEN** 主题模式为 Light，不随系统外观变化而切换

#### Scenario: 面板文字可读
- **WHEN** 三分区（消息/状态/输入）与补全菜单在浅色主题下渲染
- **THEN** 各区域文字与面板底色对比清晰可读，无深字深底

### Requirement: 悬浮层渲染
GPUIAgentTerminal MUST 从共享浮层状态渲染全部浮层变体：模型/思考级别/会话/scoped-models 选择器、信息查看、会话树、设置、快捷键、信任确认、导入确认、扩展 select/confirm/input/editor 对话，以遮罩加居中面板呈现；渲染 MUST NOT 改写共享浮层状态，浮层行为以共享层为唯一源。

#### Scenario: 选择器渲染
- **WHEN** 模型选择器打开且列表已加载
- **THEN** 面板显示标题、可选项列表、选中行高亮与计数

#### Scenario: 扩展对话渲染
- **WHEN** RPC 扩展请求入队扩展 select/confirm/input/editor 对话
- **THEN** 对应对话渲染标题与内容（含多行编辑展示），界面不崩溃

#### Scenario: 加载态渲染
- **WHEN** 浮层处于 loading（数据未返回）
- **THEN** 显示加载态，而非空面板或崩溃

### Requirement: 悬浮层按键与滚轮路由
浮层打开时全部按键 MUST 经共享键处理分发且不得落入输入编辑器：Esc 关闭浮层先于流式中断，ctrl+c 保持全局优先（与终端 TUI 分支序一致）；滚轮在浮层打开时 MUST 被忽略，不滚动被遮住的消息区。浮层关闭时键鼠行为保持现状。

#### Scenario: 浮层内导航与关闭
- **WHEN** 浮层打开时按上下/翻页键、enter、esc
- **THEN** 分别移动选中、应用选择、关闭浮层，输入区内容不被这些按键改动

#### Scenario: 浮层下滚轮忽略
- **WHEN** 浮层打开时滚动滚轮
- **THEN** 消息区不滚动、浮层不位移

#### Scenario: ctrl+c 浮层下仍全局
- **WHEN** 浮层打开时按 ctrl+c
- **THEN** 走全局清空输入语义，不被浮层吞掉

### Requirement: 悬浮层入口
浮层入口 MUST 可用：打开键（alt+t 打开思考级别选择器、ctrl+l 打开模型选择器）经拦截进入共享键分发；可开浮层的斜杠命令分发后打开对应浮层；扩展对话由既有 RPC 事件入队。

#### Scenario: 打开键
- **WHEN** 空闲时按 alt+t 或 ctrl+l
- **THEN** 分别打开思考级别选择器与模型选择器

#### Scenario: 斜杠开浮层
- **WHEN** 输入 /tree、/settings 等可开浮层命令并提交
- **THEN** 打开对应浮层（与终端 TUI 一致），不显示"暂不支持"桩
