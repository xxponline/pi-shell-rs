# 配色注入 (Theme Injection) 需求

> 来源: balabala-agent-gui 仓 explore 会话 (2026-11-15)。宿主 GUI 要支持
> 浅色/深色双主题并要求终端内容区跟随; 本需求描述 pi-shell-rs 侧需要的
> 最小改动。设计点已决: **注入完整值** (见文末)。

## 背景

宿主 GUI (balabala-agent-gui) 正在做全应用的浅色/深色双主题适配。pi-shell-rs
当前全部颜色按 pi dark.json 深底设计:

- 作为**独立 TUI** (`src/main.rs` → `App::new().run()`) 这是对的, **必须保持现状**;
- 作为 **lib 被 embed** (宿主经 `EmbedConfig` 拉起) 时, 宿主需要注入浅色配色,
  否则浅色主题下出现结构性坏点 (深灰工具卡面板上叠深色默认字、深底设计的
  语法高亮 fg 铺在白底上洗白)。

**主题状态的唯一真相在宿主**: pi-shell-rs 不读系统外观、不自己切主题、不内置
任何浅色知识, 只接受注入 ("宿主说啥我画啥")。

### 颜色分类 — 为什么只有 6+1 处要动

全仓 ~140 处颜色分两类:

| 类别 | 数量 | 主题化归属 |
|---|---|---|
| 硬编码 `Color::Rgb` (越调色板直通) | 6 处 | **本需求: 注入** |
| syntect 主题 (fg 也是 Rgb 直通) | 1 处 | **本需求: 注入** |
| 命名 ANSI 色 (Cyan/DarkGray/Yellow/Green/Red/Black-on-Cyan…) | ~140 处 | **不动** — 走 ANSI 0-15 索引, 由宿主渲染时的调色板表解释, 天生主题无关 |

## 需求

### R1 — 配色注入结构 `TuiTheme` (名字可自定)

包含以下字段, 默认值即现值:

| 字段 | 现值 (= 默认值) | 代码位置 (行号 @ 1535eba) |
|---|---|---|
| 工具卡运行中面板色 | `Rgb(52, 56, 58)` (运行灰) | `src/app/chat.rs:313` |
| 工具卡失败面板色 | `Rgb(91, 40, 42)` (失败暗红) | `src/app/chat.rs:314` |
| 工具卡成功面板色 | `Rgb(37, 65, 49)` (成功墨绿) | `src/app/chat.rs:315` |
| 滚动条 track | `Rgb(72, 78, 82)` | `src/app/wrap.rs:45` (`SCROLLBAR_TRACK_COLOR`) |
| 滚动条 thumb | `Rgb(151, 160, 165)` | `src/app/wrap.rs:51` (`SCROLLBAR_THUMB_COLOR`) |
| 语法高亮主题名 | `"base16-ocean.dark"` | `src/app/markdown.rs:43` (`theme()`) |

消费点: `chat.rs::tool_panel_bg()` / `wrap.rs` 渲染与 `view.rs:857-858` /
`markdown.rs::highlight_uncached():386`。

### R2 — 默认行为字节级不变

- `Default` 实现 = 上表现值;
- standalone 路径与未注入的 embed 路径输出与今天**完全一致**;
- 现有测试 (`chat.rs:904-906` 断言精确 RGB、`tool_card_panel_bg_three_states`
  等) **不改一行仍全绿**。

### R3 — embed 注入点

`EmbedConfig` (`src/app/embed.rs:32`) 加 `theme: Option<TuiTheme>`,`None` =
默认。对齐现有先例 (`set_terminal_sinks` / `set_title` / `copy_text`)。

### R4 — 运行时切换

`EmbeddedApp::set_theme(TuiTheme)` → 经 keys 通道送新 `UiEvent::Theme(TuiTheme)`
→ `event_loop` (`src/app/mod.rs:374`, 现有 30 个 arm) 应用新值并置
`need_draw = true`。

ratatui 的 diff 比较含 style: 主题变 → 全部带色格子变 → 全屏 SGR 自动重发 →
宿主 vt100 收到新色, 一帧内生效。**不需要**宿主手动清屏或重灌。

### R5 — 高亮缓存防串色 (最容易漏的坑)

`HL_CACHE` (`src/app/markdown.rs:28`) 键是 `(lang, body)`, **不含主题** —
切主题后旧色行会从缓存漏出。要求: 键加主题标识, 或 `set_theme` 时清空缓存。

### R6 — `theme()` 可切换

现在是 `OnceLock` 静态单例 (`src/app/markdown.rs:38`), 唯一消费点
`highlight_uncached():386`。需改为随注入可变 (App 持有 / 参数下传均可)。
`chat_lines` 是纯函数、参数已有 4 个 — 实现方式自定, 但**保持纯函数可单测**。

### R7 — spec delta (本仓 openspec)

- `openspec/specs/tui-app/spec.md` 的 **工具调用卡片** requirement 钉死了
  "MUST 按状态着背景面板 (**运行灰、成功墨绿、失败暗红**)" + scenario
  **状态背景面板** — 必须泛化: "默认深色调为运行灰/成功墨绿/失败暗红;
  宿主注入配色时以注入值为准"。**不改 spec 直接实现 = 违反本仓 spec。**
- **消息区滚动条** 措辞已是 "对齐**主题的** track/thumb 色" — 相对化, 不动。
- **工具卡片语法高亮** 措辞已是 "**同一主题**" — 不动。
- 建议新增 requirement: **宿主配色注入** (EmbedConfig 注入 + set_theme 运行时
  切换 + 默认不变)。

### R8 — 明确不改

- 命名色语义 (Cyan=选中、DarkGray=提示、Green/Red=状态、Black-on-Cyan 高亮条);
- bash 输出纯文本化;
- `UiEvent` 既有语义 (只新增 `Theme` 变体);
- pi RPC 协议 (pi agent TS 侧零改动 — 它只发结构化 JSON, 不含颜色);
- standalone 默认外观。

## 验收

1. 默认路径: 现有测试全绿 + 输出与改动前逐字节一致;
2. 注入浅色 `TuiTheme` 后: 工具卡面板、滚动条、代码高亮按注入值出 ANSI;
3. `set_theme` 后一帧内: 三个注入面全部按新值重发 (无头 `TestBackend` 断言);
4. 切主题后高亮缓存不漏旧色 (R5);
5. standalone `cargo run` 外观与今天无差别。

## 已决设计点: 注入完整值

`TuiTheme` 是**完整值结构体** (5 个颜色值 + syntect 主题名), `Default` =
现值。宿主 (GUI 仓) 拥有全部配色决策, pi-shell-rs 不内置浅色知识。

推论: 未来加颜色旋钮 (如输入框边框) = 结构体加字段 + 默认值, 单仓扩 API,
两仓不需要同步改。
