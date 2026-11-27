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

### Requirement: 输入区简约边框
输入区 MUST 呈现为纯矩形边框：浅色模式下黑色、深色模式下浅色系（与 pi dark
主题边框语义一致），直角、无阴影；未聚焦时边框为 1px，获得焦点（可输入）时
MUST 加粗为 2px。输入区 MUST NOT 渲染框架内建的底色、圆角与聚焦光环。
外观改动 MUST NOT 影响内建编辑语义（插入 / 换行 / 撤销 / 提交）。

#### Scenario: 常态外观
- **WHEN** 输入区未获得焦点
- **THEN** 浅色模式下黑色 1px 直角边框、白色底（与消息区同底）；深色模式下浅色系 1px 直角边框、深色底（与消息区同底），均无圆角、无内建装饰

#### Scenario: 聚焦加粗
- **WHEN** 输入区获得焦点（可输入）
- **THEN** 边框加粗为 2px，其余外观与常态一致

#### Scenario: 编辑语义不变
- **WHEN** 聚焦态下执行插入、换行、撤销与提交
- **THEN** 行为与内建编辑器既有语义一致（外观改动不引入行为差异）

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

### Requirement: 全局键位补全（P1）
除既有已路由键外，以下键 MUST 进入共享键分发且 MUST NOT 落入输入编辑器：
`alt+up` 把全部排队消息取回到输入框、`ctrl+p` / `ctrl+shift+p` 循环切换模型
（下一个 / 上一个）、`shift+tab` 循环 thinking 级别、`tab` 在菜单关闭时强制
路径补全。补全菜单打开时这些键 MUST 保持既有菜单行为；浮层打开时 MUST 遵守
浮层消费焦点键的既有规则。

#### Scenario: 队列取回与循环键
- **WHEN** 空闲时分别按 `alt+up`、`ctrl+p`、`shift+tab`
- **THEN** 分别把排队消息取回到输入框、切换到下一个模型、切换到下一个 thinking 级别，且输入区既有文本不被这些按键改动

#### Scenario: 强制路径补全
- **WHEN** 补全菜单关闭时按 `tab`
- **THEN** 打开路径补全菜单（与终端 TUI 一致），按键不落入输入编辑器

### Requirement: 消息区滚动命令对齐
消息区滚动 MUST 由内建虚拟滚动状态承载（唯一滚动真相，不经过应用层滚动
状态）：`PageUp` / `PageDown` 按约一个视口的量回看（宿主虚拟滚动的条目
粒度近似，方向与端点语义 MUST 与终端 TUI 一致，粒度差异记为已知差异）、
`shift+PageUp` / `shift+PageDown` 反向约半页且 MUST 确有移动、`ctrl+home` /
`ctrl+end` 绝对顶 / 底（底 = 恢复贴底跟随）、`ctrl+↑` / `ctrl+↓`
（`ctrl+shift` 同义）按消息边界跳转——消息 = 用户与 assistant 条目，通知与
`!` bash 不构成跳转点；到端点后继续跳转 MUST 归顶 / 贴底恢复跟随。
补全菜单打开时 `PageUp` / `PageDown` MUST 保持菜单整页翻页、`ctrl+↑/↓`
照常跳转消息区；浮层打开时 MUST NOT 滚动。所有滚动键 MUST NOT 改动输入
编辑器内容。

#### Scenario: 整页与半页回看
- **WHEN** 内容超出视口时按 `PageUp`、随后按 `shift+PageDown`
- **THEN** 视口先向回看方向移动约一页、再反向移动约半页（方向正确且确有移动），输入区内容不变

#### Scenario: 消息边界跳转与端点
- **WHEN** 连续按 `ctrl+↑` 直到最早消息、再连续按 `ctrl+↓` 直到最末消息之后
- **THEN** 视口依次移到各消息起点；到端点后分别停在绝对顶部、贴底并恢复跟随

#### Scenario: 一键顶底
- **WHEN** 按 `ctrl+home` 或 `ctrl+end`
- **THEN** 视口分别定位到消息区绝对顶部、绝对底部（底部恢复贴底跟随）

#### Scenario: 菜单与浮层下的滚动键
- **WHEN** 补全菜单打开时按 `PageDown`、浮层打开时按 `ctrl+end`
- **THEN** 菜单整页翻页（消息区不动）；浮层下消息区不滚动

### Requirement: 消息区滚动条
消息区 MUST 具备垂直滚动条以指示可视区间位置：内容总高度不超过视口时 MUST
NOT 显示；超出视口时 MUST 显示，位置与长度随可视区间成比例移动，并支持按住
拖动连续改变视口。滚动条的显示时机与交互表现以宿主内建组件为准，MUST NOT
承诺与终端 TUI 的瞬态显隐 / 悬停加粗语义一致（已知差异）。浮层打开时滚动条
MUST 被遮罩覆盖、不可交互。

#### Scenario: 超视口显示且可拖动
- **WHEN** 内容超出视口，按住滚动条拖动
- **THEN** 滚动条可见，视口随拖动连续变化，抬起后停在对应位置

#### Scenario: 不足一屏不显示
- **WHEN** 内容总高度不超过视口高度
- **THEN** 滚动条不渲染（无论是否滚动过）

#### Scenario: 浮层下不可交互
- **WHEN** 浮层打开时在滚动条位置按住拖动
- **THEN** 消息区视口不变化（遮罩覆盖）

### Requirement: 窗口形态剪贴板复制
`ctrl+x` MUST 把最后一条 assistant 文本复制到启动本窗口的终端剪贴板（经
与终端 TUI 相同的复制通路写启动终端），并给出相同提示；无回复时 MUST 红条
提示。`/copy` MUST 与 `ctrl+x` 经同一出口。`ctrl+v` 文本粘贴 MUST 以宿主内建
编辑器的粘贴行为把剪贴板文本插入光标处；图片粘贴链不在本能力范围（已知差异）。
（注：`/export` 的 jsonl 为本地文件复制，不经剪贴板出口，与 TUI 一致。）

#### Scenario: 复制最后回复
- **WHEN** 会话已有 assistant 回复时按 `ctrl+x`
- **THEN** 该文本进入终端剪贴板并有成功提示；无回复时红条提示

#### Scenario: 文本粘贴
- **WHEN** 系统剪贴板含文本时按 `ctrl+v`
- **THEN** 该文本插入输入区光标处，剪贴板为空时无操作、无错误提示

### Requirement: 斜杠命令全集分发
GPUI MUST 与终端 TUI 同分发全部已知斜杠命令：查讯型展示结果、可开浮层型
打开浮层、本地执行型执行其共享语义、桩型给出同款提示；MUST NOT 因子集
闸门把可执行命令降级为"暂不支持"桩。未知 `/xxx` MUST 按普通文本发送。

#### Scenario: 查讯与浮层命令
- **WHEN** 输入 `/changelog`，或无参 `/model`、`/tree`
- **THEN** 分别展示结果、打开选择器与会话树浮层，与终端 TUI 一致

#### Scenario: 本地执行型命令
- **WHEN** 输入 `/compact`、`/new` 等本地或发送型命令并提交
- **THEN** 与终端 TUI 相同执行，不显示"暂不支持"桩

#### Scenario: 桩与未知命令
- **WHEN** 输入 `/share` 或 `/未知名`
- **THEN** 分别显示与 TUI 一致的桩提示、按普通文本发送

### Requirement: 独立窗口主题三模式
GPUI 独立窗口主题 MUST 支持 light / dark / system 三模式：启动默认 system，
跟随操作系统外观并随其变化热切换；light / dark 固定对应模式。mode MUST 为
窗口内存态——不持久化、不读写 pi `settings.json` 的 `theme` 键，重启回默认。
渲染 MUST 由随 mode 解析的两套调色板驱动：语义 token（工具卡三态、通知、
bash/exit、滚动条、文字角色）与 pi 内置 `light.json` / `dark.json` 同源，
面板底色类维持自有值（浅色现值、深色自有值）；工具卡 MUST 随模式取色
（浅色浅卡深字、深色深卡浅字）。框架内建控件 MUST 与所选 mode 同步切换；
宿主嵌入时以框架全局主题注入为准。

#### Scenario: 启动默认跟随系统外观
- **WHEN** 运行 `pi-shell-gpui` 独立窗口且未手动选择模式
- **THEN** mode 为 system，窗口渲染随操作系统外观的深浅变化热切换

#### Scenario: 手动切换立即生效且不落盘
- **WHEN** 在 `/settings` 选择 light 或 dark
- **THEN** 面板、工具卡与内建控件立即按该模式重绘；选择不写入任何文件，重启后回到 system

#### Scenario: 浅色模式工具卡为浅卡
- **WHEN** mode 为 light 且渲染工具卡
- **THEN** 面板与卡片为浅色系（pi light 主题同源）、卡上文字为深色系，不再呈现白面板加深卡的混血

#### Scenario: 深色模式全套深色
- **WHEN** mode 为 dark 且渲染三分区、补全菜单与工具卡
- **THEN** 面板为深色系、工具卡深卡浅字，语义色与 pi dark 主题一致
