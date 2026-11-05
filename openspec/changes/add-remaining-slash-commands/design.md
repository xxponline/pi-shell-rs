# Design

## Context

4b 落地了斜杠分发骨架（`src/app/commands.rs` 的 `KNOWN` 表 + `dispatch`）与 9 个命令；输入编辑在 `src/app/editor.rs`（多行 EditorBuffer），渲染在 `view.rs`。pi 侧补全实现在 `pi/packages/tui/src/autocomplete.ts`（CombinedAutocompleteProvider）+ `components/editor.ts`（SelectList 菜单、键位、预选），命令元数据在 `core/slash-commands.ts`（BUILTIN_SLASH_COMMANDS）。RPC 可用方法中与本变更相关的有 `export_html`、`get_last_assistant_text`、`set_auto_compaction`、`set_auto_retry`；**没有** settings 全量读写、scoped models、reload、import 方法（见 proposal.md）。动机见 proposal.md - Why。

## Goals / Non-Goals

**Goals:**
- 补全菜单行为对齐 pi（触发/模糊/预选/键位/应用/参数补全），纯本地、零 RPC
- 24 个命令全部可按回车执行且行为可观察、可测试（真实现/简化/桩三态）
- 补全与命令表共用一份元数据，后续加命令只改一处
- 交付 `Docs/slash-commands.md` 记录实现/简化/桩状态与差异原因

**Non-Goals:**
- `@文件` 与路径补全（pi 的 fd 集成，差异表未列）
- `/share` `/bug` `/login` `/logout` 的真实现（网络/交互登录约束）
- 完整 settings / scoped-models 写入能力（RPC 缺方法，只做简化版）
- HTML 导出模板复刻（走 pi 的 `export_html` RPC）
- 扩展命令/技能命令的补全（pi 侧来自 extensionRunner，本 TUI 无该运行时）

## Decisions

### D1. 补全作为独立纯逻辑模块 + App 状态
新建 `src/app/autocomplete.rs`：`CompletionState { open, prefix, items, selected }` 与纯函数
`fuzzy_filter(items, query)`、`best_match_index(items, prefix)`、`apply_completion(text, prefix, value)`。
复刻 pi `fuzzy.ts` 的多词元 AND + 打分（按分数降序，不改稳定序）。菜单状态存 `App`，
文本每次变化后调 `update_completion()` 重算；渲染在 `view.rs` 输入区下方（最多 5 条 +
`(i/n)`）。**理由**：无 TTY 单测覆盖触发/过滤/预选/应用，符合"纯逻辑抽无 TTY 单测"分层约定。
**替代**：把菜单塞进 Overlay——被否，菜单是输入态子状态而非模态浮层（Esc 语义不同）。

### D2. 命令表元数据单源
`commands.rs` 的 `KNOWN` 从 `&[&str]` 扩为 `&[CommandSpec { name, description, argument_hint,
arg_completion }]`；`dispatch` 的 match、补全候选、参数补全开关（仅 `model`/`thinking`）都读它。
桩命令以 `Behavior::Stub(原因文案)` 表达，分发臂统一出黄条。

### D3. 键位分发顺序
`handle_key` 中补全菜单分支置于**编辑器输入之前、overlay 之前**：菜单打开时
↑↓/PageUp/PageDown 只动选中、Esc 只关菜单、Tab 应用、Enter 应用后**落入 `submit_as`**（与 pi
"fall through to submit" 一致）；其余键照常进编辑器。菜单不与模态 overlay 并存（overlay 打开
时编辑器不收键，补全自然关闭）。**理由**：pi 的菜单是 editor 内部态，不抢全局键。

### D4. 剪贴板用 OSC52
`/copy` 输出 `\x1b]52;c;<base64>\x07` 到 stdout。**理由**：零依赖、跨平台；pi 用系统剪贴板
库（差异记入文档）。降级（pbcopy/xclip 探测）不做。

### D5. 导出路径与实现
- 无参/`.jsonl`：`std::fs::copy(state 当前会话文件, dst)`，默认路径 `<cwd>/session-<sessionId>.jsonl`
  （pi 默认路径行为记入文档差异）
- `.html`：async 链发 `export_html`（复用 `AsyncKind` 既有模式），成功通知路径
- `session_file` 字段 4b 已有，直接复用；无会话文件时红条提示

### D6. 导入 = 复制进项目会话目录 + 切换链
`/import` 确认浮层（复用 extension confirm 的通用确认 Overlay）→ `fs::copy` 到
`project_sessions_dir()`（4b 已有，文件名取源文件名，保证 `ts_sessionId.jsonl` 前缀可被
`parse_session_file` 解析）→ 复用 4b 的 `spawn_session_chain` 切换链（get_state+get_messages
重建）。**风险**：pi 的 `switch_session` 是否加载从未见过的文件需 probe 验证——见 Risks。

### D7. trust.json 直接读写
`~/.pi/agent/trust.json` 为 `{ <规范化cwd>: <decision> }` 映射（源码
`core/trust-manager.ts`），用带缩进 JSON 原子写回（tmp+rename）。浮层显示当前 cwd 的信任状态
并提供"信任本项目/清除"两项。decision 取值在实现时对照 `ProjectTrustDecision` 类型确认。

### D8. changelog 用浮层而非消息区
`CHANGELOG.md` 在 pi 包目录（`config.ts getChangelogPath`），启动后按候选路径探测一次
（`PI_BIN` 相对位置、`./pi` 仓库根、`~/.pi` 相关安装位）。内容渲染进**可滚动浮层**而非消息区：
消息区是 transcript，`load_messages` 重建会清掉本地插入的条目（4b 已确立重建语义）。
**替代**：塞 transcript——被否，切换会话即丢失且污染归约。

### D9. settings 简化面
浮层仅两项开关（autoCompact→`set_auto_compaction`、autoRetry→`set_auto_retry`）+ 当前模型等
只读行（`get_state` 有的字段才显示）。状态先由 `get_state` 回填，缺失字段显示"—"。
scoped-models 浮层纯只读（当前模型 + `get_available_models` 全列表）。

### D10. reload 的拒绝条件
检查 `state.phase == Streaming || state.compacting` → 黄条拒绝；否则重读会话目录设置
（`resolve_sessions_dir` 的输入）+ 绿条完成 + `draw`。**注意**：不重启 pi 进程（pi 侧扩展重载
无 RPC 可达，差异记入文档）。

### D11. 文档交付
`Docs/slash-commands.md`：24 命令三态表（✅实现 /⚠️简化 /🚫桩）+ 每行差异说明与原因；
`Docs/tui-usage.md` 增补补全键位；`Docs/pi-tui-interaction.md` 差异 #7 标记完成。

## Risks / Trade-offs

- **`switch_session` 不加载外部导入文件** → ✅ 已真机验证（任务 6.3）：pi 的
  `runtimeHost.switchSession(path)` 经 `SessionManager.open(path)` **直接按路径打开文件**，
  复制进项目目录后切换成功（"已导入并切换"绿条）。残留约束：session header 的 `cwd`
  必须存在（pi `assertSessionCwdExists`），失效目录的会话导入会失败——apply_import 的
  非法校验已拦首行，cwd 缺失场景由 pi 侧报错经 SendFailed 呈现
- **trust.json 格式/decision 取值猜错** → 实现时读 `ProjectTrustDecision` 与读写函数逐字对齐，
  写前读回验证
- **CHANGELOG.md 定位失败**（安装形态多样）→ 多候选路径探测 + 缺失时可见提示（spec 已含）
- **OSC52 在部分终端被禁** → 复制仍提示成功（无法回读剪贴板）；文档记录
- **Enter 补全即提交可能误触** → 与 pi 完全一致的行为；预选最佳匹配降低误触率
- **命令数增多使 `commands.rs` 变大** → CommandSpec 单源 + 按行为分组的 match 臂，保持可读

## Open Questions

- `/export` 无参默认路径（D5 定为 `<cwd>/session-<id>.jsonl`）是否需要跟随 pi 实际默认——实现时
  跑一次真机 `/export` 对照，若差异显著再调整（不改 spec，只改文档记录）
