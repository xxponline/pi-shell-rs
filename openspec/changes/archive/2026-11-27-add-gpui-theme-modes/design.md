# Design

## Context

- 现状：`window.rs::pin_light_theme` 把 gpui 全局主题钉为 `ThemeMode::Light`
  （明确注释「完整主题系统为后续 change」）；`terminal.rs::palette` 模块收拢
  28 个浅色硬编码常数（33 处消费），其中工具卡三态取 `TuiTheme::default`
  （pi dark.json 值）——形成「白面板 + 深卡浅字」的已验收混血观感。
- 框架件现成：gpui-component `Theme::change(mode, window, cx)` 是热路径
  （reconcile + `apply_config`），`ThemeMode` 仅 `{Light, Dark}`；OS 外观
  经 `Platform::on_appearance_changed` → window `appearance_observers` 可订阅；
  `sync_system_appearance` 提供 `appearance → ThemeMode` 解析。
- 值源：pi `dist/modes/interactive/theme/{light,dark}.json` 各 56 token +
  8 vars（okhsl），语义覆盖我们 28 token 的大部分；页面级底色 pi 留给终端
  （无 bg token），须自有值。
- 所有权（explore 已定）：独立窗口自注入；嵌入时宿主经 gpui 全局注入；
  主项目持久化 mode；本仓不持久化、不碰 pi `settings.json`。
- 不受影响的既有机制：`TuiTheme` 注入（embed/宿主真相 doctrine）、TUI
  standalone 默认外观。

## Goals / Non-Goals

**Goals:**

- 独立窗口 light/dark/system 三模式，默认 system，选择即时生效、不落盘。
- 调色板 pi 同源（P1）：语义 token 双表对齐 pi light.json/dark.json，
  工具卡随模式取色。
- 内建控件（textarea/滚动条/浮层等 kit 面）与我们的 token 同一 mode 驱动。

**Non-Goals:**

- 不解析 pi 自定义主题 JSON、不处理 pi auto 值（`light/dark` 配对）。
- 不读写 pi `settings.json` `theme` 键；不实现任何持久化。
- TUI standalone、embed `TuiTheme` 注入、pi RPC 零改动。
- 不复刻 pi 的终端背景查询（OSC 2031）——那是 pi TUI 的「system」语义，
  GPUI 窗口的 system = OS 外观。

## Decisions

**D1 mode 状态模型**：独立窗口持有一个内存态 `mode: Light | Dark | System`
（默认 `System`），渲染前解析为 `resolved: Light | Dark`（system → 读 OS
外观）。切换只改内存 + 重绘。备选「写 pi settings.json」被 A3 决策否决
（持久化属主项目职责）；备选「独立配置文件」引入新配置面，同样否决。

**D2 双调色板（P1 pi 同源）**：`palette` 模块从 `const` 集合改为
`Palette` 结构 ×2（`LIGHT` / `DARK`），消费点改为按 resolved mode 取表。
token 分两类：
- **pi 同源语义 token**（工具卡三态、通知错误/重试/默认、bash 命令/正文、
  exit 成败、滚动条 track/thumb、thinking、卡片文字/状态/diff 色等）：
  从 pi 两份 json 的对应键预计算 okhsl → sRGB 常数。映射表在 tasks 中列出
  （如 `CARD_STATUS_RUN ← toolPendingBg 系`、`USER_LABEL ← accent/blue`、
  `EXIT_OK ← success`、`BASH_CMD ← warning 系`）。
- **自有面板面**（`CHAT_BG`/`STATUS_BG`/`INPUT_BORDER`/补全行四色）：
  pi 无对应 token（终端留白）——浅色 = 现手调值不动，深色 = 新增手调值
  （实现期定值 + PTY 验收）。
工具卡随模式：浅色模式卡 = pi light.json 三态（浅卡）+ 深字（toolTitle/
toolOutput 系）——替换现混血；深色模式 = 现值（pi dark 同源）。
备选 P2（卡片恒深）被用户否决，取复刻一致性。

**D3 system = OS 外观 live 跟随**：启动/切到 system 时订阅窗口外观变化
（`appearance_observers` 通路），回调里 `Theme::change(resolved)`；切离
system 取消依赖（或回调内先查 mode 再动作，天然幂等）。备选轮询
`cx.window_appearance()` 被否（有推送通路不轮询）。

**D4 选择入口**：`/settings` 浮层加 Theme 行，循环 `system → light → dark`
（`doubleEsc` 同款循环交互与提示形态），写入 D1 的内存 mode 并即时重绘。
与 `double_esc` 的唯一差别：**不回写文件**。备选 `/theme` 斜杠命令：pi 的
复刻基准是 settings selector，且「简单选择」= 一行循环，不另造命令面。

**D5 热切换与重绘**：一次 `Theme::change(resolved)` 翻转 kit 全局面
（内建控件自动跟随）；我们的 token 在渲染时读 `Theme::global(cx).mode`
解析取表。风险点：mode 变化是否触发本组件重绘——`Theme::edit` 有
reconcile/notify 路径，若实测本窗口不重绘，则组件观察 Theme 全局变化
（`cx.observe_global::<Theme>`）→ `window.notify()` 兜底（tasks 中有 spike）。

**D6 常数表生成**：okhsl → sRGB 在**编写期**用脚本按 pi 同算法预计算，
产出 Rust `const` 表入库；运行时不引色彩解析依赖。PTY 验收对比 pi 原生
观感，偏差逐 token 修正。

**D7 规格落位**：`gpui-agent-terminal` 内解决（窗口能力边界内），不新建
capability：REMOVED `独立窗口浅色模式固定` + ADDED `独立窗口主题三模式`
+ MODIFIED `输入区简约边框`。tui-app 不动（TUI/embed 零改动）。

## Risks / Trade-offs

- [okhsl 预计算与 pi 数值不一致] → 用同算法脚本生成 + PTY 与 pi 并排比对；
  验收不过逐 token 调。
- [mode 切换后窗口不重绘] → D5 兜底观察全局 Theme；测试断言切换后一帧
  内新色生效。
- [已验收的浅色混血观感被 P1 改变（浅色下浅卡）] → 用户在 explore 中
  明确选择 P1（复刻一致性优先），属预期变化，PTY 验收时按新基准确认。
- [深色面板底色无 pi 值源（手调）] → 实现期定值，与浅色现值对仗，
  PTY 验收对比度（深底上的 ANSI 名色仍走终端/语义提亮规则）。
- [system 模式测试不可控（依赖 OS 外观）] → 单测只测解析函数与订阅
  接线（注入 appearance 值），真实跟随交 PTY 手工。

## Migration Plan

仓内增量改动，无部署/回滚问题；回滚 = revert 本 change 的提交。
规格侧为 REMOVED+ADDED+MODIFIED 一次归档同步，无中间态。

## Open Questions

- 深色面板底色 / 深色输入边框的具体数值：实现期手调 + PTY 验收定稿
  （不改规格、不改任务结构）。
