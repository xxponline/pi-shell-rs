# Tasks

## 1. 斜杠命令分发骨架

- [x] 1.1 新建 `src/app/commands.rs`：本地命令表（9 项）+ `dispatch(app, text) -> bool` 骨架与 Submit 路径挂点（命中→执行+清空输入，未命中→走原 prompt，空白不触发）；验证单测：命中/未命中（/unknown 透传判定）/空白/单独 "/" 四分支
- [x] 1.2 `/quit` 与 `/name`（有参发送 set_session_name、无参仅提示用法）落地到 dispatch；验证单测：quit 置退出标志、name 两分支的 RPC 命令与输入清空
- [x] 1.3 现有 135 项回归全绿 + clippy 无新增告警；验证两项命令

## 2. session 状态机扩展（load_messages + compaction）

- [x] 2.1 `SessionState::load_messages(&Value)`：system 跳过、user/assistant 映射、toolResult 按 toolCallId 并入最近 ToolCall、重建后清空 phase/inflight/队列/compacting；验证单测：完整对话重建、toolResult 合并、重复调用整体替换
- [x] 2.2 load_messages 防御：缺 content、空数组、坏消息类型不 panic；验证三类单测
- [x] 2.3 compaction 归约：`compacting` 标志 + start/end 通知（成功含 tokensBefore→estimated、失败含 errorMessage 红色语义由 kind 区分）；验证单测：开始→成功结束、失败清标志、phase/消息不受影响

## 3. 会话目录读取

- [x] 3.1 `src/app/sessions.rs`：`resolve_sessions_dir()`（`PI_CODING_AGENT_SESSION_DIR` > settings.sessionDir > `~/.pi/agent/sessions`）+ `parse_session_file`（首行 header 校验、最后 session_info 名、首条 user 消息预览 60 字、坏行跳过）；验证单测：合成文件六类（正常/改名覆盖/无名回退/坏首行/非 jsonl/坏行混入）
- [x] 3.2 `list_sessions`（mtime 倒序、目录不存在返回空）+ 设置解析失败回退；验证单测：排序、空目录、坏 settings.json

## 4. 会话选择器与切换链

- [x] 4.1 `Overlay::SessionSelect`（items/cursor/loading/seq/confirming）+ `overlay_key` 键位（↑↓/Enter/Esc、ctrl+d 两段式确认）+ `view` 渲染（名称或首条消息预览、相对时间、当前会话标记、确认态标题）；验证状态单测与 TestBackend 渲染断言
- [x] 4.2 `/resume` → 异步 `SessionList`（seq 防过期）填充浮层；验证过期序号丢弃 + 加载态→列表态单测
- [x] 4.3 切换链：Enter → `switch_session` 成功续发 `get_messages` → `load_messages` 重建；cancelled → notice 不动状态；验证假 pi PTY：切换后消息区显示目标会话内容、取消路径无变化

## 5. 新建 / 树 / 会话信息

- [x] 5.1 `/new` → `new_session` 链（同 §4.3 重建，消息区自然清空）+ `/clone` 同链；验证假 pi PTY：/new 后消息区清空、cancelled 有提示
- [x] 5.2 `Overlay::Tree` + `flatten` 纯函数（缩进/折叠集/leaf 标记/5 过滤模式，对照 pi applyFilter：usage 恒隐、无文本纯工具消息隐、leaf 例外、no-tools 隐 toolResult、user-only、labeled-only、all）；验证单测：5 模式 × 折叠 × leaf 例外逐项
- [x] 5.3 `/tree` 打开（异步 `get_tree`）+ 键位（↑↓ 导航、←→ 折叠展开、ctrl+o 过滤循环且模式显示在标题、Enter=fork 链重建、Esc 取消）；验证状态单测 + 假 pi PTY：fork 后消息区变为分支内容
- [x] 5.4 `/session` → `Overlay::Info`（异步 `get_session_stats`：名称/消息数/token/费用，加载态、失败 notice、Esc 关闭）；验证单测与假 pi PTY

## 6. 删除会话

- [x] 6.1 `delete_session(path)`：canonicalize 三重校验（.jsonl + 父目录=会话目录 + 非当前活跃会话）→ trash 失败回退 unlink → 结果 notice；验证单测：temp dir 真删成功、路径逃逸拒绝、活跃会话拒绝（注入 current path）、trash 缺失走 unlink
- [x] 6.2 ctrl+d 两段式确认接线（首次置确认态改标题、Esc 清确认态不关窗、二次执行删除后刷新列表）；验证状态单测：三步状态机 + 删除失败确认态复位

## 7. compact 流程

- [x] 7.1 `/compact [指令]` → `compact` 命令（可带 customInstructions）+ 输入清空；compacting 标志驱动界面状态（状态行/通知）；验证假 pi PTY：触发→进行中→完成通知（含 token 量）全链
- [x] 7.2 压缩期间本地排队：`state.compacting` 时 submit 入 `compact_queue`（队列带显示 `[queued]`、不发 RPC 不报错），compaction_end 边沿按序补发（恒带 steer）；验证假 pi PTY：压缩中提交→排队可见→结束后 prompt 行发出且含 streamingBehavior=steer、期间无红色错误
- [x] 7.3 compact 业务失败（too small 等）→ 红色错误通知、回空闲；验证假 pi PTY 失败路径

## 8. 验收与收尾

- [x] 8.1 真机 PTY/终端验收清单：/resume 列表与真实切换（历史会话内容出现）、/tree 真实树（导航/折叠/过滤/fork）、/new、/name 生效于状态行、/session 面板、对废弃会话删除（先由探针创建）、/compact 真实压缩（含期间排队）；验证逐项目测/目测，差异记入 tui-usage 已知差异（如选择器无列表内改名、无 listAll 双列表）
- [x] 8.2 文档：`Docs/tui-usage.md` 斜杠命令表与树/选择器键位、`Docs/pi-tui-interaction.md` 差异表更新（#5 仍后置说明、斜杠落地进度）、`Docs/roadmap.md` 4b 标注；验证文档与实现一致
- [x] 8.3 全量 `cargo test` 与 `cargo clippy --all-targets` 零告警；`openspec validate add-session-management --strict` 通过；验证三项命令输出
