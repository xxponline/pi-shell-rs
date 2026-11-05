# Design

## Context

阶段 2 交付了可用的最小 TUI（chat.rs 纯函数映射 + view.rs 三区布局 + wrap.rs 底部锚定换行），
session 状态机已归约 transcript/队列/usage，48 个测试离线可跑。本变更在不破坏"纯函数可测"
结构的前提下把聊天面做到与 pi 基本无差别。

已勘察的事实：
- pi 用 `marked` 解析 markdown，代码高亮走可插拔 `highlightCode` 回调，宿主传入
  **highlight.js**（markdown 代码块与 read/write 工具渲染共用）。
- read 工具结果：`{content: TextContent[], structuredContent}`（structuredContent 为文件原文）。
- edit 工具结果：`details.diff`（结构化 diff，`EditToolDetails.firstChangedLine`）+ args 中的
  old/new 文本可作降级。
- bash 工具结果：`content` 文本（output）+ 截断标记，`fullOutputPath` 可能存在。
- 我们的 `Usage` 缺 cost；`get_state` 响应（model/sessionName/thinkingLevel）目前被丢弃；
  `auto_retry_*` 事件被显式忽略。

## Goals / Non-Goals

**Goals:**
- markdown（结构 + 代码语法高亮）、thinking 折叠、read/bash/edit 工具卡片、
  状态行 tokens/cost/model、队列可见 + alt+enter/alt+up、retry/error 通知。
- 全部渲染路径保持纯函数（状态 + 展开标志 + 宽度 → 行），可离线单测。
- 流式重绘性能可接受：高亮/解析结果按内容缓存，避免每帧重算。
- 新增 probe 场景归档 bash/edit 真实 fixture，供离线卡片测试。

**Non-Goals（本变更明确不做）：**
- 表格、LaTeX、mermaid、图片；write/grep/find/ls 专用卡片（通用卡片兜底）。
- 会话/模型选择器、compact 流程（阶段 4）。
- 主题系统、编辑器多行历史/自动补全、图片粘贴。
- 向上滚动回看（仍自动到底）。

## Decisions

### 1. markdown 管线：pulldown-cmark + syntect（选定）

```
assistant 文本 --pulldown-cmark--> Event 流 --映射--> Vec<Line<'static>>（带样式 span）
                                    │
代码块围栏 ──lang──► syntect highlight ──► 着色行（缓存）
```

- **pulldown-cmark**：Rust 生态事实标准，纯 Rust、快、支持 CommonMark + GFM 扩展，
  对不完整输入（流式未闭合围栏）安全（解析为未闭合段落/代码块，不报错）。
- **syntect**：highlight.js 的 Rust 对应物，自带默认语法集与主题。
  取舍：编译时间增加（约 1-2 分钟首次）、二进制增大约 10-30MB（默认语法集）——
  个人工具可接受；用 `default-themes` 仅取一个深色主题、语法集用默认全集
  （常见语言都有，无需 two-face）。备选与否决：
  - *两阶段手写高亮（tree-sitter）*：依赖重、语言覆盖要逐个接，否决。
  - *纯结构不高亮（零依赖）*：代码块无着色，与 pi 并排对比明显掉档，否决。
  - *自研关键词正则高亮*：工作量与维护成本高于收益，否决。
- 主题选择：固定一个深色主题（syntect `base16-ocean.dark` 或同级），主题系统留阶段 4+。

**流式与缓存**：渲染每帧对完整文本重新解析；解析/高亮按 `(宽度, 内容哈希)` 缓存
（`HashMap`，容量上限防膨胀）。未闭合代码围栏按 pi 同样策略"补一个临时闭合"渲染，
避免流式中代码块闪烁。缓存使重复帧成本接近零，只有内容变化的块重算。

### 2. 渲染分层（chat.rs 内部重构）

```
chat.rs
├── chat_lines(state, flags) -> Vec<Line>        # 入口：条目循环
│     flags = { show_thinking: bool, expand_tools: bool }
├── markdown.rs (新)  text -> Vec<Line>          # 纯 markdown → styled lines
├── tools.rs (新)     Block::ToolCall -> Vec<Line>  # 卡片格式化器注册表
│     match tool_name { "read" => read_card, "bash" => bash_card, "edit" => edit_card, _ => generic_card }
└── （thinking/user/notice 保持现有路径）
```

- **flags 下沉为参数**：thinking 折叠与工具展开是 App 级 UI 状态（`ctrl+t`/`ctrl+o`
  维护的 bool），不进 SessionState（它是 UI 偏好，不是会话事实）；`chat_lines(state, flags)`
  保持纯函数签名可测。
- **markdown 只作用于 assistant/user 的 Text 块**；thinking/tool/notice 走各自卡片。
- user 消息保持现状还是 markdown？pi 的 user 气泡按原文显示（带前缀）。保持原文不解析——
  用户输入的 `*` 等不应被解释，且与 pi 一致。

### 3. 工具卡片（tools.rs）

每张卡片 = 头行（`● read · path · done · 12ms`）+ 展开体：

| 工具 | 头行 | 展开体（`ctrl+o`） |
|---|---|---|
| read | 文件路径（args.path/file_path） | `structuredContent` 或 `content[].text` 的内容预览（限行数） |
| bash | 命令摘要（args.command） | `content[].text` 输出；截断时提示 `fullOutputPath` |
| edit | 文件路径 + diff 摘要（±行数） | `details.diff` 结构化红绿行；缺失时降级 args 的 old/new 对比 |
| 其它 | 工具名 + 状态 | `args_json` 摘要 + result content 摘要 |

- 结果形状**防御式读取**（`content[]` / `structuredContent` / `details` 都可能缺），
  读不到就降级，绝不 panic——pi 协议会演进，卡片不能脆。
- 折叠态默认：头行 + 结果首 3 行（`expand_tools=false`）；`ctrl+o` 全局切换。
  （pi 的 `app.tools.expand` 即全局开关，对齐之。）
- diff 红绿：`details.diff` 若为 pi 的结构化 hunk 数组则逐行渲染；否则对
  args.oldString/newString 做行级 LCS 简易对比（自研 ~60 行，无新依赖）。
- **fixture**：probe 增加 `tool_bash`（跑 `echo` 小命令）与 `tool_edit`（改一个临时文件
  再改回）两个场景，`--snapshot` 归档；卡片单测直接回放这两个 fixture 断言渲染输出。

### 4. 状态行（view.rs header）

```
● streaming  session-name  model:xxx  thinking:high  1.2k↑/340↓/8kcache  $0.012  [queue:2]
```

- **cost 来源**：`Usage` 增加 `cost_input/output/cache_read/cache_write/total: f64`
  （`from_json` 读 `cost.*`，缺失为 0）。显示用 `cost.total`，格式化函数独立可测
  （`0 → 不显示`、`<0.01 → $0.004`、`≥1 → $1.23`）。
- **model/sessionName/thinkingLevel**：新增 `SessionState::apply_get_state(&Value)`，
  从 preflight 响应写入（字段缺失保留旧值）。App 在 preflight 后调用一次；
  `thinking_level_changed` 事件照常覆盖。
- usage 刷新来自既有 `message_update` 归约，无需新事件路径。

### 5. 队列 UI 与键位（view.rs + mod.rs）

- **显示**：输入框上方一条队列带（有内容才占行，高度 ≤2，超出截断显示 `…N more`）；
  每条 `▸ <text>` 弱化色，steering 与 follow_up 分别标注前缀。空队列不渲染该带
  （布局动态：`Constraint::Length(队列行数)`）。
- **键位**（crossterm KeyEvent 的 modifiers 判定）：
  - `alt+enter`（KeyCode::Enter + ALT）→ `follow_up` 命令；输入照常清空。
  - `alt+up`（KeyCode::Up + ALT）→ 发送 `clear_queue`，把返回的 steering/followUp
    全部回填输入框、队列清空（RPC 无单条取回命令；与 pi 原生
    "restore all queued messages" 一致——实现期据 pi 源码修正，spec 已同步）；
    队列空则忽略。
  - `ctrl+t` → `show_thinking` 取反；`ctrl+o` → `expand_tools` 取反。
  - 阶段 2 键位全部保留；冲突检查：`up`/`enter` 无 ALT 时行为不变。
- pi 键位表对齐：follow_up=alt+enter、dequeue=alt+up、thinking.toggle=ctrl+t、
  tools.expand=ctrl+o（windows 下 pi 用 ctrl+q/alt+q 等，本机 macOS 路径优先，
  Windows 差异记入已知差异，不做平台分支——个人项目只跑 macOS）。

### 6. retry 通知（session）

- `auto_retry_start` → `Notice{kind:"retry", text:"重试 {attempt}/{max}: {errorMessage 摘要}"}`。
- `auto_retry_end` → `Notice{kind:"retry", text:"重试成功" | "重试失败: {finalError}"}`。
- 与 error 通知同路径进 chat_lines，样式：retry 用黄色（区分 error 红）。
- `summarization_retry_*` 仍忽略（compaction 相关，阶段 4 随 compact 流程）。

### 7. 测试策略

- **纯函数单测为主**：markdown（结构/高亮/畸形/未闭合围栏）、工具卡片（三 fixture +
  未知工具）、状态行格式化（cost 边界）、队列带（空/截断）、键位映射（alt 组合）。
- **fixture 离线回放**：既有 3 个 + 新增 tool_bash/tool_edit，断言卡片输出包含
  路径/命令/diff 行。
- **性能冒烟**：10KB markdown 连续渲染 100 帧不超时（缓存命中验证）。
- **真机 PTY**：手工验收清单（见 tasks 末组）——与 pi 并排对比。

## Risks / Trade-offs

- **[syntect 编译/体积]** → 首次构建变慢、二进制变大；接受（个人工具）。若不可接受，
  降级方案：仅结构高亮（保留围栏/语言标签，去掉词法着色），改动局限在 markdown.rs。
- **[每帧重解析 markdown]** → 内容哈希缓存；流式每帧内容必变、但只有当前块重算。
  性能冒烟测试兜底。
- **[edit diff 结构未知（fixture 未抓前）]** → 防御式读取 + args 降级对比；
  抓到 fixture 后按真实结构收敛（tasks 中先抓后渲染的顺序）。
- **[alt 键在部分终端被吞]** → alt+enter/alt+up 若不生效，备选 `ctrl+q`/`ctrl+j`
  （记录于 tui-usage 已知差异）；PTY 验证时确认本机 iTerm/Terminal.app 行为。
- **[队带动态布局抖动]** → 队列高度按内容计算并 clamp，空→非空切换只多一行。
- **[与 pi 的细节差异]** → 验收线是"基本无差别"，逐像素对齐不做；差异记录进
  tui-usage.md 的"已知差异"节。

## Migration Plan

纯增量：新依赖（pulldown-cmark、syntect）+ chat 内部分层 + session 字段扩展。
顺序：session 扩展（含规格补记）→ markdown 管线 → 工具卡片（先抓 fixture）→
状态行 → 队列/键位 → 文档与验收。回滚即 revert，无数据迁移。

## Resolved Questions

- **syntect 体积（已定）**：`cargo build --release` 实测 **2.9MB**（编译 11.5s），
  远低于 40MB 降级线，全语法集保留，不降级。
- 队列带位置（输入框上方 vs 状态行内联）以 PTY 实观感微调，不改规格
  （规格只要求"可见且空时不占显著空间"）。
