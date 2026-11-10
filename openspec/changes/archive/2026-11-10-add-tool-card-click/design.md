# Design

## Context

前置 `add-mouse-wheel-scroll` 落地后：捕获已开、`Down`/`Up` 在 `event.rs`
被丢（其测试钉住本 change 的边界），本 change **不碰 terminal.rs**，只做
纯逻辑三层：命中、分发、单卡态。已核实的实现事实：

- `Block::ToolCall { id, name, args_json, tool }` 带 id；`tool` 内嵌
  `ToolExecution{ result: Option<Value>, .. }`——"有结果" = `result.is_some()`。
- `tool_lines(name, args_json, tool, expanded: bool)` 的 `expanded` 入参
  **已存在**（当前恒传 `flags.expand_tools`）——单卡态只需换这个入参来源。
- `chat_lines` → `append_blocks` 同时服务历史 transcript 与 in-flight；
  `!` bash 走独立 `append_bash`（不在本 change 命中范围）。
- pi 语义已核实（`interactive-mode.ts:4737-4749`）：
  `setToolsExpanded` **遍历所有卡片广播绝对状态**（`child.setExpanded(v)`），
  即全局切换会**清除**单卡独立状态；新卡片创建时以当前全局值初始化
  （各 `setExpanded(this.toolOutputExpanded)` 创建点）。
- 反馈回写先例：`add-message-scrollback` 的 `render → Cell → draw 存回`。

动机见 proposal.md；行为契约见 specs/tui-app「工具卡片点击展开」。

## Goals / Non-Goals

**Goals:**

- 规格四场景落地（点开单卡、点收单卡、运行中无效、Ctrl+O 覆盖单卡态）。
- 命中与分发全为可单测纯逻辑（无 TTY）；渲染管线只多一路 meta 传播。
- 与 Ctrl+O 的交互**逐字对齐 pi 广播语义**（proposal 明确留给 design 的点）。

**Non-Goals:**

- **滚动条（可视化/悬停）——暂不做**（roadmap 挂账保留）。
- hover 高亮/光标反馈、拖拽选择语义（`Up`/`Moved`/`Drag` 仍丢弃）。
- `!` bash 卡片、会话树/选择器/补全菜单的点击导航。
- 右键粘贴、scroll-to-end 指示器点击、embed 宿主点击注入。

## Decisions

### D1 单卡展开态：`App.user_tool_expanded: HashMap<String, bool>` 覆盖表

- **生效值**：`override.get(id).unwrap_or(expand_tools)`——无覆盖跟随全局。
- **点击**：`override.insert(id, !当前生效值)`（绝对值，非翻转位）。
- **Ctrl+O**：`expand_tools = !expand_tools; user_tool_expanded.clear();`
  ——清表 + 翻全局 ≡ pi 的"遍历广播 `setExpanded(新全局值)`"（既有卡全部
  被写成新全局；此后新建卡默认新全局 ✓ 两侧行为逐点等价）。
- **否决 XOR 变体**（`生效 = global XOR toggled.contains(id)`）：Ctrl+O 后
  被点过的卡会相对新全局**反向**（全局展开时它反而收起），语义反直觉且
  不符 pi。
- `tool_lines` 的 `expanded` 入参改取该生效值；折叠策略（bash 尾 5 等）
  在 `expanded=false` 路径原样生效，点击不触碰 `collapse()`。

### D2 命中表管道（核心）：meta 并行穿过 chat_lines 与 wrap，产出**绝对屏行**区间

```
chat_lines(state, flags, overrides) -> (Vec<Line>, Vec<Option<String>>)
    │  仅 Block::ToolCall 分支、且 tool.result.is_some() → Some(id)
    │  （门控前移到建表：运行中卡不入表 → 分发无需回查 state）
    ▼
wrap_for_viewport(lines, w, h, scroll, meta)
    │  换行时 meta 随 logical→screen 传播（一条逻辑行的全部子行同 meta）
    │  在 drain 窗口之前、全量 screen 空间扫描 → Vec<CardHit{start,end,id}>
    ▼
render -> (ChatScroll, Vec<CardHit>)  ──draw Cell──>  App 存两样（同帧一致）
```

- **为何绝对屏行而非窗口内行号**：流式期间内容只在**底部追加**，既有屏行
  绝对号不移位——"上一帧 hits + 上一帧 top"在两次绘制的间隙点击仍然精确；
  窗口内行号会随贴底下移而漂移。
- **同帧一致性**：`scroll.top` 与 `hits` 同一次 render 产出、同一次 draw
  存回，点击时 `abs = top + (y - 1)` 二者天然同帧。批内乱序（RPC 先改
  state、点击后处理）靠上述 append-only 性质兜底；`load_messages` 整段
  置换时旧 id 查不到新表 → 无害（id 全局唯一，覆盖表晚一帧重建）。
- meta 类型 `Option<String>`（每逻辑行克隆 id）：与既有"每帧全量重建
  所有 Line"同阶开销，不值得引入索引间接层。

### D3 坐标映射与命中判定

- 布局：`y=0` 恒为 1 行 header，聊天区 `y ∈ [1, 1+viewport)`；
  `viewport`/`top` 均取自上一帧反馈（`ChatScroll` 已存 viewport ✓）。
- `card_at(x, y)`：`row = y-1`，`row >= viewport → None`；
  `abs = top + row` → 线性扫 `hits` 找 `start <= abs < end`。
  **只判行不判列**（卡片行按满宽语义，列越界在文本右侧留白也视为命中
  卡片行——命中区域 = 卡片占用的整行）。
- `overlay.is_some()` → 分发处直接忽略（浮层遮挡，与滚轮 D4 同规则）。
- 线性扫描：卡片数百级、点击低频，微秒级——不做二分。

### D4 事件转发：仅左键 `Down` → `UiEvent::Click { x, y }`

- `event.rs`：`MouseEventKind::Down(MouseButton::Left)` → `Some(Click)`
  （crossterm 已给 0-based 坐标）；`Up`/`Moved`/`Drag`/右键/滚轮横向 →
  `None`。**act-on-press**：单事件、PTY 注入一条序列；抬起与拖拽不参与
  （选择语义挂账）。
- `add-mouse-wheel-scroll` 中钉边界的测试（`Down/Up 仍丢`）随本 change
  改名/更新为"仅左键 Down 转发、其余仍丢"——tasks 组 3 显式包含此改写，
  不静默改变前置 change 的断言语义。
- 不动 `terminal.rs`（捕获由前置 change 承担）。

### D5 渲染反馈扩为二元组

`view::render` 返回值从 `ChatScroll` 扩为 `(ChatScroll, Vec<CardHit>)`
（或等价小结构体）；`draw` 沿用前置 change 的 `Cell` 收集模式，闭包外
回写 `self.scroll` 与 `self.card_hits`。`ViewUi` 增
`tool_expanded: Option<&'a HashMap<String, bool>>`（`None` = 无覆盖，
保住 `derive(Default)` 与既有 `..Default::default()` 测试构造）。

### D6 `chat_lines` 签名与波及面

`chat_lines(state, flags, overrides: &HashMap<String, bool>)` 加**独立参数**
（不给 `RenderFlags` 加生命周期——其 `Copy` 与全签名波及面更大）；
`append_blocks` 同步加 meta 输出参数。波及：`view.rs::render_chat`、
`chat.rs`/`view.rs` 既有测试构造——编译期即全部暴露，逐处补 `&HashMap::new()`。

## Risks / Trade-offs

- **[批内事件乱序：state 已变、hits 还是上一帧]** → append-only 增长下
  绝对行号不移位（D2）；整段置换走"旧 id 无害"路径——均已在 D2 分析。
- **[每帧 meta/ hits 分配]** → 与既有全量 wrap 同阶；若长会话未来卡顿，
  与增量渲染一并另立变更，不在本处预优化。
- **[前置未落地时无法编译验证]** → 两 change 串行 apply（组 3 显式声明
  依赖前置的 event.rs 形态）；本 change tasks 不含 terminal.rs 内容。
- **[点击误触（无 hover 预览)]** → 语义最小化（仅卡片行、仅左键按下、
  仅结果卡）；hover 反馈属挂账，后补不影响本契约。

## Migration Plan

无数据/协议影响，纯输入通道 + 渲染反馈扩展。回滚 = revert 实现提交
（spec delta 随归档同步）。按 tasks 分组逐组落地，每组独立可测。

## Open Questions

（无——Ctrl+O 与单卡态的叠加规则已按 pi 源码核实定死（D1），点击区域
判定（D3）、事件形态（D4）均已在 proposal 范围内收口。）
