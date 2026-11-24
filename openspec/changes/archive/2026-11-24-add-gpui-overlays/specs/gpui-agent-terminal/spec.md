# Spec Delta

## ADDED Requirements

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

## MODIFIED Requirements

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
