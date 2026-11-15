# Proposal

## Why

宿主 GUI（balabala-agent-gui）正在做全应用浅色/深色双主题适配，而 pi-shell-rs
的全部颜色按 pi dark.json 深底设计。作为独立 TUI 这是对的、必须保持；但作为
lib 被 embed 时宿主无法注入浅色配色，浅色主题下出现结构性坏点（深灰工具卡面板
叠深色默认字、深底语法高亮 fg 铺白底洗白）。主题状态的唯一真相在宿主——本仓
只接受注入（"宿主说啥我画啥"），不读系统外观、不内置浅色知识。

全仓 ~140 处颜色中只有 **6 处硬编码 `Color::Rgb` + 1 处 syntect 主题**是
RGB 直通、主题相关的；其余 ~140 处命名 ANSI 色走 0-15 索引、由宿主渲染调色板
解释，天生主题无关。所以这是一个**边界清晰的最小改动**。

## What Changes

- 新增配色结构 `TuiTheme`（5 个颜色值 + syntect 主题名，`Default` = 现值）：
  工具卡运行中/失败/成功面板色、滚动条 track/thumb 色、语法高亮主题名。
- **穿线方式（已决，方案 A）**：新增 `chat_lines_themed(..., theme)` 入口，
  原 `chat_lines` 薄包装传 `TuiTheme::default()`——现有 28 处调用点（含 ~20
  处测试）**一行不改**；`ViewUi` 加 `theme` 字段（现有 `..ViewUi::default()`
  构造点全兼容）；`render_markdown` 同样提供 themed 入口 + 默认包装。
- `wrap.rs` 滚动条色由 `const` 改为随注入值（`wrap_for_viewport` 加 theme
  参数；不画滚动条的调用路径传 Default）。
- `markdown.rs` 的 `theme()` 由 `OnceLock` 单例改为按名可切换：`ThemeSet`
  一次性加载后按名查表；非法主题名校验回退默认（宿主输入不 panic、调用方可感知）。
- **双缓存防串色**：`HL_CACHE`（键 lang+body）与 `MD_CACHE`（键内容哈希）
  都烧入了主题色——`set_theme` 时同线程一并清空（两缓存均为渲染线程
  thread_local，keys 通道送达 event loop 线程正好同线程）。
- `EmbedConfig` 加 `theme: Option<TuiTheme>`（`None` = 默认，对齐
  `set_terminal_sinks`/`set_title` 先例）；`EmbeddedApp::set_theme(TuiTheme)`
  → `UiEvent::Theme` → event loop 应用新值并 `need_draw = true`。
- **默认行为字节级不变**：`Default` = 现值，standalone 与未注入 embed 输出
  与今天完全一致。
- spec delta（`tui-app`）：泛化"工具调用卡片"的面板着色措辞（默认深色调，
  宿主注入时以注入值为准）；新增 requirement **宿主配色注入**。
- **明确不做**（R8）：命名 ANSI 色语义、bash 输出纯文本化、既有 `UiEvent`
  语义（只新增 `Theme` 变体）、pi RPC 协议、standalone 默认外观。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `tui-app`: ①"工具调用卡片"requirement 的面板着色从钉死"运行灰、成功墨绿、
  失败暗红"泛化为"默认深色调为三色、宿主注入配色时以注入值为准"（含 scenario
  措辞）；②新增 requirement **宿主配色注入**：`EmbedConfig` 注入 +
  `set_theme` 运行时切换 + 默认值不变 + 切换后缓存不漏旧色。

## Impact

- **代码**：`src/app/chat.rs`（`chat_lines_themed` + `tool_panel_bg` 吃
  theme）、`src/app/wrap.rs`（滚动条色参数化）、`src/app/markdown.rs`
  （themed 入口、按名主题、双缓存清理）、`src/app/view.rs`（`ViewUi.theme`
  下传）、`src/app/mod.rs`（App 持有 theme、`UiEvent::Theme` arm）、
  `src/app/event.rs`（新增 `Theme` 变体）、`src/app/embed.rs`
  （`EmbedConfig.theme`、`set_theme`）。
- **测试**：现有测试零改动全绿；新增 themed 注入断言（三注入面按注入值出色、
  `set_theme` 一帧内重发、切主题后缓存不漏旧色）。
- **跨仓**：宿主 GUI 仓按 `TuiTheme` 字段注入（结构体加字段 = 单仓扩 API，
  两仓不需同步改）；宿主需保证其 ANSI 0-15 调色板在双主题下与注入面板色的
  对比度（本仓 R8 不动 ANSI 语义，对比度是宿主调色板职责）。
- **文档**：`Docs/theme-injection-requirements.md` 验收项销账；可能同步
  `Docs/tui-usage.md` 已知差异说明。
