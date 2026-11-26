# Spec Delta

## ADDED Requirements

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

## REMOVED Requirements

### Requirement: 斜杠命令查讯型子集（v1）
**Reason**: P0 子集闸门是 GPUI 初期的范围收缩；共享分发与窗口剪贴板出口
就绪后，全集分发取而代之。
**Migration**: 全部已知斜杠命令改由「斜杠命令全集分发」需求约束，行为与
终端 TUI 一致；原「未支持命令给桩」的子集特例废止。
