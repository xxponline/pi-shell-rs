# Design

## Context

- pi 参考实现全在 `pi-tui/dist/components/editor.js`：`historyIndex(-1=草稿态)`
  状态机、`navigateHistory(±1)`（首按捕获 `historyDraft`、回 -1 还原、取旧
  条目光标落开头/新条目落结尾、越界 no-op）、`cursorUp/cursorDown` 内嵌分派
  （首行且(空|浏览|col0)→历史否则行首；末行且浏览→历史否则行尾）、
  `addToHistory`（trim/相邻去重/头插/上限100）。**缺陷**：`populateHistory`
  从会话渲染回灌而 editor 无 clear、单例跨会话复用 → 重灌翻倍（explore 已
  查实 4 个回灌调用点 + 唯一写入处无清除口）。
- 我们现状：`EditorBuffer`（`src/app/editor.rs`）纯光标移动（首行 `move_up`
  no-op、末行 `move_down` no-op），零历史；TUI `key_intent` 有 Edit/Move 分类；
  GPUI `route_keystroke` 把 Up/Down 放行内建 `Textarea`；补全菜单开时两表面
  Up/Down 已归菜单；`alt+up` = Dequeue（自有语义，不冲突——历史只管无修饰键）。
- 决策（explore 已定）：**AA** = 双表面共享一套 + 会话/分支内隔离、幂等重建。

## Goals / Non-Goals

**Goals:**

- pi 手感的三件套：草稿捕获/还原、首按回行首、翻过最新回草稿。
- 幂等重建消灭 pi 的重复入栈缺陷；树导航/reload 后历史 = 当前分支 user 消息。
- 双表面同一 `EditorBuffer` 真源；补全菜单/浮层既有优先级不回归。

**Non-Goals:**

- 不持久化、不跨会话累积（A 决策：隔离；切会话/分支即以该 transcript 重建）。
- 不做 `ctrl+r` 反向搜索、历史搜索 UI、模糊查找。
- 不动 `alt+up` Dequeue、不动 pi RPC、不改 embed 注入。
- 不追求 GPUI 与 pi 视觉行（折行）粒度完全一致（见 D2/Risks）。

## Decisions

**D1 历史状态归 `EditorBuffer`（共享层单真源）。**
新增字段：`history: Vec<String>`（最新在前）、`history_index: Option<usize>`
（None=草稿态，等价 pi 的 -1）、`history_draft: Option<Snapshot>`（lines/row/col
快照，复用既有 Snapshot 形态）。纯函数面：`navigate_history(dir) -> Option<...>`
（返回需回写的新文本+光标落点，便于两表面各自回画）、`push_history(text)`、
`rebuild_history(iter)`（先清后灌）。备选「状态放各表面各自维护」被否——违背
共享层单真源与可测性惯例。

**D2 分派规则 pi 同源，两处本地适配。**
规则表（无修饰 Up/Down、编辑区、菜单关）：
- Up：`row==0 && (空 | 浏览中 | col==0)` → `navigate_history(-1)`；
  `row==0` 其余 → 回行首（**TUI 适配**：现 `move_up` 首行 no-op，分派层补
  `home()`，与 pi `moveToLineStart` 对齐）；`row>0` → 原生上移。
- Down：`row==last && 浏览中` → `navigate_history(1)`；`row==last` 其余 →
  行尾（**TUI 适配**：现 `move_down` 末行 no-op，补 `end()`，对齐 pi）；
  `row<last` → 原生下移。
- GPUI：分派条件用**逻辑行**（首行/末行 = `TextareaState` 的 row 边界）；
  条件成立 → 共享处理 + 回写 Textarea（复用 `push_editor_via_active_window`
  桥，RestoreQueued 同款）；条件不成立 → 放行原生移动。pi 用视觉行（折行），
  GPUI 输入框短内容为主，逻辑行近似——差异记已知差异（Risks）。

**D3 幂等重建模型（替代 pi 的渲染回灌）。**
`rebuild_history(entries 中 user 消息)` = `history.clear()` + 按时间序倒灌，
天然可重入；`push_history` 只在提交路径追加——下次重建整体覆盖，两路不打架。
保留 pi 的 trim/相邻去重/上限 100（防重建点之外的边界重复与无界增长）。
接线点（全量替换场景）：启动首载、`switch_session`/new、树导航/fork、reload；
日常流式 append 不触碰 history。修正缺陷的对照测试：同一 transcript 重建
两次 → 历史逐字节相同（pi 会翻倍的场景）。

**D4 浏览态退出（内容变更）两表面各自的检测面。**
TUI：`key_intent` 分类现成——Edit 类（打字/删除/粘贴类）→ 先
`exit_history_browsing()` 再执行；Move 类不退。
GPUI：插入/删除走 `Textarea` 原生不经共享——监听 `InputEvent`（文本变化
事件，现有桥）→ 共享 `exit_history_browsing()`（草稿丢弃、文本留驻）；
比逐键拦截更稳（覆盖粘贴/撤销/IME）。鼠标点击退出：TUI 选区点击已有通路
可顺带；GPUI 点击输入区 = 焦点行为，经 `InputEvent` 已覆盖内容面——纯点击
不改内容时浏览态保留（与 pi 鼠标退出有差异，记已知差异或按 PTY 再收）。

**D5 草稿语义。**
首次 `navigate_history` 进入（None→Some(0)）时 `push_undo_snapshot`（与
pi 同：进浏览可撤销）+ 保存 draft；回 None 还原 lines/row/col + 触发回写；
浏览中取条目：Up 落光标行首、Down 落行尾（`set_cursor` 由各表面回画）。
内容变更退出时 draft 置 None（丢弃），当前文本即成为新草稿。

**D6 键路由优先级不变。**
浮层 > 补全菜单（菜单开：Up/Down=菜单导航）> 历史分派 > 原生移动。
`route_keystroke`（GPUI）与 `key_intent`（TUI）只在「编辑区、无修饰、
菜单关、浮层关」落入历史臂——与 input-scroll 三区模型兼容（不新增 Route 态，
历史臂在共享分发内部）。

## Risks / Trade-offs

- [GPUI 逻辑行 vs pi 视觉行：折行长条目时首/末行判定偏差] → PTY 手工验证
  长输入/长历史条目；若手感明显再收紧到视觉行（不动规格——规格只钉条件语义）。
- [Textarea 原生移动与拦截边界的手感（首按回行首是否被原生抢先）] → 分派
  条件在 route 层先判（条件成立才放行/拦截），PTY 对照 pi 逐按验证。
- [重建点遗漏（fork/树导航后历史错源）] → D3 接线点清单 + 幂等重建场景
  测试覆盖每个点。
- [InputEvent 退出在程序化回写时误触发] → 回写走「标记抑制」（回写产生的
  change 不算用户变更）——实现期用一次性 guard，测试钉住。
- [TUI 末行 Down→行尾、首行 Up→行首改变既往 no-op 行为] → 既有键位测试
  若钉了 no-op 需按 pi 语义更新（规格 MODIFIED 已把 ↑↓ 跨行归入新语义）。

## Migration Plan

纯增量：共享字段默认空（无历史时 Up/Down 行为退化为现光标移动——除两处
no-op→home/end 适配）；回滚 = revert 提交。规格侧 tui-app MODIFIED 一句
（去「不提供输入历史」）+ 双 ADDED，归档时一次同步。

## Open Questions

（无——表面范围与会话语义已在 explore 定为 AA；其余细节均有上文决策或
留给 PTY 验收的 Risks。）
