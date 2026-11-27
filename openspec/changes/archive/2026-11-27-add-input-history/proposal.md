# Proposal

## Why

pi 原生 TUI 在输入框焦点时可用上下箭头翻历史输入（fish 式草稿语义：首按
回行首、再按进历史、翻过最新还原草稿），我们 TUI 与 GPUI 两表面均无此能力
（现状 Up/Down 只做光标移动）。pi 的实现本身有缺陷——会话渲染时回灌历史
却无清除口、头插去重只挡相邻重复，重灌即翻倍；我们有机会以「transcript 为
唯一源的幂等重建」模型复刻语义并修正该缺陷。

## What Changes

- 共享层 `EditorBuffer`（`src/app/editor.rs`）增历史三件套与纯逻辑：
  `history: Vec<String>` / `history_index: Option<usize>`（None=草稿态）/
  `history_draft`（进入浏览时捕获的完整编辑器状态）；`navigate_history(±1)`
  （越界 no-op、回草稿还原含光标与滚动、取旧条目光标落开头/新条目落结尾）、
  `rebuild_history(user_msgs)`（**先清后灌、幂等**——修正 pi 重复入栈缺陷）、
  `push_history(text)`（提交时追加，trim、相邻去重、上限 100）。
- 分派规则复刻 pi：**Up** 在首条可视行且（空编辑器 | 浏览中 | 光标列==0）→
  翻历史（否则回行首）；**Down** 在末条可视行且浏览中 → 翻新/回草稿
  （否则行尾）。浏览态退出语义：内容变更（打字/粘贴/删除）、鼠标点击、提交
  → 退出且草稿丢弃（浏览文本就地成为普通草稿）；纯光标移动不退出。
  补全菜单打开时 Up/Down 仍归菜单（现状保持）。
- **TUI**：`key_intent`/编辑通路接键（分派条件读共享 `EditorBuffer` 状态）。
- **GPUI**：`route_keystroke` 增 Up/Down 历史臂交共享分发（补全菜单、浮层
  既有优先规则不变）；与内建 `Textarea` 原生光标移动的分工按 design 的
  spike 结论落地（空编辑器/浏览态/行首列零三情形拦截，其余放行原生移动）。
- 重建时机：启动、树导航、reload（对应 pi `renderInitialMessages` 的调用
  场景，但先清后灌）；提交路径即时 `push_history`。
- 文档：`Docs/tui-usage.md` 键位表补 Up/Down 历史行与已知差异（如有）。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `tui-app`：新增「输入历史浏览」需求（TUI 上下箭头历史、草稿语义、幂等
  重建、退出矩阵、补全菜单优先）。
- `gpui-agent-terminal`：新增「输入历史浏览」需求（GPUI 路由与 Textarea
  分工、同一套共享历史语义、菜单/浮层优先规则不变）。

## Impact

- 代码：`src/app/editor.rs`（历史状态与纯逻辑 + 单测）、`src/app/mod.rs`
  （key_intent/分派接线、提交入栈、启动/树导航/reload 重建点）、
  `src/gpui_agent/terminal.rs`（route Up/Down 臂 + Textarea 回写桥）、
  可能 `src/gpui_agent/input.rs`（键翻译）。
- 不改：pi RPC、会话归约层、embed 注入机制、补全/滚动/浮层既有键路由。
- 测试：双矩阵（列 A TUI 纯逻辑 + 行为、列 B GPUI e2e）；文档
  `Docs/tui-usage.md`。
- 风险面：GPUI 拦截与 Textarea 原生移动的边界（首按/次按手感）依赖
  spike 实测，见 design。
