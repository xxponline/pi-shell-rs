# pi-shell-rs 路线图

> 本文档固化产品定位、复刻范围、工程结构与落地阶段，作为后续所有变更的基准，防止方向漂移。
> 相关文档：[`rust-tui-architecture.md`](./rust-tui-architecture.md)（总体架构）、
> [`rpc-protocol-notes.md`](./rpc-protocol-notes.md)（RPC 协议实测笔记）、
> [`probe-usage.md`](./probe-usage.md)（探针使用说明）。

## 1. 产品定位（不再漂移的核心结论）

- **形态 B：Rust 自己产出终端字节流。** 程序直接运行在**通用终端**（iTerm / xterm.js / 系统 console 等只负责显示 ANSI 的终端模拟器）里，由 Rust 代码画出完整界面。
- **渲染层用 ratatui + crossterm**（Rust 终端 TUI 事实标准），不自己造终端框架，也不依赖另一个专用 UI 进程。
- 通过拉起 **`pi --mode rpc`**（无头核心）做控制与定制；pi 核心零改动。
- **目标是"体验级一致"，不是"像素级一致"**：功能一致、布局一致、用起来基本无差别即可。不追求换行/着色/编辑器逐字符行为 100% 相同。
- 我们是**可控宿主**：在保持 pi 现有使用体验的前提下，做自己的定制（权限、会话限制、工具策略、自动化等）。

### 明确不做

- 不做交互式登录：依赖原生 pi 预登录（`~/.pi/agent/auth.json`）。
- 不套 pi 的 interactive TUI 做 PTY 套壳（路线 2，已否决）。
- 不做纯结构化数据后端交给别的渲染进程（形态 A，已否决）。
- 不追求像素级复刻（编辑器 kill-ring/undo 树、mermaid/latex、彩蛋等重特性按需后置）。

## 2. pi 原生 TUI 勘察结论

pi 的 TUI 分两层，边界清晰：

```
业务层  coding-agent/src/modes/interactive  (~22.5k 行)
        interactive-mode.ts (7243 行) + components/ (48 个)
        订阅 AgentSessionEvent，维护状态，决定"显示什么"
        组件 render(width) 产出字符串行
────────────────────────────────────────────────────────
框架层  packages/tui  (~41k 行, @earendil-works/pi-tui)
        layout(flexbox) / editor(2483 行) / markdown(1161 行)
        differential rendering / ANSI / 键盘解析 / 鼠标 / kitty 图片
```

在我们的形态 B 里，框架层用 ratatui+crossterm 等价替代；业务层需要在 Rust 侧重写。

### 渲染模型（决定我们的架构）

pi 的渲染是：组件树 `render(width) -> String[]` → 容器拼行 → diff 写 ANSI。
Rust 侧对应：**维护一个与终端无关的"会话状态机"，ratatui 只负责把它画出来。**

### RPC 覆盖度（已验证）

从 keybindings 反推的功能目录里，绝大部分动作都有现成 RPC 命令
（prompt/steer/follow_up/abort/set_thinking_level/set_model/cycle_model/
new_session/switch_session/get_tree/fork/set_session_name/compact/
extension_ui_* 等）。键盘与渲染全部在客户端，pi 核心零改动——架构判断成立。

## 3. 功能等价矩阵（复刻范围与优先级）

### 第一档：MVP，缺了就"不是 pi"

- 流式聊天：用户 / assistant 气泡
- 流式 text delta 累积（按 contentIndex）
- thinking 块：显示 + 折叠
- 底部编辑器：输入 + 发送（一期多行基础编辑即可）
- esc 中断 / ctrl+c 清空
- alt+enter follow_up 队列；steer 打断
- 状态行：model / thinking / tokens（/cost）
- 工具调用 streaming：tool_execution_start/update/end
- markdown：代码块 + 基本高亮
- abort / 错误 / auto-retry 提示

### 第二档：重要

- 工具卡片 7 种，优先 read / bash / edit，再 write / grep / find / ls
- 会话选择器（resume）
- 会话树：fork / 导航 / 过滤
- 模型选择器 / 模型循环
- 手动 compact 流程
- extension UI 弹窗（9 种 method，优先 select / confirm / notify）
- 外部编辑器（$EDITOR）
- 新建 / 重命名 / 删除会话

### 第三档：可后置或定制时砍掉

- 图片渲染（kitty / sixel）
- mermaid / latex
- easter-egg 3D 等彩蛋
- first-time-setup 向导、oauth/provider 登录选择器
- 主题选择器
- bug 上报 / session share

## 4. 屏幕布局（"布局一致"只需对齐这五区 + 弹层）

```
+------------------------------------------------------------------+
| 标题: 会话名 / 模型                                   (setTitle)    |  顶栏
+------------------------------------------------------------------+
|                                                                    |
|                        滚动消息区 (ScrollView)                       |
|   user / assistant / thinking / tool cards / compaction           |
|                                                                    |
+------------------------------------------------------------------+
| [extension widget: aboveEditor]                                   |  可选 widget
| status: model · thinking · tokens · cost             (footer)     |  状态行
+------------------------------------------------------------------+
| >>> 多行编辑器                                                      |  输入区
| [extension widget: belowEditor]                                   |
+------------------------------------------------------------------+

弹层 (overlay / alt-screen): model / session / tree / thinking / settings / extension dialogs
```

### 一期编辑器范围（不追求等价 2483 行）

需要：多行输入、光标移动（上下左右/行首行尾/单词）、插入/退格/回车换行/删除、
bracketed paste 粘贴、发送 / esc 中断 / ctrl+c 清空 / alt+enter follow_up。

二期才补：kill-ring、undo 树、自动补全、外部 $EDITOR、图片粘贴。

## 5. 工程结构（lib + 两个 bin）

关键决策：抽出一个 lib crate，让 `probe`、未来的主 TUI、离线测试共享 rpc/session 层。

```
pi-shell-rs/
├── Cargo.toml
├── src/
│   ├── lib.rs                    # pi_shell_rs 库入口
│   ├── bin/
│   │   ├── probe/                # 已有探针 → 迁移为 use pi_shell_rs
│   │   └── tui.rs                # 最终主程序
│   ├── rpc/                      # 连接层（从 probe 提炼，强类型化）
│   │   ├── process.rs            #   spawn pi --mode rpc / 三线程
│   │   ├── jsonl.rs              #   wire 类型 / id 注入
│   │   ├── commands.rs           #   30 种命令的强类型 builder
│   │   ├── events.rs             #   AgentSessionEvent 强类型枚举
│   │   └── client.rs             #   id 关联响应 + 事件广播
│   ├── session/                  # ★ 无头状态机（不依赖 ratatui，fixture 可测）
│   │   ├── mod.rs                #   SessionState：当前会话权威状态
│   │   ├── stream.rs             #   message_update delta 累积 / contentIndex
│   │   ├── transcript.rs         #   结构化条目：User/Assistant/Thinking/Tool/...
│   │   ├── tools.rs              #   工具调用/结果归一化
│   │   ├── queue.rs              #   steering / follow_up 队列
│   │   └── replay.rs             #   从 tests/fixtures JSONL 回放
│   ├── app/                      # ★ TUI 应用层（ratatui，依赖 session）
│   │   ├── mod.rs                #   event loop：键盘 tick + rpc 事件 + 渲染
│   │   ├── layout.rs             #   顶栏 / 消息流 / widget / 状态行 / 编辑器
│   │   ├── chat.rs               #   滚动消息区
│   │   ├── editor.rs             #   一期多行编辑器
│   │   ├── footer.rs             #   状态行
│   │   ├── markdown.rs           #   pulldown-cmark + syntect
│   │   ├── tools/                #   read bash edit write grep find ls 卡片
│   │   └── dialogs/              #   model/session/tree/thinking + extension UI
│   └── config.rs                 # PI_BIN / keybindings（后期）
├── probe-fixture-extension/      # 探针用 fixture 扩展
├── tests/
│   ├── fixtures/probe/...        # 真实场景快照（已有）
│   └── session_replay_test.rs    # 离线：fixture → SessionState 断言
└── Docs/
```

> 阶段 0+1 的库模块与回放用法见 [`library-and-replay.md`](./library-and-replay.md)。

依赖单向，保证可测：

```
app (ratatui)  ──► session (纯逻辑)  ──► rpc (连接)  ──► pi
                        ▲
              tests/replay (fixtures JSONL)
```

## 6. 落地阶段（每阶段都可验收）

### 阶段 0：抽 lib

把 probe 现有的 process/jsonl/classify 迁入 `src/rpc`，probe 改为调用 lib。

- 验收：probe 行为不变，现有 13 个测试全部通过。

### 阶段 1：无头 session 状态机（离线，最该先做对）

fixture 回放 → 结构化 transcript：text delta 累积、thinking、多 turn、
tool start/update/end 归一化。不引入 ratatui。

- 验收：text_only / tool_call / failed_tool 的 snapshot 能重建成结构化"对话"，
  并由离线测试断言；不烧 token、CI 可跑。

### 阶段 2：最小可见 TUI（接真 pi，ratatui 首秀）

顶栏 + 滚动消息流（先纯文本，不做 markdown）+ 基础编辑器；esc 中断；流式显示。
使用方式与键位见 [`tui-usage.md`](./tui-usage.md)。

- 验收：程序跑起来能与 pi 聊天，能看到流式回复并中断。

### 阶段 3：体验对齐 ✅（已完成）

markdown 高亮 / thinking 折叠 / 工具卡片（read/bash/edit）/ 状态行 tokens·cost /
follow_up 队列 / auto-retry 提示。

- 验收：与原生 pi 并排对比，布局与功能"基本无差别"。（已按清单逐项 PTY 验证，
  详见 change `align-chat-experience`）

### 阶段 4+：选择器与会话管理 / extension UI 弹窗 / compact / 其余工具卡片

model/session/tree/thinking 选择器，extension_ui 9 种 method，write/grep/find/ls
卡片，新建/重命名/删除会话。图片、主题、mermaid 等按需后置。

拆分进度（方案 A）：
- **4a add-overlay-dialogs ✅**：overlay 框架 + model/thinking 选择器与模型循环 +
  extension UI 9 method + write/grep/find/ls 卡片
- **4b add-session-management ✅**：斜杠设施 + 会话选择器/切换、会话树（导航·折叠·
  过滤·fork）、新建/克隆/重命名/两段式删除、compact（含压缩期间本地排队补发）、
  /session 信息面板、切换后状态重建（get_messages）
- **待办（已挂账）：滚动增强剩余项**——消息区整页回看
  （`add-message-scrollback`）、鼠标滚轮（`add-mouse-wheel-scroll`）、
  滚动条（`add-message-scrollbar`：瞬态显隐/悬停/拖拽 scrub 全量）、
  prompt 跳转 + 顶/底 + 半页（`add-prompt-jump-scroll-keys`：ctrl+↑/↓、
  ctrl+home/end、shift+PageUp/Down）均已落地。剩余：alt-screen 搜索
  （ctrl+shift+f）；行级滚动销账（pi `lineUp/lineDown` 默认键即为空 +
  我们无键位配置系统 → 已对齐）；宽度变化重换行的 offset 漂移接受
  （pi 同样漂移）。
- **待办（已挂账）：工具卡片对齐 change 的移出项**——后续工具卡片对齐（运行中
  流式输出/计时器/背景面板/头行/截断警告）明确不含以下四项，原因各异：
  ① edit 执行中预计算 diff：渲染层需在 args 到齐后异步读文件出 preview，
     侵入渲染与执行的边界；
  ② 语法高亮：read/write 内容与 edit diff 需引入 highlightCode 同等能力；
  ③ 结果图片块渲染：依赖 Kitty/iTerm2 图片协议整套实现；
  ④ args 流式分帧渲染（每帧仅渲染一次）：纯性能优化，无体感差异。

操作逻辑对齐 ✅（change `align-interaction-semantics`）：按
[`pi-tui-interaction.md`](./pi-tui-interaction.md) 差异清单完成 #1/2/3/4/8/10——
流式 steer、ctrl+c/ctrl+d/esc 语义、队列恢复、多行编辑器（光标/词行删除/
kill-ring/yank/undo）。其余小项见差异表（#11 扩展快捷键——无扩展运行时、
#12 选择器分页、#9 残差：选区复制/剪贴板文件列表粘贴）。

## 7. 既定技术决策记录

- 运行时：探针已用 std 三线程（无 tokio）。主 TUI 的并发模型（继续 std +
  多 channel，还是引 tokio）推迟到阶段 2 结合键盘事件 / rpc 事件 / 渲染 tick
  再定，不在阶段 0/1 预判。
- 可执行文件名是 `pi`（v1.5.0），命令为 `pi --mode rpc`，用 `PI_BIN` 覆盖。
- fixture 扩展通过 `pi -e <path>` 注入，不安装到 `~/.pi/agent/extensions/`。
- 原始探针产物 `probe-out/` 与时间戳 run 不入库；截断后的 snapshot 入库供离线测试。
