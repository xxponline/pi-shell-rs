# Proposal

## Why

`Docs/pi-tui-interaction.md` 差异清单指出当前 TUI 与 pi 在**操作语义**上的核心差距，其中 #1/2/3/4/8/10 直接影响日常手感。grounding 验证发现一个现存真缺陷：pi 在流式中收到不带 `streamingBehavior` 的 prompt 会**直接抛错**（"Specify streamingBehavior..."），而我们当前流式中按 Enter 正是这么发的——用户会看到红色错误而非打断重定向。本变更按差异清单把提交/中断/退出/编辑器四组语义对齐到 pi。

## What Changes

- **提交语义对齐（#1/#4）**：
  - 流式中 Enter → `prompt` 携带 `streamingBehavior:"steer"`（打断重定向，不再报错）；
  - `alt+enter` 空闲时等同普通 Enter（对齐 pi 的 onSubmit 直通），流式中仍为 follow_up 排队（不变）。
- **中断与队列恢复（#3）**：流式中 esc → `abort` **并且** `clear_queue` 把排队消息恢复回输入框（对齐 pi 的 `restoreQueuedMessagesToEditor({abort:true})`）；空闲 esc 不再退出。
- **退出与清空键位对齐（#2）**：`ctrl+c` 单击=清空输入、500ms 内双击=退出；`ctrl+d` 仅输入为空时退出（非空=光标处向后删字符）；**空闲 esc 为无操作**；**`q` 退化为普通字符**（当前 q 空闲即退出会导致输入含 q 的单词直接退出，是与 pi 相悖的体验缺陷）。退出路径收敛为 ctrl+d（空输入）与双击 ctrl+c。
- **编辑器能力（#8/#10）**：输入从"追加式单行 String"升级为**带光标的多行编辑器**：
  - 多行：`shift+enter` / `ctrl+j` 换行（输入区随行数增高，有上限，不挤压消息区）；
  - 光标移动：←→/ctrl+b/f、↑↓（跨行）、home/ctrl+a、end/ctrl+e、alt+←→（词）；
  - 删除族：backspace、delete（非空时）、ctrl+w/alt+backspace（后删词）、alt+d（前删词）、ctrl+u（删至行首）、ctrl+k（删至行尾）；
  - kill-ring/yank：kill 操作入环，`ctrl+y` 粘贴、`alt+y` 回退粘贴；
  - undo：`ctrl+-` 按编辑快照回退（带上限）；
  - 提交时整个缓冲（含换行）作为一条消息发出。
  - **明确不做**：输入历史（pi 默认未绑定该键）、tab 自动补全（后续 change）。
- **不在本变更范围**：`!cmd` bash 模式（#6，独立 change）、本地 slash 命令（#7，随 4b）、ctrl+g/x/v 外部编辑器与剪贴板（#9）、双击 esc 开树（#5，依赖 4b）、Windows 键位（#13）、选择列表分页（#12）。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `tui-app`:
  - **修改**「发送 prompt 并流式渲染回复」——增加流式提交即 steer 的场景；
  - **修改**「中断生成」——esc 流式 = abort + 恢复队列到输入框（空闲退出语义移出到新需求）；
  - **修改**「队列可见与 follow_up 键」——alt+enter 空闲时等同普通 Enter；
  - **修改**「基础输入编辑」——升级为多行 + 光标 + 删除族（重写需求文本与场景）；
  - **新增**「退出与清空键位」——ctrl+c 单/双击、ctrl+d 空退出、空闲 esc 无操作、q 为普通字符；
  - **新增**「kill-ring 与 undo」——kill 系删除入环、yank/yank-pop、undo 快照。

## Impact

- **代码**：`src/app/mod.rs` 输入状态从 `String` 重构为 `EditorBuffer`（行数组 + 光标），新增 `src/app/editor.rs`（纯编辑操作，含 kill-ring/undo）；`key_intent`/`apply_intent` 扩展编辑键与新退出语义（双击计时状态）；`view.rs` 输入区动态高度 + 光标渲染（`set_cursor_position`）；提交/中断路径改带 `streamingBehavior` 与 abort+clear_queue 组合。
- **测试**：现有 112 项需在输入重构后全部通过（大量 App 测试直用 `input: String`，需适配为 EditorBuffer）；新增编辑操作族/键位语义/steer 字段/队列恢复单测；假 pi PTY 验证流式 Enter 的请求字段与 esc 恢复；真机验收。
- **文档**：`tui-usage.md` 键位表大改（q/esc/ctrl+c/ctrl+d 语义变化需醒目提示）、`pi-tui-interaction.md` 差异表勾销 #1/2/3/4/8/10、`roadmap.md` 标注。
- **无协议/库层变更**：`streamingBehavior` 是 prompt 既有字段，abort/clear_queue 均为既有命令。
- **行为破坏性**：q 与空闲 esc 不再退出（对齐 pi 是本变更目的，文档显著提示新退出方式）。
