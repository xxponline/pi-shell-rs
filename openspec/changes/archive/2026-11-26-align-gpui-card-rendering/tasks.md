# Tasks

## 1. 共享层中性语义模型（仅加法 + TUI 薄适配）

- [x] 1.1 `app/tools.rs` 新增 `Seg`/`SegRole`/`ToolCardModel` 与 `tool_card_model()`，把 `head_arg`/`collapse_policy`/`collapse`/`protected_tail`/`timer_text` 下沉为 model 构建；验证：新增 model 单测（bash 尾 5 行 + 提示行 + exit 受保护、grep 15、read 头 10、头行 `$ cmd (timeout 5s)`/`:1-20`/`/pat/ in path`、计时三态）全绿
- [x] 1.2 `tool_lines_themed` 改为基于 `tool_card_model` 的薄适配（Seg → Span、role → 既有 Style）；验证：`cargo test` 全绿（`tools.rs` 30+ 存量测试逐条过，行为零变化）
- [x] 1.3 `app/chat.rs` 新增 `notice_model()`/`bash_entry_model()`/`thinking_visible()` 中性函数（`[kind]` 前缀样式角色、bash 尾部 500 行截断 + `…（已截断 N 行）` + 状态行文案、thinking 空块 → None）；验证：新增单测覆盖 error/retry 前缀、600 行截断、exit 0/N/cancelled/运行中文案、空块不占行
- [x] 1.4 `notice_line`/`append_bash` 改为基于 1.3 模型的薄适配；验证：`cargo test` 全绿（`chat.rs` 存量测试含 `thinking_collapsed_by_default` 等逐条过）
- [x] 1.5 thinking 单块 seq 规则收敛为共享单测：空块计数、跨条目递增、双端同 fixture 断言相同序号；验证：新测试全绿
- [x] 1.6 回归修复（5.3 冒烟 B2 查色发现）：`seg_style(Cmd)` 补回 TUI 黄色——task1.2 段化重构把 `$ cmd` 的 Yellow+BOLD 丢成了纯粗体（与 HEAD/spec「命令行黄色加粗」不符）；验证：新增回归测试 `bash_command_line_is_yellow_bold`（黄+粗）

## 2. GPUI 卡片渲染重写（内建控件）

- [x] 2.1 `gpui_agent/terminal.rs` 增加 `SegRole → palette` 颜色映射；`assistant_block`/`tool_card` 改为消费 `ToolCardModel`（内建文本 runs + 面板 `div().bg()` 三态）；验证：`fixture_renders_message_grid_snapshot` 的 `tool_call`/`failed_tool` 断言更新后过，且新增头行参数/计时/提示行 needle 断言
- [x] 2.2 折叠态/展开态接 `Collapsible` + 共享覆盖表（`render_flags` 补传 `tool_expanded`/`thinking_overrides`；仅 `result.is_some()` 卡响应点击）；验证：新增 GPUI 测试——单卡点击仅该卡切换、运行中卡点击无响应、全局 `expand_tools` 生效
- [x] 2.3 thinking 块：空块不占行、折叠占位、点击单块切换（seq 用 1.5 的共享规则）；验证：新增测试——空块无行、`<thinking N chars>` 占位、单块点击不影响全局与它块
- [x] 2.4 edit 卡红绿 diff 文本 runs（`-` 红/`+` 绿/`@@` 青，old/new 降级路径）；验证：新增测试——diff 字段与降级两路径 needle 断言
- [x] 2.5 `notice_row` 消费 `notice_model`（`[kind]` 前缀 + 粗/斜/色）；验证：新增测试——`[error]` 红粗、`[retry]` 黄 needle 断言

## 3. bash 条目 / 队列条 / widget 面板

- [x] 3.1 `bash_row` 消费 `bash_entry_model`（尾部 500 行 + 截断标记 + 状态行 + `（不进上下文）`）；验证：新增测试——600 行 fixture 显示尾 500 + `…（已截断 100 行）`、exit 状态 needle
- [x] 3.2 消息区与输入区之间渲染队列条（复用 `view.rs::queue_lines` 输出，空队列不占位）；验证：新增测试——流式排队出现 `[queue]`、清空后消失
- [x] 3.3 `apply_ui_event` 补 `setWidget`/`clearWidget` 分支 + ` widget ` 边框面板渲染（`widget_lines` 入 `TerminalState`）；验证：新增测试——设置后面板可见、null 清除、不挤压消息区
- [x] 3.4 运行中 bash 尾槽（5.3 冒烟 B2 发现的缺口）：GPUI 消息区尾部渲染运行中 `!cmd`（共享 `bash_entry_model(live)` + 行数对账含尾槽），完成后收敛为落盘条目；验证：新增测试——运行中命令行/状态行在场、输出分片增长、落条目后尾槽消失

## 4. 键路由与计时

- [x] 4.1 `route_keystroke` 白名单补 `ctrl+t`/`ctrl+o` → `ToApp`；验证：`route_keystroke_three_zones` 补断言，且端到端测试——按键后 `show_thinking`/`expand_tools` 翻转、消息区重渲染、不落入 Textarea
- [x] 4.2 运行中工具 1s interval → `cx.notify` 走字（任务按 `has_running_tool` 启停、weak 实体退出）；验证：新增测试——Running 期间计时递增、Ended 后无周期重绘（测试用可注入时钟或手动 pump 断言任务启停）

## 5. 双矩阵验证与文档

- [x] 5.1 `cargo test` + `cargo clippy --all-targets` 全绿零告警（列 A：默认特性）
- [x] 5.2 `cargo test --features gpui-support` + `cargo clippy --all-targets --features gpui-support` 全绿零告警（列 B）
- [x] 5.3 PTY 手工验证：真实终端多轮冒烟完成（A 基线 / B 工具卡·bash / C thinking 全过；期间发现并修复 3.4 运行中 bash 尾槽缺失与 1.6 TUI 命令行黄色丢失，修正项均补了回归测试）
- [x] 5.4 更新 `Docs/tui-usage.md` GPUI 节（卡片语义对齐、ctrl+t/ctrl+o 键位、队列条/widget 面板）；验证：文档命令与键位表照做可用

## Workflow follow-up

- `openspec validate align-gpui-card-rendering --strict` 通过后归档（`/opsx-archive`），主规格随归档同步。
