# Proposal

## Why

路线图 4b：会话管理是 pi 的核心导航面（resume 历史会话、树内分支、新建/改名/删除、手动压缩）。grounding 发现三个决定范围的事实：(1) pi 中这些功能的键位**默认全部未绑定**，唯一入口是斜杠命令（`/resume /tree /new /name /compact...`）——不做斜杠设施则 4b 无任何触发点；(2) RPC **没有** list/delete 会话的命令，pi 本身也是直接读会话目录、用 trash/unlink 删文件；(3) `switch_session`/`fork` 后 pi 的 `rebindSession` 只重绑扩展、**不回放历史**，客户端必须自行用 `get_messages` 重建状态。

## What Changes

- **斜杠命令设施**（修改 `tui-app`）：输入以 `/` 开头且命中本地命令表 → 本地执行、不发送模型（执行后清空输入）；未命中的 `/xxx` 按普通文本发送（与 pi 一致）。本变更落地命令集：`/new /resume /tree /fork /clone /session /name /compact /quit`。
- **会话列表与切换**（新增能力 `session-management`）：斜杠 `/resume` 打开会话选择器——直接读会话目录（优先级：`PI_CODING_AGENT_SESSION_DIR` > settings.sessionDir > `~/.pi/agent/sessions`），解析每文件的 header（id/timestamp/cwd）、最新 `session_info` 名称、文件 mtime，列表显示"名称或首条消息预览 + 相对时间"；选中 → `switch_session` → **用 `get_messages` 重建状态**。
- **会话树**：`/tree` 打开树浮层（数据 `get_tree`）——↑↓ 导航、折叠/展开、5 种过滤模式循环（default / no-tools / user-only / labeled-only / all，对照 pi 的 `applyFilter` 实现）、回车 = `fork(entryId)` 分支到该节点（含状态重建与 notice）、esc 取消。树内搜索/标签编辑/复制**不做**（后续）。
- **新建 / 重命名 / 删除**：`/new` → `new_session` 并清空界面（状态重建）；`/name <n>` → `set_session_name`（无参提示用法）；会话选择器内 `ctrl+d` 删除——**两段式确认**、拒绝删除当前活跃会话、路径必须 canonicalize 后位于会话目录内且以 `.jsonl` 结尾，删除按 pi 语义先试 `trash` 再 `unlink`，完成后刷新列表。
- **compact 流程**：`/compact [自定义指令]` → `compact` 命令；进行中界面可见（compacting 标志 + 通知），完成通知含 token 压缩量，业务失败（如 "session too small"）红色通知；**compaction 期间 Enter 本地排队**（pi 的 `prompt` 在 compaction 中直接 throw，必须由客户端排队，compaction 结束后按序补发，每条带 `streamingBehavior:"steer"` 以兼容随后进入流式的情况）。
- **`/session` 信息面板**：简化版会话信息浮层（`get_session_stats` 的名称/消息数/token/费用等基本字段），esc 关闭。
- **状态机扩展**（修改 `session-state`）：`load_messages`——从 `get_messages` 快照重建 transcript（system 跳过、user/assistant 映射、toolResult 按 toolCallId 并入既有 ToolCall 块，重建后 phase=Idle、队列清空）；`compacting` 标志与 compaction start/end 通知归约。
- **不在本变更范围**：树内搜索/标签编辑/复制、选择器内改名（用 `/name`）、pi 其余 17 个本地斜杠命令（/settings /export /share /bug /login /model /thinking 等，选择器已有键位入口或属独立功能）、双击 esc 开树（差异 #5，仍后置）、`!cmd` bash 模式（差异 #6，独立 change）、列表渐进式加载（pi 的 onProgress；我们一次加载）、Windows 键位。

## Capabilities

### New Capabilities

- `session-management`: 会话生命周期与导航的用户可观察行为——列表与切换、树浏览与分支、新建/重命名/删除、手动 compact、会话信息面板，以及切换后界面反映新会话内容。

### Modified Capabilities

- `tui-app`: 新增「斜杠命令分发」——本地命令表路由、未知 `/` 文本透传模型、命令执行后清空输入。
- `session-state`: 新增「从消息快照重建」（`load_messages`）与「compaction 状态与通知」（`compacting` 标志 + start/end 通知）。

## Impact

- **代码**：`src/app/` 新增 `sessions.rs`（目录读取/列表解析/删除）、`commands.rs`（斜杠命令表与分发）、树与会话选择器并入 `overlay.rs` 新变体；`view.rs` 新增信息面板与队列带扩展；`mod.rs` 接线（斜杠分发、AsyncData 新 kind、compaction 排队）；`src/session/` 新增 load_messages 与 compaction 归约。
- **文件系统访问**：会话目录读取与删除是 RPC 之外的首次文件系统操作——必须做路径 canonicalize + 目录包含校验，只碰 `.jsonl`。
- **测试**：合成会话目录 fixture（header/session_info/坏行）单测；假 pi PTY 覆盖 switch/fork/new 重建链与 compaction 排队补发；删除用临时目录真删验证；现有 135 项回归。
- **文档**：tui-usage（斜杠命令与树键位）、pi-tui-interaction（#5 仍列后置）、roadmap 4b 标注。
- **无破坏性变更**。
