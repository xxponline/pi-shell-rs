# Design

## Context

阶段 2/3 已交付：三区布局 + chat/markdown/tools 纯函数渲染 + `key_intent`/`apply_intent`
纯键位映射 + RpcClient（含 `send_raw` 能力在 lib 层）。当前 `extension_ui_request` 事件
到达 App 后被 `SessionState::handle` 的 `_ => {}` 静默忽略；选择器与弹层完全不存在。

已勘察的事实：
- pi 模型选择器：`app.model.select` = ctrl+l，打开 `ModelSelector`（数据 `get_available_models`）。
- pi thinking 选择器：由 **`/thinking` 斜杠命令**触发（无参开列表、带参直设）；键位只有
  shift+tab 循环（`app.thinking.cycle`）。我们没有斜杠命令基础设施 → 需自选开列表键。
- `ctx.ui.editor` 是**终端内多行编辑组件**（`ExtensionEditorComponent`，Ctrl+G 可跳外部
  $EDITOR），不是直接开 $EDITOR。
- grep/find/ls 结果形状：主要为 `content` 文本行 + `details` 元数据（如
  `linesTruncated`），**无 structuredContent**；args 为 `pattern/path/glob/limit`。
- write 结果（tool_edit fixture 实测）：`content: [{text: "Successfully wrote to <path>"}]`，
  args 含 `path/content`。
- 既有 `extension_ui` fixture 已捕获 select/confirm/notify 真实请求形状，可离线回放。

## Goals / Non-Goals

**Goals:**
- 一套 overlay 框架（焦点路由 + 居中渲染 + 恢复）供全部弹层复用。
- model/thinking 选择器 + 模型循环；extension UI 9 种 method 全覆盖；4 种专用工具卡片。
- 所有弹层状态与渲染保持纯函数可测；extension 请求-响应往返用假 pi 端到端验证。
- 不新增 crate、不改 session 层归约语义。

**Non-Goals：**
- session 选择器 / 会话树 / 新建·重命名·删除 / compact（4b）。
- 外部编辑器 `$EDITOR`（ctrl+g）与斜杠命令基础设施。
- 选择器输入即筛（fuzzy filter）——先导航选择，filter 后置。
- 图片、主题、mermaid。

## Decisions

### 1. overlay 框架：App 内 `Overlay` 枚举 + 焦点拦截

```rust
enum Overlay {
    ModelSelect { items: Vec<ModelItem>, cursor: usize },
    ThinkingSelect { levels: Vec<String>, cursor: usize },
    ExtSelect   { id: String, title: String, options: Vec<String>, cursor: usize },
    ExtConfirm  { id: String, title: String, message: String },
    ExtInput    { id: String, title: String, placeholder: String, buffer: String },
    ExtEditor   { id: String, title: String, lines: Vec<String>, row: usize, col: usize },
}
```

- **焦点路由**：`handle_key` 开头——`ctrl+c` 永远全局；`Overlay::Some` 时把按键交给
  `overlay_key()`（返回是否消费），消费则不再进 `key_intent`/输入框；这样主界面键位
  表零改动。
- **渲染**：`overlay.rs` 纯函数 `centered_rect(w, h, area)` + `Clear` 打底 + 各形态
  widget（列表/确认/单行输入/多行编辑），`view::render` 在最上层调用；底层照常渲染
  （spec 要求浮层存在时消息区/状态行可见）。
- **数据异步加载**：打开选择器时不能阻塞事件循环 → 沿用 dequeue 的"一次性线程 +
  回报通道"模式，新增 `UiEvent::AsyncData(kind, Value)`（kind = models / thinking_levels /
  model_applied）。`set_model`/`cycle_model` 的响应同样经此回来更新 `state.model`。

### 2. 键位（对齐 pi + 一个自选键）

| 键 | 行为 | 来源 |
|---|---|---|
| ctrl+l | 打开 model 选择器 | pi `app.model.select` ✓ |
| ctrl+p / shift+ctrl+p | 模型循环 next/prev | pi `app.model.cycle*` ✓ |
| shift+tab | thinking 级别循环 | pi `app.thinking.cycle` ✓ |
| **alt+t** | 打开 thinking 级别列表 | **自选**（pi 走 `/thinking`，我们无斜杠设施；记入已知差异） |
| 弹层内 ↑/↓/Enter/Esc | 导航/确认/关闭 | 通用 |

### 3. extension UI 分发（App 层，不进 session）

事件循环里在 `state.handle(&raw)` **之前**截获 `extension_ui_request`：

- **对话 4 种**（select/confirm/input/editor）→ 构造 `Overlay` 压入**单槽队列**
  `VecDeque<Overlay>`（pi 的 `withUIPrompt` 也是串行的；同刻多请求排队逐个处理），
  携带原请求 id。提交/取消时经 `CommandSender::send_raw`（lib 新增，与 RpcClient.send_raw
  同源走 PumpMessage::Raw）回发 `extension_ui_response`。
- **被动 5 种** → 立即应用、**不回发**：
  - notify → `App.notice`（复用现有状态行瞬时提示，按 notifyType 着色 info 默认色/
    warning 黄/error 红）
  - setStatus → `App.status_text: Option<String>`（状态行尾部显示）
  - setWidget → `App.widget_lines: Option<Vec<String>>`（编辑器上方独立区，布局加
    `Length(widget_h)`，null 清除）
  - setTitle → `crossterm::execute!(stdout, SetTitle(..))`（绕过 ratatui 直写）
  - set_editor_text → 覆盖 `App.input`
- editor 多行键位：**Enter=提交，alt+enter=换行**（与主输入 alt 语义一致；PTY 验证
  crossterm 对 `\x1b\r` 的解析，不行则降级 ctrl+j 并记录差异），Backspace 行首回并上一行。

### 4. 选择器行为细节

- 打开：`AsyncData` 未到前显示 "loading…" 占位项；到达填充 items。
- 应用：`set_model {provider, modelId}`（Model 项存 provider+id，显示 name）；
  响应 Model 数据 → 更新 `state.model`（复用 apply 思路，直接赋显示名）。
- `cycle_model` 响应 `{model,...}|null`：null → notice "无可用模型"。
- 取消/失败：Esc 关闭不发命令；请求失败 → `SendFailed` 通道 → notice（已有路径）。
- thinking：`cycle_thinking_level` 响应 `{level}|null`；`set_thinking_level` 成功后
  `thinking_level_changed` 事件会同步 state（无需手动），但乐观先更新避免闪烁。

### 5. 工具卡片扩展（tools.rs 增 4 臂）

| 工具 | 头行关键参数 | 结果体 |
|---|---|---|
| write | path | args.content 预览（前几行）→ 退回 result text |
| grep | `/pattern/ in path` | result text 行 + `details.linesTruncated` 截断提示 |
| find | `pattern in path` | result text 行 |
| ls | path | result text 行 |

- `key_arg` 扩展 write 已支持（path），grep/find/ls 增加专用头行参数函数。
- 结果缺失 → 走现有 generic 降级（spec 场景）。
- **不新增探针场景**：四者结果均为纯文本行（形状简单且防御式读取已兜底），
  用合成单测覆盖；若 PTY 验收发现真实差异再补 fixture（设计余量）。
- 折叠行数沿用阶段 3 的 3 行（不改既有 spec）。

### 6. 测试策略

- **纯函数/状态单测**：overlay 焦点路由（弹层吃键不漏进输入框）、列表导航/确认/取消、
  多行编辑器（插入/换行/回并/提交）、被动 5 method 的状态效果 + "不回发" 断言
  （用可观察的 `send_raw` 记录 mock——通过 CommandSender 不可 mock，改为断言
  App 状态变化 + PTY 端到端数响应条数）、4 种卡片渲染（合成 shape + write 用
  tool_edit fixture）。
- **回放**：既有 `extension_ui` fixture 回放 select/confirm/notify 请求 →
  App 状态机产出正确浮层与回复载荷。
- **假 pi PTY 端到端**：脚本化 select/confirm/input/editor 往返 + notify/setTitle/
  set_editor_text 生效 + 无响应 method 零回发（脚本记录收到的 stdin 行断言）。
- **真机**：模型选择器 ctrl+l、thinking alt+t/shift+tab 与真实 pi 联调。

## Risks / Trade-offs

- **[alt+t 键位与 pi 不一致]** → 已知差异记入 tui-usage；若与用户习惯冲突可改配置化（后置）。
- **[多行编辑器过重]** → 最小实现（无光标移动到行尾外的历史/剪贴板），只满足 editor
  method 的 prefill 编辑与提交；不做 pi 2483 行编辑器的复刻。
- **[弹层队列串行]** → 同刻多请求排队，极端情况下后到请求等待前一个被用户处理；
  与 pi 行为一致，可接受。
- **[setTitle 绕过 ratatui 直写 stdout]** → 与 raw mode 兼容（纯 CSI 序列），
  退出时不恢复标题（记录差异；恢复属锦上添花后置）。
- **[AsyncData 竞态]**（切换选择器期间旧数据到达）→ kind+序号校验：仅当前 overlay
  期望的数据被采纳，过期丢弃。
- **[grep/find/ls 无真实 fixture]** → 防御式渲染 + 合成测试；PTY 验收清单含一项
  真实 grep 卡片检查，发现差异再补 probe 场景。

## Migration Plan

纯增量：新 `overlay.rs`/`dialogs` 分发、tools 4 臂、App 键路由改造（key_intent 前置
overlay 拦截）。顺序：overlay 框架+焦点路由 → extension 分发与被动 method →
对话 4 method+editor → 选择器（model/thinking/cycle）→ 工具卡片 → 文档/验收。
回滚即 revert，无数据迁移。

## Open Questions

- `AsyncData` 的"过期数据丢弃"用序号还是 overlay 身份比对——实现时取更简者
  （不影响结构与规格）。
- 选择器是否要在首版带输入即筛：默认不做（Non-Goal），PTY 验收后若列表过长体验差
  再评估，作为 4b 增量。
