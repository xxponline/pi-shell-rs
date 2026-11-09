# Proposal

## Why

原生 pi 在光标前是空白（或空输入）时按 Tab 会打开当前目录的文件/目录选择列表
（`docs/usage.md`："Press `Tab` to complete a path."），我们的 TUI 没有任何纯路径
补全：`key_intent` 无 Tab 臂，菜单未开时按 Tab 静默无反应，用户无法用键盘浏览/
插入路径。`Docs/tui-usage.md` 已把"tab 补全"登记为后续独立变更，本次兑现。

## What Changes

- 新增**纯路径补全**：菜单未开时按 Tab，按光标前提取路径 token（空白/空输入 =
  空 token = 列出 cwd 顶层），readdir 前缀匹配生成候选（目录优先→字母序、无 20
  条上限、不跑 fd），开菜单展示。
- 新增 **force 态**：Tab 打开的菜单在继续输入时保持打开并按新 token 重过滤
  （可下钻 `src/` → `src/…`），仅在零候选、Esc、应用时关闭；退格到空白不关菜单
  （退回 cwd 列表，对齐 pi）。
- 新增**单条自动应用**：force + Tab 且恰有 1 条候选时直接插入，不弹菜单。
- 新增 `CompletionKind::Path` 应用规则：不加尾随空格、目录保 `/`、含分隔符自动
  包 `"`（无 `@` 前缀）、光标落插入文本之后。
- **BREAKING**（菜单键位）：`@` 文件菜单与 Path 菜单的回车改为**只应用不提交**
  （对齐 pi：仅斜杠菜单回车 fall-through 到提交）；斜杠菜单行为不变。因 openspec
  不支持场景改名，`file-completion` 的需求"菜单应用与键位"退役（REMOVED）、以新名
  "菜单键位与应用"重立（ADDED），行为除回车语义外不变。
- 菜单主列渲染从 `item.value` 改为 `item.label`（回落 value），对齐 pi 的
  `SelectList.getDisplayValue()`；`@` 菜单主列由 `@main.rs` 变为 `main.rs`，
  斜杠菜单视觉不变，Path 菜单主列为条目名（无描述列）。
- 修订主规格 `tui-app`："不提供输入历史与 tab 自动补全"不再成立。

## Capabilities

### New Capabilities

- `path-completion`: Tab 触发的纯路径补全——token 提取、cwd/目录 readdir 候选、
  force 态生命周期、单条自动应用、Path 应用规则与菜单键位。

### Modified Capabilities

- `file-completion`: 需求"菜单应用与键位"退役并以新名"菜单键位与应用"重立
  （REMOVED+ADDED，因场景"回车应用并提交"必须改名）：回车语义由"应用并提交"
  改为"只应用不提交"，其余键位与应用规则不变。
- `tui-app`: "基础输入编辑"需求中"不提供…tab 自动补全"改为指向
  `path-completion`（Tab 补全存在且由该能力定义）。

## Impact

- 代码：`src/app/mod.rs`（key_intent Tab 臂、Enter 按 kind 分流、force 态接线）、
  `src/app/autocomplete.rs`（路径 token 提取、readdir 候选、`Path` 应用规则、
  `CompletionState.force`）、`src/app/view.rs`（主列改渲染 label）。
- 测试：`mod.rs` Enter/Tab 断言分组、`view.rs` @ 渲染测试改断言、新增 Path
  纯函数单测；session/rpc 层零改动。
- 文档：`Docs/tui-usage.md`（补全菜单章节 + "tab 补全是后续独立变更"注销）、
  `Docs/pi-tui-interaction.md` 差异清单补行。
- 不在本变更（挂账）：token 边界缺 CJK 标点、不解 `([{` 包裹（@ 补全同样受影响）；
  斜杠菜单触发缺行首（`cursorLine==0`）限制。
