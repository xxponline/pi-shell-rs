# TUI 使用说明（阶段 2：最小可见界面）

路线图阶段 2 的产物：用 ratatui 在通用终端里复刻 pi 的最小交互闭环。
协议与库层见 [`rpc-protocol-notes.md`](./rpc-protocol-notes.md)、
[`library-and-replay.md`](./library-and-replay.md)。

## 前置条件

- 本机已安装 `pi`（实测 v1.5.0），并**用原生 pi 完成过登录**
  （`~/.pi/agent/auth.json`）。本 TUI 不做交互式登录。
- 必须在**真实交互终端**里运行（需要 raw mode；非 TTY 会报
  "无法进入终端 raw/alt-screen 模式"并退出）。

## 启动

```bash
cargo run                       # 默认运行 TUI（default-run）
# 或指定：cargo run --bin pi-rust-tui
# 或先构建：cargo build && ./target/debug/pi-rust-tui
```

- `pi` 不在 PATH 时：`PI_BIN=/path/to/pi cargo run`
- 启动流程：进入 TUI → 拉起 `pi --mode rpc` → `get_state` 探活（15s）→
  进入界面。任何一步失败都会**恢复终端**后打印可读原因并以非零码退出。

## 斜杠命令（24 个全部可用）

完整命令表、每个命令的实现/简化/桩状态与差异原因见
[`slash-commands.md`](./slash-commands.md)。要点：

- 会话类：`/resume`（选择器+两段式删除）· `/tree`（导航/折叠/过滤/分支）·
  `/new` · `/clone` · `/fork` · `/session` · `/name` · `/compact`（压缩中本地排队）·
  `/quit`
- 选择器入口：`/model [搜索]`（精确匹配直切）· `/thinking [级别]`（未知级别红条）
- 信息与文件：`/copy`（OSC52）· `/changelog` · `/hotkeys` · `/export` · `/import`（确认后导入并切换）
- 设置类：`/settings`（简化：两开关）· `/scoped-models`（只读）· `/trust` · `/reload`
- 🚫 提示桩：`/share` `/bug`（网络上报）· `/login` `/logout`（不做交互式登录，靠预登录）
- 未命中的 `/xxx` 作为普通消息照常发送（与 pi 一致）——其中**扩展/skill/模板
  命令由 pi 内部拦截执行**；补全菜单会列出它们（描述带 `[extension]`/`[prompt]`/
  `[skill]` 标签），`/reload` 可刷新列表

### bash 模式（`!` 前缀）

- `!ls -la` 回车 → 走 `bash` RPC 执行，输出在消息区**实时增长**，完成后落为
  条目（`exit N` / `cancelled` / `truncated → 路径` 状态行）
- `!!cmd` → 结果**不进模型上下文**（条目带"（不进上下文）"标记）
- 仅 `!` 无命令体 → 按普通消息发送
- 运行中再提交 `!cmd` → 黄条警告（Esc 可中止）且**输入保留**；Esc 中止当前命令
- 压缩期间 `!cmd` 也可执行（对齐 pi：bash 先于压缩排队）

### 外部编辑器与剪贴板

- **Ctrl+G** 挂起 TUI（退出 alt-screen/raw、暂停键盘读取）→ 编辑器于
  `pi-editor-*/prompt.md` 打开当前输入 → 退出码 0 读回去 BOM/末尾换行回填；
  命令不存在/非零退出 → 黄条提示且输入不变
- **Ctrl+V** 平台后端（零第三方依赖）：macOS 图片 `osascript` JXA / 文本
  `pbpaste`；Linux `wl-paste`→`xclip`；Windows PowerShell。后端缺失/超时
  静默降级到下一环，均失败则无操作（不弹红条）。图片写入 `pi-clip-*.{ext}`
  临时文件后插入其路径
- 差异（对齐 pi 时省略）：无"复制选区"分支（本 TUI 无选区）、无剪贴板
  **文件列表**粘贴（链为 图片→文本）

### 补全菜单

输入行首 `/` 即弹出候选：模糊过滤（说明列 = `参数提示 — 说明`）、↑↓/PageUp/PageDown
导航、Tab 应用、**Enter 补全并立即提交**、Esc 只关菜单；`/model `/`/thinking `
空格后有参数候选。最多 5 条，超出显示 `(i/n)` 滚动。

## 界面布局

```
○ idle  session-name  thinking:off  model:xxx  1.2k/340↓/8kcache $0.012
                                          ← 状态行（phase·会话名·模型·用量/费用）
┌────────────────────────────────────┐
│ You: 你的消息                       │  滚动消息区（自动跟随最新内容）
│ # H1 heading                       │  assistant 回复按 markdown 渲染
│ **bold** 与 `code`、列表、引用        │  （标题/粗体/斜体/行内代码/链接）
│ ┌─ rust ─                          │  代码块：边框+语言标签+语法高亮
│ │ fn main() { … }                  │
│ └─                                 │
│ <thinking N chars>                 │  思考块（默认折叠，Ctrl+T 展开）
│ ● read Cargo.toml · done · 12ms    │  工具卡片（红=error 黄=running 绿=done）
│ exit 0                             │  展开态含结果/diff（Ctrl+O 切换）
└────────────────────────────────────┘
[queue] 排队中的 follow_up 消息        ← 队列带（空队列不占行，最多 2 行）
[widget] 扩展 setWidget 的行              ← widget 区（未设置不占行）
┌ input (enter: send · q/esc: quit) ┐  输入框
│ 输入的文本                         │
└───────────────────────────────────┘

浮层（居中覆盖，Esc 关闭后底层无损）：
┌─ Model (ctrl+l) ─────────────┐   模型/thinking 选择器、
│ > MiMo-V2.6-Pro   (provider) │   扩展 select/confirm/input/editor
└──────────────────────────────┘   对话（editor 为多行：Enter 提交·Alt+Enter 换行）
```

## 键位

> **键位语义已对齐原生 pi**（见 [`pi-tui-interaction.md`](./pi-tui-interaction.md)）。
> 注意：`Shift+Enter` 与 `Ctrl+-`（undo）依赖 **kitty 键盘增强协议**（启动时自动探测，
> iTerm2/kitty/wezterm 支持；不支持的终端请用 Ctrl+J 换行）。启动探测最多等待 2 秒。

| 按键 | 行为 |
|---|---|
| 可打印字符 / 退格 | 编辑输入（多行缓冲，光标处编辑） |
| Enter | 发送 prompt（空/空行不发送）；流式中 = **steer 打断重定向**（带 streamingBehavior） |
| Shift+Enter / Ctrl+J | 换行（多行输入，输入区随行数增高、上限 8 行） |
| Alt+Enter | 流式中排队 follow_up；**空闲时等同普通 Enter** |
| Alt+Up | 取回全部排队消息到输入框（clear_queue；队列空时无操作） |
| 光标 | ←→/Ctrl+B/F、↑↓ 跨行、Home/Ctrl+A、End/Ctrl+E、Alt+←→（词） |
| 删除 | Delete（向后删）、Ctrl+W/Alt+Backspace（后删词）、Alt+D（前删词）、Ctrl+U（删至行首）、Ctrl+K（删至行尾） |
| kill-ring | Ctrl+K/U/W/D 的删除入环；Ctrl+Y 粘贴、Alt+Y 回退粘贴（yank-pop） |
| Undo | Ctrl+-（kitty 终端）撤销最近一次编辑 |
| Ctrl+C | **单击=清空输入；500ms 内双击=退出** |
| Esc ×2（空输入，500ms 内） | **打开会话树**（按 `doubleEscapeAction` 设置：tree/fork=开树，none=禁用；`/settings` 可改）——单次 Esc 无动作 |
| `!command` 回车 | **执行 bash**（不发给模型）；输入以 `!` 起始时边框变黄提示；`!!command` 结果不进模型上下文 |
| Esc（bash 运行中） | 中止当前 bash 命令（优先于双击开树；输入非空也生效） |
| Ctrl+G | **外部编辑器**：挂起 TUI 打开编辑器（`settings.json externalEditor` → `$VISUAL`/`$EDITOR` → nano），保存退出回填输入；失败黄条且输入不变 |
| Ctrl+X | 复制最后一条回复到终端剪贴板（OSC52，同 `/copy`） |
| Ctrl+V | 粘贴剪贴板：图片→临时文件路径插入，否则文本→光标处插入；空则无操作 |
| Ctrl+D | **仅输入为空时退出**（非空 = 光标处向后删字符） |
| Esc（流式中） | 发送 abort **并恢复排队消息到输入框**，回到空闲 |
| Esc（空闲） | **无操作**（不退出、不清空——对齐 pi） |
| Ctrl+T | 切换 thinking 块展开/折叠（默认折叠为单行占位） |
| /resume·/tree 内 ctrl+d | 会话两段式删除（再按一次确认；拒绝删除活跃会话） |
| /tree 内 ctrl+o / ←→ | 过滤模式循环 / 折叠展开子树 |
| Ctrl+O | 切换工具卡片输出展开/折叠（默认显示头行+前 3 行） |
| Ctrl+L | 打开模型选择器（↑/↓ 导航，Enter 应用，Esc 取消） |
| Ctrl+P / Shift+Ctrl+P | 循环切换模型（下一个 / 上一个） |
| Shift+Tab | 循环 thinking 级别 |
| Alt+T | 打开 thinking 级别列表选择 |
| Ctrl+F1 等 | F 系列与未绑定组合均被忽略（不插入字符） |

**退出方式只有两种**：`Ctrl+D`（输入为空）与 `Ctrl+C` 双击。`q` 不退出；空闲 `Esc`
单击无动作、**双击（空输入）打开会话树**
（`q` 是普通字符——输入含 q 的单词不会误退出，与 pi 一致）。

流式期间按 q **不会**退出（会作为文本进入输入框），与 pi 的"esc 中断优先"语义一致。

## 与原生 pi 的已知差异

- **不支持的 markdown**：表格按原文纯文本显示；LaTeX/mermaid 不渲染
  （降级为暗色原文/代码样式）；图片不显示（显示 `(image: url)` 提示）。
- **模型/会话选择器、compact 流程**尚未实现（阶段 4）。
- 消息区只自动跟随最新内容，暂不支持向上滚动回看。
- Windows 键位差异（pi 在 Windows 用 ctrl+q 等）未做平台分支——当前只按
  macOS 键位实现。
- 工具卡片覆盖 read/bash/edit/write/grep/find/ls（其余走通用卡片兜底）。
- **Alt+T 打开 thinking 列表**与 `/thinking` 斜杠命令**都可用**（两者等价，
  pi 原生只有后者）；级别循环 Shift+Tab 与 pi 一致。
- 扩展 `editor` 弹层为终端内最小多行编辑（Enter 提交 / Alt+Enter 换行），
  不含 pi 编辑器的 kill-ring/undo/外部 $EDITOR（Ctrl+G）等高级能力。
- 选择器暂无输入即筛（fuzzy filter），靠 ↑/↓ 导航。
- 斜杠命令 24 个全部可执行（实现/简化/桩见 `slash-commands.md`）；树内搜索/
  标签编辑未实现；`@文件`/路径补全未实现（pi 的 fd 集成）。
- 会话列表仅当前项目（pi 的 listAll 双列表未实现）。
- **Shift+Enter / Ctrl+- 依赖 kitty 键盘协议**（启动时探测，最多 2s；不支持的
  终端上换行请用 Ctrl+J，undo 无替代键）。
- 输入区超长单行按水平滚动显示（不视觉换行）；无输入历史/自动补全
  （pi 的历史键默认也未绑定，tab 补全是后续独立变更）。

## 排查

- **启动报"无法启动 pi"**：检查 `which pi` / `PI_BIN`，或先用原生 pi 登录。
- **报"pi 探活失败 (get_state)"**：模型/网络未就绪，先在原生 pi 里跑通一次。
- 退出后终端乱码/卡 raw mode：Ctrl-C 或 Ctrl-\ 退出；正常路径与 panic
  都会自动恢复（alt screen + 光标 + canonical mode）。
- 疑似"没反应/不显示回复"：先用 `PI_RUST_TUI_LOG=/tmp/prt.log cargo run`
  复现，再看日志里的 `rpc` 行（模型错误会以红色 `[error]` 通知显示）。
