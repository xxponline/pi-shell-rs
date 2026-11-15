# Design

## Context

见 proposal.md「Why」。当前颜色的三条链路（均已探查核实）：

```
                     TuiTheme（本 change 新增，注入完整值）
                           |
           +---------------+----------------+
           |               |                |
           v               v                v
     tool_panel_bg    wrap 滚动条色      syntect theme()
     chat.rs:313-315  wrap.rs:45/51      markdown.rs:38 (OnceLock 单例)
           |               |                |
           v               v                v
     append_blocks   wrap_for_viewport  highlight_uncached
           |               ^                |
           +-------+-------+--------+-------+
                   |                |
                   v                v
     chat_lines（纯函数 4 参）    HL_CACHE (lang, body)
                   ^              MD_CACHE (content hash)  <-- 两缓存都烧入主题色
                   |
     view::render:367（App 每帧调用）+ refresh_selection_doc
```

约束（来自需求文档 R2/R6 与探索结论）：

- 现有测试（`chat.rs` 精确 RGB 断言、`tool_card_panel_bg_three_states`、
  `view.rs` 滚动条断言等，`chat_lines` 约 20 处测试直调）**一行不改仍全绿**；
- `chat_lines` / `render_markdown` 渲染链保持纯函数可单测；
- 约 ~140 处命名 ANSI 色不动（R8）。

## Goals / Non-Goals

**Goals:**

- 7 处 RGB 直通点（6 硬编码 + syntect 主题）可注入、可运行时切换；
- 默认路径输出与改动前逐字节一致（`Default` = 现值）；
- 切主题后任何缓存不漏旧色；
- 纯函数链可单测、现有测试零改动。

**Non-Goals:**

- 命名 ANSI 色主题化（宿主调色板职责，R8）；
- 独立 TUI 的主题切换 UI / 系统外观探测；
- 宿主侧调色板对比度保证（跨仓，交付说明中提示即可）；
- `available_themes()` 等宿主辅助 API（范围外，未来可单仓扩）。

## Decisions

### D1 — 穿线方式：themed 薄包装（探索已决，方案 A）

新增 `chat_lines_themed(state, flags, overrides, thinking_overrides, theme)`，
原 `chat_lines` 保留 4 参签名、内部转发 `TuiTheme::default()`；
`render_markdown` 同型（`render_markdown_themed(text, theme)` + 默认包装）。

- **为什么**：唯一同时满足 R2（28 处调用点零改动）与 R6（纯函数可测）的
  方案。
- **否决的替代**：直接加第 5 参——约 20 个测试调用点要改，违反 R2；
  `RenderFlags` 加字段——5 处 struct 字面量无 `..Default`，编译即挂；
  thread_local 存主题——签名零改动但破坏纯度，R6 明确要求保纯。
- `ViewUi` 加 `theme: TuiTheme` 字段作注入通道：现有构造点全部经
  `..ViewUi::default()`，天然兼容；`view::render` → `chat_lines_themed`
  与 `wrap_for_viewport` 从 `ui.theme` 取值。

### D2 — 滚动条色参数化

`wrap.rs` 的 `SCROLLBAR_TRACK_COLOR`/`SCROLLBAR_THUMB_COLOR` `const` 降级为
`TuiTheme::default()` 的字段来源；`wrap_for_viewport` 加 `theme` 参数。
`refresh_selection_doc` 调用路径 overlay=None 不画滚动条，传 Default。
`view.rs` 测试直接引用 const 断言——const 保留为默认值定义处（或测试改读
`TuiTheme::default()` 字段，若 const 移除则该测试属"读取默认值"的等价改写，
优先保持 const 存在以零改测试）。

### D3 — syntect 主题按名可切换

- `ThemeSet::load_defaults()` 一次性存 `OnceLock<ThemeSet>`（原
  `.themes.remove(...)` 改为 `get` 查表，返回 `&'static Theme`）；
- 非法主题名：`set_theme` / embed 注入时校验，**回退默认主题 + 调用方可感知**
  （`set_theme` 返回 bool/Result；embed 启动注入非法名同样回退不 panic）。
  否决静默回退——宿主拼写错误会变隐形 bug；否决 panic——宿主输入打死 embed
  线程。
- 不做 `available_themes()`（Non-Goal，未来单仓扩 API 即可）。

### D4 — 双缓存清理（R5 的完整版）

`HL_CACHE`（键 lang+body）**和** `MD_CACHE`（键内容哈希）都烧入主题色——
探索发现 R5 只点了 HL_CACHE。决策：**`set_theme` 应用时无条件清两缓存**，
而非改键：

- 两缓存均为渲染线程 thread_local，`set_theme` 经 keys 通道送达 event loop
  线程，与绘制同线程，清空即彻底；
- 改键需要把主题标识下传到缓存读写点，侵入更大；且 markdown 输出只受
  syntect 主题名影响（面板/滚动条色不进 markdown），清空是超集正确；
- 缓存本身有容量上限（64/128）且是性能优化，清空只损失一次冷启动。
- `App.selection_doc` 缓存带色行但只做文字提取、从不回绘——**不清、不改**。

### D5 — 主题值的持有与事件流

`App` 持 `theme: TuiTheme` → `draw()` 填入 `ViewUi`。运行时切换：

```
EmbeddedApp::set_theme(t)
  -> keys 通道 send(UiEvent::Theme(t))
  -> event loop 新 arm：self.theme = t; 清 HL_CACHE+MD_CACHE; need_draw = true
```

- ratatui diff 只重画值变化的格子——变色格子带新 SGR 重发，一帧内生效，
  宿主无需清屏（需求 R4 的结论成立，"全屏重发"措辞略强但不影响行为）；
- `EmbedConfig` 加 `theme: Option<TuiTheme>`，spawn 时先于首帧应用
  （对齐 `set_terminal_sinks` 先例：构造后 setter 注入）；
- `UiEvent` 仅新增 `Theme` 变体，既有语义不动（R8）。

### D6 — spec delta 对应

- MODIFIED「工具调用卡片」：面板措辞泛化 + 新增"注入配色时面板取注入值"
  scenario；
- ADDED「宿主配色注入」：覆盖注入/切换/默认不变/缓存不漏/非法名回退。
- 「消息区滚动条」（已是"对齐主题的 track/thumb 色"）与「工具卡片语法高亮」
  （已是"同一主题"）措辞已相对化，不动。

## Risks / Trade-offs

- [R2"逐字节不变"依赖 `Default` 精确等于现值] → 字段默认值直接取现 RGB
  常量；验收第 1 条跑全量测试 + 现有精确断言。
- [themed 包装造成双入口，未来调用方误用默认入口] → view 主路径统一走
  themed；包装仅服务存量测试与非渲染路径，文档注释标明分工。
- [切主题清缓存导致一帧冷渲染变慢] → 缓存有上限（64/128 条），切换是低频
  操作，可接受；否决改键方案正是为避免常驻键膨胀。
- [宿主 ANSI 调色板对比度是隐性跨仓依赖] → 不改代码，在交付说明/需求文档
  验收注记中点明（proposal Impact 已记）。
- [event loop arm 数量继续增长] → 单 arm 单职责，与既有 30 个 arm 同模式，
  无结构风险。

## Migration Plan

单仓内部改动，无部署顺序问题。回滚 = revert commit（默认路径行为不变，
回滚不产生数据/协议迁移）。建议任务分组按用户要求：先 `chat_lines_themed`
+ 测试 + commit，再滚动条/markdown/embed 后续分组。

## Open Questions

- `set_theme` 返回类型（`bool` 表示主题名是否生效 vs `Result<(), String>`）：
  不影响 spec（spec 只要求"宿主可感知"），实现时按 embed 先例定即可。
