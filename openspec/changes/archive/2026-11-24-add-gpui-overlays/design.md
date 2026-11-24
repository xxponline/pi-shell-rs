# Design

## Context

- `add-gpui-support` 已归档：GPUIAgentTerminal 三分区（MessageScroller /
  Textarea / StatusBar）+ 单一按键拦截层（`is_global_keystroke` →
  `App::handle_key`）已稳定；渲染模型是"内建控件 + 共享状态直绘"。
- 共享层浮层机制**完备且已测**：`Overlay` 14 变体、`overlay_key` 纯函数
  （Handled/Close/Apply/Delete/Toggle）、`pending_overlays` 队列与
  `overlay_seq`、`key_intent` 键表、`open_*` 方法、slash 命令表
  （`commands.rs`）、扩展对话经 `push_dialog` 由 RPC 事件入队。TUI 在
  `app/terminal.rs` 只负责画。
- 主题机制：`gpui-component theme::init` 初始即 `ThemeMode::Light`，框架
  **不会**自动跟随系统外观（host 仓是手动 sync + 订阅的）。我们从不碰
  theme；但我方 16 处硬编码面板底是深色，与 Light 主题的内建控件深色字
  相撞 → 深字深底（见 proposal Why-1）。
- 键鼠优先级事实（`app/mod.rs`）：ctrl+c 全局优先（浮层之下也生效）→
  浮层分支（Esc 由 `overlay_key` 消费，优先于流式 Abort 与空闲双击 Esc）
  → `key_intent` 分发；浮层打开时 `handle_wheel` / 滚条 / 指针交互全部
  忽略（TUI design D4：不滚被遮住的消息区）。
- 约束：feature 门控不变、共享层仅加法、双矩阵测试全绿。

## Goals / Non-Goals

**Goals:**

- 独立窗口文字可读：浅色模式显式固定 + 硬编码面板浅色化（可读性 pass）。
- 14 个 Overlay 变体在 GPUI 全量可渲染，状态/按键/队列语义与 TUI 完全
  一致（共享 `overlay_key` 是唯一行为源）。
- 单一、可测的键鼠路由模型（三区），拦截表与 `key_intent` 的漂移有
  测试与注释锚点。
- 斜杠入口扩容后各浮层可达（含无参 `/model`、`/thinking`）。

**Non-Goals:**

- 主题系统（ActiveTheme token 化、深浅热切换、TuiTheme 注入对齐、
  16 处色值的"观感打磨"）——后续 change。
- kit 状态化选择器（Select / Combobox / Dialog）作为控制器。
- 浮层动画/过渡效果；ratatui 路径与 embed 管线的任何改动。
- 新增共享行为（本变更对 `app/` 只放宽可见性）。

## Decisions

### D1 浅色固定 = 显式 pin + 面板浅色化，两半缺一不可

- `window.rs::run` 在 `gpui_kit::init(cx)` 后调
  `Theme::change(ThemeMode::Light, None, cx)`；**不**安装
  `observe_window_appearance` 订阅（那是主题 change 的事）。
- 16 处硬编码 `rgb(0x...)` 收拢为 `gpui_agent` 内一处命名常量块
  （消息底/状态底/输入底/补全行/错误重试行等），取浅色系近白/浅灰值，
  与 Light 主题内建文字对比度达标；常量块集中便于后续 change 直接
  替换成 theme token。
- 备选与否定：(a) 只 pin 不改面板 → 无可见变化，dark-on-dark 仍在；
  (b) 反向锁 Dark + 改文字色 → 与"写死 lighting"指示相反且要改的
  颜色更多；(c) 直接接 ActiveTheme → 越界到主题 change。
- pin 只放在窗口入口（不进组件）：组件测试环境不依赖主题模式，
  文本快照断言不受影响。

### D2 浮层渲染：共享状态直绘，kit 只做"贴纸"

- 新模块 `src/gpui_agent/overlay.rs`：入口
  `overlay_layer(app, size, cx) -> Option<AnyElement>`，match
  `App.overlay` 分发到三组渲染助手：

```
 overlay_layer(app, size, cx)
   |
   +-- list_panel      ModelSelect / ThinkingSelect / SessionSelect
   |                   ScopedModels / ExtSelect        （标题+行列表+计数）
   +-- viewer_panel    Info / Tree / Hotkeys           （标题+行/树/键表，
   |                                                     Tree 含 filter/search 行）
   +-- form_panel      Settings / Trust / ImportConfirm
   |                   ExtConfirm / ExtInput / ExtEditor（行/开关/两段确认/
   |                                                     单行/多行编辑展示）
   +-- chrome（共用）  半透明遮罩 + 居中 panel（主题 token：radius/border/
                       bg/阴影）+ 焦点行高亮
```

- 挂载点：GPUIAgentTerminal 的 render 中**最后**绘制（gpui 树序即 z 序），
  覆盖三分区之上，尺寸取整窗。
- 备选与否定：kit `Dialog/Select/Combobox/searchable_list` 作控制器 →
  状态双源（kit 内部态 vs 共享 `Overlay`）、双 Esc（kit 关闭语义 vs
  `overlay_key`）、焦点争夺、14 变体 × 桥接；放弃。样式层用
  `cx.theme()` token 与 kit div/panel 能力 = "内建用在贴纸层"。
- `centered_rect` 是 ratatui Rect 几何，不跨层复用；GPUI 侧按
  百分比在整窗尺寸上自算（纯函数，可单测）。

### D3 键鼠路由：三区模型，浮层区全量接管

```
 keystroke ──> 拦截层 is_global_keystroke / 新增 overlay 分支
   |
   +-- A. overlay 打开 ── 全部键 -> keystroke_to_keyevents -> App::handle_key
   |                      -> 同步 Textarea 镜像 -> stop_propagation
   |                      （ctrl+c 由 handle_key 内部全局优先，语义不变；
   |                        Esc -> overlay_key 关闭，先于流式 Abort——与 TUI 同序）
   +-- B. completion 打开 ── 导航/应用键（现状不动）
   +-- C. idle ── 既有全局键（enter/esc/ctrl+c）+ 打开键
                  ctrl+l -> OpenModelSelect / alt+t -> OpenThinkingSelect
                  （镜像 key_intent；会话/树/设置等打开键在 TUI 中仅 slash
                   驱动，无需新增拦截）-> handle_key -> stop_propagation
   非全局键 ──> Textarea（编辑自理，现状不动）

 wheel ── overlay 打开 -> App::handle_wheel（TUI 语义：忽略，return false）
                     + stop_propagation（阻断 MessageScroller）
         overlay 关闭 -> MessageScroller 内建滚动（现状不动）
```

- 路由判定抽成纯函数（如 `route_keystroke(overlay_open, completion_open,
  ks) -> Route`），无 TTY 可单测；A 区与 C 区共用"转 KeyEvent → handle_key
  → 同步镜像"的既有管线。
- 滚轮在浮层下**忽略**是 TUI 既有语义（design D4 注释），不是遗漏；
  浮层列表翻页用 pageup/pagedown（`overlay_key` 已支持）。条目点击、
  遮罩点击是否响应以 TUI `app/terminal.rs` 现行为为准（实现时核对，
  不发明新行为）。

### D4 共享层加法清单（可见性放宽）

- `src/app/mod.rs`：`mod overlay` → `pub(crate) mod overlay`；
  字段 `overlay: Option<Overlay>` → `pub(crate)`（gpui_agent 只读）。
- `Overlay` 枚举本就 `pub`，变体字段随枚举可见；`overlay_key` /
  `OverlayOutcome` 本就 `pub`。不新增方法、不改任何行为。
- 审计口径同上个 change：rpc/ 零 diff、session/ 零 diff、app/ 仅
  可见性变化，双跑测试锁定。

### D5 斜杠扩容：分发集合判定细化（共享层一个谓词）

- 现状：`slash_subset_only` + `GPUI_QUERY_SUBSET`（changelog/model/
  thinking/quit），其余已知命令 → "暂不支持"桩。
- 改为三段：**查讯型**（原集合）∪ **可开浮层型**（model/thinking/tree/
  settings/trust/hotkeys/session/import/scoped-models…，以 TUI 分发实际
  打开浮层的命令为准，从 `commands.rs`/分发代码枚举）→ 走 TUI 分发
  （无参 `/model` 自然打开选择器，去掉我方 notice 桩）；其余已知 → 桩；
  未知 → 普通文本。共享层新增一个谓词函数（如 `gpui_slash_supported`），
  保持 ratatui 路径行为不变（该谓词仅 GPUI 分发引用）。

### D6 测试策略

- **逐变体冒烟 ×14**：测试内直接置 `app.overlay = Overlay::X{...}`（或
  走 `open_*` / `push_dialog`）→ 断言快照含标题与首行/计数 → 导航键 →
  光标移动 → Esc → 浮层消失。
- **路由纯函数单测**：A/B/C 三区 × 关键键型矩阵；浮层下 wheel 阻断。
- **E2E（fake-pi）**：alt+t 开 ThinkingSelect → 快照可见 → esc 关闭；
  ctrl+l 开 ModelSelect；扩展 `select` 事件 → ExtSelect 渲染 →
  回复发回。
- **浅色**：抽 `pin_light_theme(cx)` 可测入口，测试窗断言
  `theme().mode == Light`；面板常量浅色化后更新既有颜色断言（如有，
  文本断言不受影响）。
- 双矩阵 + clippy 双列零告警照旧。

## Risks / Trade-offs

- [terminal.rs 已 ~2k 行，再堆 14 变体必膨胀] → 渲染全部进新模块
  `gpui_agent/overlay.rs`，逐变体小函数 + 三组共享助手。
- [Esc 语义在"流式 + 浮层"同时为真时易错] → 严格镜像 TUI 分支序
  （浮层先于 intent），加 streaming×overlay 双开测试锁定。
- [拦截表与 key_intent 漂移] → 打开键枚举处注释锚定 `key_intent`
  行号，E2E 覆盖两个打开键；新增打开键时测试先红。
- [浅色化可能波及既有断言] → 现有 GPUI 测试以文本快照为主；
  颜色断言逐个按新常量更新，语义断言不动。
- [Ext* 变体依赖 RPC 扩展事件链] → 用既有 `push_dialog` 路径直测，
  不 mock 新协议；事件链本身是共享层既有行为。
- [14 变体工作量集中] → 任务按三组助手拆分、逐变体勾选，进度可视。

## Migration Plan

单 change 合入，无数据/协议迁移；回滚 = git revert（全部在
`gpui-support` 特性内 + app/ 可见性放宽）。`Docs/tui-usage.md` GPUI 段
随实现同步更新（浅色固定说明、浮层键位表）。

## Open Questions

- 条目点击 / 遮罩点击行为：以 TUI `app/terminal.rs` 现行为为唯一基准，
  实现该任务时核对并照抄——若 TUI 无此行为则 GPUI 也不实现（不发明）。
