# Design

## Context

GPUI 窗口的卡片渲染目前是 `gpui_agent/terminal.rs` 里的一套简化自绘（`tool_card` 只有头行+首行预览、`bash_row` 取头 5 行、`notice_row` 无前缀），而 TUI 侧的完整 pi 对齐语义全在共享层 `app/tools.rs`（`tool_lines_themed`）与 `app/chat.rs`（`append_blocks`/`append_bash`/`notice_line`），输出为 ratatui `Line`。GPUI 渲染不用 ratatui，因此语义无法直接复用。另有键路由缺口：`route_keystroke`（terminal.rs:605）白名单不含 `ctrl+t`/`ctrl+o`，两者落入 `Route::ToEditor` 后永远到不了 `App::key_intent`。

约束（见 proposal.md）：共享层"仅加法"（`gpui-agent-terminal` 规格硬要求 + 存量测试全绿）；渲染优先内建控件；方案 A（中性语义模型）已由用户拍板。

## Goals / Non-Goals

**Goals:**
- 卡片/通知/bash 条目语义单一真相源，GPUI 与 TUI 逐项一致（头行参数、折叠行数、提示文案、受保护尾部、截断与状态行）
- GPUI 渲染全部走内建控件/内建文本样式
- `ctrl+t`/`ctrl+o` 在 GPUI 窗口可达；队列条与 widget 面板补齐
- ratatui 路径行为零变化（存量测试全绿为验收线）

**Non-Goals:**
- 消息区文本选区、滚动条 hover/drag 细节、alt-screen 搜索
- 主题系统（浅色固定现状不变）
- 引入 gpui-component 的 `Diff`、`Notification` 组件（已裁决不用）
- TUI 侧行为/外观调整

## Decisions

### D1：中性语义模型 `ToolCardModel` + `Seg`（方案 A）

共享层新增与渲染框架无关的段序列模型：

```rust
/// 语义段：文本 + 语义角色（由渲染端映射为具体颜色/字重）
pub enum SegRole { Dot, Name, Arg, StatusOk, StatusErr, StatusRun, Timer, Body, Hint, Exit, Warn }
pub struct Seg { pub text: String, pub role: SegRole }

pub struct ToolCardModel {
    pub head: Vec<Seg>,        // ● name · 关键参数 · 状态 · 计时
    pub body: Vec<Seg>,        // 折叠后结果体（已按 collapse_policy 截断）
    pub hint: Option<Seg>,     // "... (N more lines, ctrl+o to expand)"
    pub tail: Vec<Seg>,        // exit 行 + 截断警告（受保护）
}
pub fn tool_card_model(name, args_json, tool, expanded) -> ToolCardModel;
```

- 现有 `tool_lines_themed` 改为 `tool_card_model` 的薄适配（Seg → Span，role → 既有 Style），行为零变化；`head_arg`/`collapse`/`protected_tail`/`timer_text` 等纯函数下沉到 model 构建内，不再直接产出 ratatui 类型
- 通知/bash/thinking 同构处理：`notice_model(kind, text) -> Vec<Seg>`、`bash_entry_model(...) -> BashEntryModel`（尾部 500 行截断 + 状态行）、`thinking_visible(text, expanded) -> Option<String>`（空块 → None）
- **否决 B**（GPUI 重写语义）：两套逻辑双份测试、必然漂移
- **否决 C**（`Line` → `GridLine` 转换上屏）：最省事但数据源自 ratatui，不满足"渲染全内建"的裁决；状态行维持现状走 C 路径不动（不在本变更范围）

GPUI 侧 `SegRole → 颜色` 映射收拢为一个 match（palette 常量扩展），是纯视图层。

### D2：展开交互用 `Collapsible`/`AccordionItem` + 共享覆盖表

- 单卡/单块展开状态继续用共享 `App.tool_expanded` / `App.thinking_overrides`（GPUI 行渲染经 `EntryCtx` 携带 `&App`——`render_flags()` 接线由它承接，同时补传两张覆盖表与点击回路）
- 卡片头行 `.on_click` 切换覆盖表项（仅 `tool.result.is_some()` 的卡响应，对齐 TUI 命中门控）；thinking 块头 `.on_click` 切换 `thinking_overrides[seq]`——**seq 游标必须与 `chat.rs` 的全局序号规则一致**（无条件递增、空块也计数），否则单块命中错位
- 折叠体用内建 `Collapsible::open(bool).content(...)` 控制挂载；不用 `Accordion` 的整组语义（卡片互相独立、无手风琴互斥）

### D3：计时走字 = 有运行中工具才起 1s interval

组件内 `cx.spawn` 循环：`while has_running_tool { sleep(1s); cx.notify() }`，用任务句柄按需启停（状态变化时对账），对齐 TUI `timer_redraw_due`（`mod.rs:3212`）。**否决**不走字方案（与 TUI 有可见差异，用户已选 A）。

### D4：键路由白名单补两项

`route_keystroke` 增加 `"t" if control`、`"o" if control` → `Route::ToApp`（镜像 `key_intent` 的 `Char('t')/'o' if ctrl` 分支）。补单测进 `route_keystroke_three_zones`。注意 `ctrl+shift+o` 等变体按 `key_intent` 现状不特判（其 match 也不特判 shift）。

### D5：edit diff 用内建文本 runs

`edit_body` 语义产出的行按前缀着色（`-` 红 / `+` 绿 / `@@` 青），GPUI 用文本 runs 上屏。**否决** gpui-component `Diff` 组件：文件级虚拟滚动查看器（DiffState + ListState + 文件头 + 横向滚动），嵌入消息行会引入嵌套滚动与独立状态，收益为零。

### D6：通知行内联文本，不用 toast

`Notification` 组件是自动消失的浮层 toast，与 TUI"对话流内联行"形态不符；内联 `Vec<Seg>` + 粗/斜/色即可满足。

### D7：队列条与 widget 面板

- 队列条：复用 `view.rs::queue_lines` 语义（抽为中性模型或直接复用其 `Vec<String>` 输出——它是纯文本行，无样式语义，直接复用即可），渲染于消息区与输入区之间，空则不占位
- widget 面板：数据面已通——RPC `extension_ui_request` → `apply_core_event` → 共享层 `handle_extension_request` 的 `setWidget` 臂写 `App.widget_lines`（null/缺失 → 清空；协议无独立 `clearWidget` 方法，实测 pi 源码零命中），GPUI 侧只需渲染 ` widget ` 标题边框面板（青色、上边框，`widget_lines` 读取器入 App，空则不占位）

## Risks / Trade-offs

- [共享层重构碰 `tools.rs` 核心路径，回归面大] → `tool_lines_themed` 只做"改为基于 model 组装"的薄适配；`tools.rs` 现有 30+ 测试与 `layout_cache`/`chat` 测试全绿为每步验收；分 tasks 提交
- [thinking 单块 seq 与 `chat.rs` 游标规则漂移导致点击错块] → model 层统一产出 seq（与 `append_blocks` 同一函数或同规则单测双端断言）；补"空块计数"专项测试
- [1s interval 任务泄漏/关窗后仍跑] → 任务持有 weak 实体引用，实体释放即退出；启动/停止与 `has_running_tool` 状态对账
- [内建控件样式 token 与浅色固定主题冲突] → `SegRole → 颜色` 映射全部走既有 `palette` 常量，不引入主题系统
- [GPUI 测试无法断言颜色] → 沿用 `test_text` 快照断言语义文本；语义一致性由共享 model 单测（双端渲染同一 fixture 断言文本相同）保证

## Migration Plan

单 PR 内按 tasks 顺序落地：共享 model（+TUI 薄适配，测试全绿）→ GPUI 渲染重写 → 键路由/计时/队列条/widget → 双矩阵验证（`cargo test` / `cargo test --features gpui-support` + clippy 两列）。回滚即 revert，无数据迁移。

## Open Questions

- GPUI 卡片点击的 hover 反馈（TUI 无 hover 概念）是否后续补——不影响本变更 specs，留待主题 change。
