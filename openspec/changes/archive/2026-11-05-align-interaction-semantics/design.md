# Design

## Context

操作逻辑整理（`Docs/pi-tui-interaction.md`）给出差异清单，本变更处理 #1/2/3/4/8/10。
grounding 关键事实：

- pi `agent-session.prompt`：**流式中缺省 `streamingBehavior` 直接 throw**
  （"Specify streamingBehavior..."）——我们当前流式 Enter 正是缺省发送 → 现存真缺陷。
- pi 退出语义：ctrl+c 单击清空/双击(500ms)退出、ctrl+d 仅空输入退出、空闲 esc 无操作、
  **q 是普通字符**（我们当前 q 空闲即退——输入含 q 的单词会直接退出，相悖）。
- pi 流式 esc：`abort` + `restoreQueuedMessagesToEditor({abort:true})`（排队消息回编辑器）。
- pi 编辑器：Emacs 键族 + 多行（shift+enter/ctrl+j）+ kill-ring + undo（`ctrl+-`）；
  **输入历史默认未绑定**（`defaultKeys: []`）→ 本变更不做历史。
- 我们当前输入是追加式 `String`（无光标），`view` 输入区固定 3 行。

现有可复用件：`UiEvent::RestoreQueued`（alt+up 取回链路）、`CommandSender`、
`apply_intent`/`key_intent` 纯键位映射、`spawn_async` 一次性线程模式。

## Goals / Non-Goals

**Goals:**
- 差异 #1/2/3/4/8/10 全部落地并可验收；编辑操作为纯函数可单测。
- 输入重构不破坏既有 112 项测试（App 测试适配新输入 API）。
- 流式 Enter 的 steer 字段用假 pi 断言真实请求内容。

**Non-Goals：**
- 输入历史（pi 默认未绑定）、tab 自动补全、斜杠命令、bash 模式、
  ctrl+g/x/v（外部编辑器/剪贴板）、双击 esc 开树、Windows 键位。
- 编辑区水平滚动以外的高级排版（视觉换行与光标映射不做——保持一行一逻辑行）。

## Decisions

### 1. 输入状态：`EditorBuffer`（新 `src/app/editor.rs`）

```rust
pub struct EditorBuffer {
    lines: Vec<String>,     // 至少一行
    row: usize, col: usize, // 字符级光标（col 为字符序号）
    undo: Vec<Snapshot>,    // (lines, row, col) 快照，cap 100
    kill_ring: Vec<String>, // 新 kill 在前，cap 32
    yank_pos: usize,        // yank/yank-pop 游标
}
```

- **纯操作方法**：`insert(char)`、`newline()`、`backspace()`、`delete_forward()`、
  `move_*`（left/right/up/down/home/end/word_left/word_right）、
  `kill_to_line_start/end`、`kill_word_back/forward`、`yank`、`yank_pop`、
  `undo()`、`text()`（join "\n"）、`replace(text)`（set_editor_text/restore 用）、
  `append_block(text)`（队列恢复：换行追加到尾部）、`is_empty()`。
- **快照时机**：每个改变内容/光标的操作前压 undo 栈（同一连续输入流可按
  "自上次非编辑事件以来"合并——简化为每次按键一快照，cap 100 即可，内存无虞）。
- `App.input: String` 字段迁移为 `editor: EditorBuffer`；所有消费点
  （submit/can_submit/restore_queued/set_editor_text/view）改走 buffer API。
  现有测试中 `app.input = "x".into()` 等改为 `app.editor.replace("x")`。

### 2. key_intent 扩展（纯映射保持）

```
新增/改写意图：
  CtrlClear      ctrl+c（apply 内做 500ms 双击判定：超时→清空输入并记时；窗口内→Quit）
  ExitIfEmpty    ctrl+d（apply: is_empty → Quit，否则 DeleteForward）
  DeleteForward  非空 ctrl+d → 光标处向后删
  Edit           现有 catch-all 改为承载全部编辑键（见下）
  Abort          esc 流式（语义升级为 abort+restore，见 §4）
  （移除）Esc→Quit、Char('q')→Quit、BackTab/…保持
```

编辑键在 `apply_intent` 的 `Edit` 分支前直接分发到 `EditorBuffer`：
left/right/up/down/home/end（及 ctrl+b/f/a/e、alt+left/right）、
backspace、delete、ctrl+w/alt+backspace、alt+d、ctrl+u、ctrl+k、
shift+enter/ctrl+j（`KeyCode::Enter` + SHIFT / `Char('j')` + CTRL → newline，
注意与提交 Enter、alt+enter 的判定顺序：Enter 先判 SHIFT→newline、
ALT→followUp、否则 Submit）、ctrl+y、alt+y、ctrl+-（`Char('-')` + CTRL）。

`q` 与空闲 `esc` 不再有专属分支 → 自然落入 Edit/Ignore：
- `q` → Edit（普通字符）✓
- 空闲 esc → Ignore（无操作）✓（Esc 守卫只在 streaming 时返回 Abort）

### 3. 提交与中断语义（App 层）

- `submit_as("prompt")`：当 `state.phase == Streaming` 时 payload 增加
  `"streamingBehavior":"steer"`；follow_up 路径不变。
- `SendFollowUp`（alt+enter）：`streaming → follow_up 命令`；`空闲 → submit_as("prompt")`
  （等同 Enter，不带 streamingBehavior）。
- **Abort 升级**（流式 esc）：一次性线程内顺序执行
  `abort`（AcceptError）→ `clear_queue`（RequireSuccess）→
  回报既有 `UiEvent::RestoreQueued(data)` 回填输入框。
  clear_queue 失败 → SendFailed（红条），abort 已生效不回滚。
  空队列时 clear_queue 返回空数组 → `restore_queued` 早退，输入不变 ✓。

### 4. 渲染：动态高度 + 光标 + 水平偏移（view.rs）

- 输入区高度 `= clamp(逻辑行数 + 2(边框), 3, 8)`；消息区 `Min(1)` 相应伸缩，
  三/五区结构不变（规格"独立区域"仍满足）。
- 文本渲染：按 `scroll_x` 水平偏移截取每行（保证光标列可见，
  `scroll_x = max(0, col - (width-4))` 按需回看），避免长行光标消失。
- 光标：`frame.set_cursor_position` 定位到 (row, scroll_x) 对应屏幕坐标；
  输入框边框保留，光标画在文本区。
- 多行显示：`lines` 逐行渲染（逻辑行不做视觉换行——水平滚动替代，
  与 pi 的视觉换行差异记入已知差异，"基本无差别"容忍）。

### 5. 测试策略

- **editor.rs 纯函数单测**（大头）：每类移动/删除/kill/yank-pop/undo 的
  边界（行首行尾、空缓冲、跨行、词边界、CJK 字符安全）、快照 cap。
- **App 键位语义**：ctrl+c 单/双击（注入时间或用真实 Instant 间隔）、
  ctrl+d 双语义、q 进输入、空闲 esc 无操作、Enter 三分支
  （SHIFT 换行 / ALT followUp / 普通提交）。
- **steer 字段**：假 pi 记录 prompt 行，断言流式态 payload 含
  `streamingBehavior:"steer"`、空闲态不含。
- **esc 恢复**：假 pi 返回 clear_queue 数据 → 断言 abort 与 clear_queue
  两条 stdin + 输入框回填。
- **view**：动态高度（1 行与 8 行输入）、光标坐标、水平偏移、
  超长多行不 panic（TestBackend）。
- **回归**：既有 112 项适配后全绿；真机 PTY 清单（见 tasks 末组）。

## Risks / Trade-offs

- **[输入重构面大]** → 分两步提交级推进：先 EditorBuffer + 适配测试（语义不变），
  再加新键位与语义；每步全量测试兜底。
- **[ctrl+c 双击计时依赖真实时钟]** → 单测用注入时间（`apply` 接受 now 参数）
  或宽松断言；PTY 真机验证。
- **[水平偏移与光标换算错误]** → view 单测断言光标坐标与 scroll_x 关系；
  CJK 双宽字符按字符数近似（列宽差异记已知差异，"基本无差别"容忍）。
- **[abort+clear_queue 双命令竞态]**（abort 处理中队列被 pi 清）→
  clear_queue 返回即回填，pi 的队列状态以响应为准；失败走红条。
- **[行为破坏：q/esc 不再退出]** → 文档显著提示 + ctrl+d/双击 ctrl+c 可发现
  （输入框 hint 更新）；这是对齐 pi 的目的本身。
- **[undo 每键一快照的内存]** → cap 100，单快照仅行数组，可忽略。

## Migration Plan

顺序：EditorBuffer 核心 + 测试适配（语义不变的重构）→ 移动/删除族 →
kill-ring/undo → view 动态高度与光标 → 键位语义切换 → 提交/中断语义 →
PTY/真机验收与文档。回滚即 revert；无数据/协议迁移。

## Open Questions

- undo 快照粒度（每键 vs 每操作段）实现时取简者，规格只要求"最近一次编辑"可撤销。
- alt+y（yank-pop）的环游标语义按"替换上一次粘贴"实现；若与 pi 细节有出入，
  记入已知差异（规格场景已定义可观察行为）。
