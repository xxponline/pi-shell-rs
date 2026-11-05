# Design

## Context

4b 会话管理。grounding 事实（决定全部结构）：

- pi 中会话功能键位默认全部未绑定，入口是斜杠命令 → 必须先有本地斜杠分发。
- RPC 无 list/delete 会话命令；pi 列表=读目录（header + session_info 名 + mtime），
  删除=`trash`→`unlink` 文件、拒绝删活跃会话。
- `switch_session`/`fork`/`clone` 的 `rebindSession` 只重绑扩展、不回放历史 →
  成功后客户端必须 `get_messages` 自建状态。
- **`prompt` 在 compaction 期间直接 throw**（与流式缺 streamingBehavior 同款）→
  压缩中提交必须由客户端本地排队，补发时带 `steer`（idle/streaming 均合法：
  pi 仅在流式时消费该字段，空闲时忽略）。
- 会话目录优先级：`PI_CODING_AGENT_SESSION_DIR`（config.ts `ENV_SESSION_DIR`
  模板确认存在）> settings.sessionDir > `~/.pi/agent/sessions`。
- 树：`get_tree` → `SessionTreeNode{entry,children,label?}`；enter 在 pi 即
  `onSelect(entryId)`（无确认）→ 我们映射为 `fork(entryId)`；过滤 5 模式
  `default|no-tools|user-only|labeled-only|all`（`FilterMode` 实测）。
- 现有可复用：overlay 框架（4a）、`AsyncData` 通道与序号防过期、
  `RestoreQueued` 回填、`build_command_payload`（steer）、135 项测试基线。

## Goals / Non-Goals

**Goals:**
- 斜杠入口 + 6 类会话操作全部可验收；切换/fork/new 后界面如实反映新会话。
- 会话目录读取与删除是首次文件系统写操作——安全校验（目录包含、扩展名、
  活跃会话）必须有测试。
- compaction 全链路（触发/状态/失败/期间排队补发）用假 pi PTY 端到端验证。
- 所有新逻辑纯函数化单测；现有 135 项回归不破。

**Non-Goals：**
- 树内搜索/标签编辑/复制；选择器内改名（用 `/name`）；pi 其余本地命令
  （/settings /export /share /bug /login /model /thinking 等）；双击 esc（#5）；
  `!cmd` bash（#6）；列表渐进加载；Windows 键位；compaction 的自定义
  summarizer 选项。

## Decisions

### 1. 斜杠分发（`src/app/commands.rs`）

```rust
/// 本地命令表：名字 → 处理器（参数原文）
static LOCAL: &[(&str, fn(&mut App, &str))] = &[
    ("new", ..), ("resume", ..), ("tree", ..), ("fork", ..),
    ("clone", ..), ("session", ..), ("name", ..), ("compact", ..), ("quit", ..),
];
pub fn dispatch(app: &mut App, text: &str) -> bool  // true=已本地处理
```

- 挂点在 `submit_as` 之前（`handle_key` 的 Submit 路径）：`text.trim()` 后
  以 `/` 开头 → 取首段 token 查表 → 命中则执行并 `editor.replace("")`，
  返回"已处理"；未命中走原 prompt 路径（含 `/unknown`）。
- 斜杠命令的副作用（打开浮层/发命令/退出）全部落在 App 方法上，
  单测直接调 `dispatch`。
- 命令与 overlay 的关系：`resume→SessionSelect`、`tree→Tree`、`session→Info`；
  `new/name/compact/clone/quit` 直接发 RPC 或置退出。

### 2. 会话目录读取（`src/app/sessions.rs`，纯函数 + IO 分离）

```rust
pub struct SessionItem { path, name: Option<String>, first_message: String, modified: SystemTime }
pub fn resolve_sessions_dir() -> PathBuf      // env > settings > 默认
pub fn list_sessions(dir: &Path) -> Vec<SessionItem>
fn parse_session_file(path) -> Option<SessionItem>   // 逐行防御解析
```

- `parse_session_file`：首行必须是 `{"type":"session",...}`（否则 None=跳过）；
  后续行提取**最后一条** `session_info.name`、第一条 user 消息文本（截断 60 字
  作预览）、`message` 条目计数；坏行跳过。整文件读入（个人规模会话文件小，
  pi 也是全扫；不做渐进加载）。
- 目录优先级：`PI_CODING_AGENT_SESSION_DIR` → `settings.json.sessionDir` →
  `~/.pi/agent/sessions`（`~` 用 `$HOME`；settings 解析失败回退默认）。
- 列表按 `modified`（mtime）倒序。IO 与解析分离，解析可单测（合成字节）。

### 3. 切换/fork/new 状态重建链

统一模式（与 `cycle_model_prev` 同款的一次性线程链）：

```
发送命令 (switch_session/fork/clone/new_session)
  └─ cancelled → AsyncData{Cancelled} → notice，状态不动
  └─ 成功 → 续发 get_messages → AsyncData{Messages, data}
                └─ App: state.load_messages(&data) → draw
```

- `AsyncKind` 增 `SessionList`（选择器数据，带 seq 防过期）、`Tree`、`Stats`、
  `Messages`、`Cancelled`（带目标命令语义的 notice 文本）。
- `load_messages`（session 层）：见 §6。
- `new_session` 成功 → `get_messages` 通常只含 system → transcript 自然清空 ✓
  （同一链路，无需特判）。

### 4. overlay 扩展（`overlay.rs` 三个新变体）

```rust
Overlay::SessionSelect { items: Vec<SessionItem>, cursor, loading, seq, all_sessions: bool }
Overlay::Tree { rows: Vec<TreeRow>, cursor, folded: HashSet<entry_id>, filter: FilterMode, loading, seq }
Overlay::Info { title, lines: Vec<String>, loading, seq }
```

- **键位**（复用现有焦点路由，全部在 `overlay_key` 内分发）：
  - 通用：↑↓ 导航、Enter 确认、Esc 关闭；
  - SessionSelect 额外：`ctrl+d` 两段式删除（`confirming: bool` 字段——
    首次按下置位并改标题提示"再按一次确认"，esc 清除确认态而非关窗，
    再次 ctrl+d 执行）、`ctrl+a` 全部会话/当前 cwd 会话切换（pi 有 listAll
    双列表——**简化**：仅当前会话目录全量列表，`all_sessions` 字段预留不实现，
    记入已知差异）；
  - Tree 额外：←/→ 或 alt+left/right 折叠/展开、`ctrl+o` 过滤循环
    （模式序 default→no-tools→user-only→labeled-only→all，标题显示当前模式）。
- **Tree 数据构建**（纯函数，独立可测）：
  `flatten(tree, folded, filter, leaf_id) -> Vec<TreeRow { depth, text, id, is_leaf, has_children }>`：
  - 折叠：`folded` 集合中的节点隐藏其子树；
  - 过滤语义对照 pi `applyFilter`：恒隐 `usage` 条目；assistant 无文本且
    非错误/中止的纯工具消息隐藏（当前 leaf 例外）；no-tools 再隐
    `toolResult`；user-only 仅 `role==user` 的 message；labeled-only 仅
    带 label 的条目；all 全显；
  - 条目文本：`[role/类型] 内容预览`（消息取文本前 60 字，compaction/label
    等类型给固定标签）。
- **Enter 语义**：SessionSelect→`switch_session{path}` 链（§3）；Tree→
  `fork{entryId}` 链；Info→无动作。

### 5. 删除会话（安全优先）

`delete_session(path)`（App 方法，一次性线程内执行）：

1. canonicalize(path) 与 canonicalize(sessions_dir) —— 前者必须以 `.jsonl`
   结尾且父目录等于后者，否则拒绝（notice "路径校验失败"）；
2. `path == get_state.sessionFile` → 拒绝（"不能删除当前活跃会话"，
   对齐 pi 的 "Cannot delete the currently active session"）；
3. 先 `spawnSync trash path`（存在且成功 或 文件消失 → 成功），失败回退
   `std::fs::remove_file`；
4. 结果 notice + 重新拉取 SessionList 刷新选择器（seq 更新）。

两段式确认在浮层内完成（见 §4），执行只在第二次 ctrl+d 触发。

### 6. `load_messages`（session 层新增）

```rust
pub fn load_messages(&mut self, data: &Value)   // data = { messages: [...] }
```

- 遍历 `messages[]`：`system` 跳过；`user` → `Entry::User`（extract_user_text
  复用）；`assistant` → `Entry::Assistant{blocks}`（复用 content 解析：
  text/thinking/toolCall，tool 执行状态初始 Ended/未知由后续 toolResult 修正）；
  `toolResult` → 按 `toolCallId` 找**最后**一个含同 id ToolCall 的 assistant
  条目，把 `{content, details, structuredContent, isError, durationMs}` 并入
  其 `tool.result`（复用 merge 语义）；找不到则跳过。
- 末尾：`transcript` 整体替换、`phase = Idle`、`inflight` 清空、
  两队列清空、`compacting=false`、`usage` 置 None（快照不带）。
- `compaction_start/end` 归约：`compacting: bool` 字段 + 两组 Notice
  （成功：`压缩完成: {tokensBefore} → {estimatedTokensAfter} tokens`；
  失败/中止：红 error 通知文本）。`session.compacting` 供 submit 拦截。

### 7. compaction 期间本地排队（App 层）

- `App.compact_queue: Vec<String>`；`submit_as` 前置检查：
  `state.compacting` 为真 → 文本入 `compact_queue` + notice（"压缩中，已排队"），
  不发 RPC（否则 pi throw）。
- 归约收到 `compaction_end`（App 侧在 event 循环观察 `state.compacting`
  由 true→false 的边沿，或 session 提供 `take_compact_flush()` 返回待发队列）
  → 按序补发：每条用 `build_command_payload("prompt", text, /*streaming*/ true)`
  （恒带 steer，idle 时 pi 忽略、流式时合法排队）→ 队列清空。
- `compact_queue` 显示：并入既有队列带渲染（前缀 `[queued]`）。

### 8. 测试策略

- **合成会话目录 fixture**（temp dir）：正常会话（header+session_info+消息）、
  改名覆盖（多条 session_info 取最后）、无名字回退首条消息、坏首行文件、
  非 .jsonl 文件、空目录 → `list_sessions` 断言。
- **删除**：temp dir 真删（正常删/路径逃逸拒绝/活跃会话拒绝——活跃判定
  用注入的 current path）；trash 不可用时 unlink 回退（PATH 里无 trash 即触发）。
- **load_messages**：合成消息数组（含 toolResult 合并、缺字段、空数组、
  重复调用替换）。
- **斜杠分发**：命令表逐项（mock App 状态变化）+ 未知透传判定 + 空白。
- **树 flatten/filter/fold**：合成 SessionTreeNode 树，5 模式 × 折叠 ×
  leaf 例外逐项断言。
- **假 pi PTY**：switch 链（switch_session→get_messages→消息区变化）、
  fork 链、/compact（状态→期间排队→end→补发的 steer prompt 行）、
  /new 清空。
- **真机**：/resume 列表与切换真实会话、/tree 真实树、/name、/session、
  删除（对一个探针创建的废弃会话）、/compact（真实模型）。
- 回归：现有 135 项全绿 + clippy 零告警。

## Risks / Trade-offs

- **[文件系统操作越权风险]** → canonicalize + 父目录相等 + `.jsonl` 后缀
  三重校验，单测覆盖路径逃逸；删除先 trash（可恢复）后 unlink。
- **[会话文件格式演进]** → 防御式逐行解析、坏行跳过、首行强校验，
  解析失败仅影响该条目。
- **[compaction 补发时已进入流式/新状态]** → 恒带 steer payload，
  pi 对 idle 忽略该字段、streaming 消费之，两种状态均安全。
- **[树过滤语义与 pi 细微出入]** → 以 pi `applyFilter` 源码为对照实现，
  关键规则（usage 恒隐、无文本纯工具消息隐、leaf 例外）写入单测；
  逐像素差异记已知差异。
- **[斜杠命令与模型 slash混淆]** → 本地表命中才拦截；`get_commands` 返回的
  扩展/技能命令（`/skill:x` 等）不拦截、原样发给 pi（pi 侧自行处理）——
  与 pi 行为一致。
- **[AsyncData 竞态]**（快速连续切换）→ 每条链带 seq，仅当前浮层/当前链
  采纳；Messages 采纳前校验链 id。

## Migration Plan

纯增量 + session 层两个 ADDED（重建/compaction 标志），无既有行为变更。
顺序：斜杠骨架 → load_messages/compaction 归约 → 会话目录读取 →
SessionSelect → 切换链 → new/name/quit/session → Tree → 删除 →
compact 排队 → 文档/真机验收。回滚即 revert。

## Open Questions

- 树条目文本的预览截断长度（60 字）与 role 标签样式——PTY 观感微调，
  不影响结构。
- `/session` 面板字段粒度（先基本四字段，费用分项后续）——按需增。
