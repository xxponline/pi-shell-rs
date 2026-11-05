# Tasks

## 1. 命令表元数据与桩臂

- [x] 1.1 `src/app/commands.rs` 改为 CommandSpec 单源（24 条：name/description/argumentHint/行为分类），dispatch match 臂重排分组；**验证**：单测断言 KNOWN 覆盖 24 个 pi 内置命令名，既有 9 命令断言测试全绿
- [x] 1.2 桩命令臂（`/share` `/bug` `/login` `/logout` → 黄条提示原因、不发 RPC、清空输入）；**验证**：单测断言桩执行后无 RPC 发送、notice 文案含原因关键词

## 2. 补全纯逻辑

- [x] 2.1 新建 `src/app/autocomplete.rs`：复刻 pi 打分的多词元模糊过滤、`best_match`（精确 > 首个前缀）、`apply_completion`（`/前缀` → `/命令名 ` + 光标位）；**验证**：单测覆盖触发条件（行首 `/` 无空格才开、消息中部不触发、无匹配收起）、多词元过滤、预选、应用后文本与光标
- [x] 2.2 App 接线：文本变化重算 `CompletionState`，菜单键位分支（↑↓/PageUp/PageDown 导航、Esc 只关菜单、Tab 应用、Enter 应用后落入 submit）；**验证**：单测用 handle_key 序列断言菜单开关、选中移动、Tab 补全文本、Enter 触发对应命令执行

## 3. 补全渲染与参数补全

- [x] 3.1 `view.rs` 输入区下方菜单渲染：最多 5 条、`(i/n)` 计数、选中高亮、描述列弱化；**验证**：渲染单测断言菜单行、计数、超限截断
- [x] 3.2 参数补全：`/model ` 后候选取缓存的可用模型（`get_available_models` 首次拉取缓存）、`/thinking ` 后取可用级别，其余命令空格后不弹；**验证**：单测断言参数候选出现/不出现与应用后替换

## 4. 本地命令实现（copy/changelog/hotkeys/trust/reload）

- [x] 4.1 `/copy`：`get_last_assistant_text` → OSC52 输出剪贴板，无内容红条；**验证**：单测断言输出含 OSC52 与 base64 内容、空会话红条
- [x] 4.2 `/hotkeys` 键位表浮层（分组静态表、esc/回车关闭）；**验证**：单测断言浮层打开与关闭
- [x] 4.3 `/changelog`：CHANGELOG.md 多候选路径探测 + 可滚动浮层渲染，缺失黄条提示；**验证**：单测覆盖命中/缺失两分支（用临时目录构造）
- [x] 4.4 `/trust`：读写 `~/.pi/agent/trust.json`（decision 取值对照 pi `ProjectTrustDecision` 逐字对齐）+ 状态浮层；**验证**：单测 roundtrip（写入→读回→清除），格式与 pi 源码一致
- [x] 4.5 `/reload`：streaming/compacting 拒绝黄条，空闲时重读会话目录设置 + 绿条 + 重绘；**验证**：单测断言两分支

## 5. 选择器入口与简化浮层

- [x] 5.1 `/model`、`/thinking` 入口与参数直切：无参开既有选择器；精确匹配（大小写不敏感）直接发 `set_model`/`set_thinking_level`；`/thinking bogus` 红条列可用级别；**验证**：单测断言 RPC 发出与状态行更新、未知级别红条
- [x] 5.2 `/settings` 简化浮层：autoCompact/autoRetry 两开关（`set_auto_compaction`/`set_auto_retry`）+ 只读行，esc 关闭；**验证**：单测断言开关发送对应 RPC、浮层开关状态更新
- [x] 5.3 `/scoped-models` 只读浮层（当前模型 + 可用模型列表）；**验证**：单测断言浮层内容与无写入命令发出

## 6. 导出与导入

- [x] 6.1 `/export`：无参/`.jsonl` 本地复制会话文件（默认 `<cwd>/session-<id>.jsonl`），`.html` 走 `export_html` 异步链，失败红条；**验证**：单测断言三种路径行为与无会话文件红条，真机 `/export /tmp/x.jsonl` 产出文件
- [x] 6.2 `/import <path>`：确认浮层 → 复制进 `project_sessions_dir()` → 复用切换链重建；取消/源缺失不变更状态；**验证**：单测断言确认/取消/失败三分支，切换链被触发
- [x] 6.3 probe 真机验证 `switch_session` 能加载新复制进目录的会话文件（D6 风险）；**验证**：probe/PTY 脚本输出加载成功证据，若失败按 design Risks 降级并同步修订 spec 场景

## 7. 文档

- [x] 7.1 新建 `Docs/slash-commands.md`：24 命令三态表（✅实现/⚠️简化/🚫桩）+ 每项差异说明与原因（含 OSC52、默认导出路径、settings/scoped-models 简化面、桩命令原因）；**验证**：文档存在且 24 行全覆盖，抽查与实现一致
- [x] 7.2 `Docs/tui-usage.md` 增补补全键位与命令表，`Docs/pi-tui-interaction.md` 差异 #7 标记完成；**验证**：文档更新且与实现键位一致

## 8. 真机验收与门禁

- [x] 8.1 假 pi PTY 全链脚本：补全 Tab/Enter 提交、model/thinking 精确匹配与红条、桩提示、settings 开关 RPC、export/import、reload 拒绝；**验证**：脚本断言全 True（ALL OK）
- [x] 8.2 门禁：`cargo test` 全绿、`cargo clippy --all-targets` 零告警、`openspec validate add-remaining-slash-commands --strict` 通过；**验证**：三命令输出无 error/告警

## Workflow follow-up

- 提交：`git add -A && git commit`
- 归档：`/opsx-archive add-remaining-slash-commands`（同步 slash-autocomplete 新规格与 tui-app/session-management delta）
