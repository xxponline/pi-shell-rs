# Rust 原生 TUI 客户端 —— 架构设计

> 目标：用 Rust 重写一套终端 TUI，通过 RPC 协议驱动 pi 核心，不改 pi 源码（或仅极少补丁）。

## 最终架构

```
宿主终端 / xterm.js / Electron terminal
        │  PTY (stdin/stdout 字节流)
        ▼
┌─────────────────────────────┐
│  Rust TUI 进程               │
│  - 终端渲染（聊天流、弹窗）   │
│  - JSONL 客户端              │
│  - 内存事件总线衔接两者       │
└──────────────┬──────────────┘
               │  JSONL stdin/stdout
               ▼
┌─────────────────────────────┐
│  pi-agent --mode rpc         │
│  - 无头核心，不渲染任何东西    │
│  - AgentSessionEvent 全透传   │
└─────────────────────────────┘
```

关键：pi 在 RPC 模式下 stdout 全是 JSONL，不产生 ANSI。Rust 自己画终端，宿主只是个终端窗口。

## pi 侧现成契约（已验证，0 入侵）

### 协议入口
- 启动：`pi-agent --mode rpc`
- stdin：JSON 命令（每行一个）
- stdout：JSON 事件流 + 命令响应（每行一个）
- 定义位置：`packages/coding-agent/src/modes/rpc/rpc-types.ts`、`rpc-mode.ts`

### 已有命令（RpcCommand）
prompt / steer / abort / clear_queue / new_session / get_state / set_model /
cycle_model / get_available_models / set_thinking_level / cycle_thinking_level /
set_steering_mode / set_follow_up_mode / compact / set_auto_compaction /
set_auto_retry / abort_retry / bash / abort_bash / get_session_stats /
export_html / switch_session / fork / clone / get_fork_messages /
get_entries / get_tree / get_last_assistant_text / set_session_name /
get_messages / get_commands

### 事件流
`rpc-mode.ts` 直接透传 `AgentSessionEvent`（定义在 `core/agent-session.ts:196`），
包括 agent_end / entry_appended / usage / auto_retry_* / compaction_* /
queue_update / thinking_level_changed / session_info_changed。

### 扩展 UI 通道（免费双向）
扩展调用 `ctx.ui.*` 自动变成 stdout 的 `extension_ui_request` 事件：
select / confirm / input / editor / notify / setStatus / setWidget / setTitle / set_editor_text。
Rust 端回复 `extension_ui_response { id, value | confirmed | cancelled }` 即可。
——扩展弹窗就是 Rust 弹窗，不需要 side-channel。

## 原则：不改 pi 源码

所有现有 RPC 命令和事件流都够用。如果后面发现真正的缺口，再单独讨论打补丁。

- 工具结果渲染：按 `AgentMessage` 原始 JSON 在 Rust 端自绘
- 跨项目会话列表：暂不实现（当前会话内的 `get_tree`/`get_entries` 已有）
- 交互式 auth：用户提前跑一次原生 pi 登录，凭据落盘 `~/.pi/agent/auth.json`

### 会话切换限制（如需要）
不动 RPC。用 pi 扩展机制：放一个 TS 扩展到 `~/.pi/agent/extensions/`，
监听 `session_before_switch` 事件返回 `{ cancel: true }`。
详见 `docs/session-switch-restriction.md`。

## 会话文件路径（可配置）

默认 `~/.pi/agent/sessions/`，三种覆盖方式（优先级从高到低）：
1. CLI：`--session-dir <dir>`
2. 环境变量：`PI_CODING_AGENT_SESSION_DIR`
3. settings.json：`"sessionDir": "..."`

## Rust 侧工作量（新工程自行规划）

- JSONL 客户端：命令发送 + id 关联响应 + 事件分发
- 终端渲染组件：聊天滚动区、markdown、代码高亮、编辑器输入框、alt-screen 弹窗
  （对标 pi `packages/tui` 约 2 万行，只复刻需要的功能集）
- 工具结果卡片：read/edit/bash/grep/write —— 按 `AgentMessage` 原始 JSON 自绘

## 动手前必须做的验证（关键）

不写任何 UI，先用脚本当 client 跑一遍完整对话：

```bash
pi-agent --mode rpc
# 逐行喂 JSON：get_state → prompt → 观察全部事件流
```

输出一份"事件类型清单"，逐条对照 Rust UI 要渲染的状态。
所有缺口（比如某类 UI 信号只走 TUI 没进事件流）在这一步暴露，
先记录下来，评估后再决定是否讨论补丁。

## 明确不做的事

- 不套 pi 的 interactive TUI（PTY 套壳方案已放弃）
- 不改 `packages/tui`（继续被原生 pi 使用）
- 不改 `AgentSession` 核心
- 不实现交互式登录（依赖原生 pi 预登录）
