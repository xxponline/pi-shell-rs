# Design

## Context

动机见 proposal.md（Why）。与实现现状相关的约束：

- 渲染是**无状态全量重建**：`view::render` → `chat::chat_lines(state, flags)` →
  `tools::tool_lines(...)` 产出 `Vec<Line>`，每次事件后从 `SessionState` 重算。
  没有 pi 那样的驻留组件（`ToolExecutionComponent` + `rendererState`），计时器/
  展开态等"渲染侧状态"必须有落点。
- `event_loop`（`mod.rs`）每事件 `draw` 一次，`recv_timeout(200ms)` 的 Timeout
  分支**故意不重绘**（`// Idle timeout: no state changed, so no repaint.`）——
  运行中计时刷新需要在这里开洞。embed 模式复用同一 `event_loop`。
- `transcript.rs::on_tool_update` 显式丢弃 `partialResult`（"we only need to mark
  running existence"）；`ToolExecution` 只有 `status/is_error/result/duration_ms`。
- 节流已在 **pi 源端**：`ref/pi/.../core/tools/bash.ts:331` `scheduleOutputUpdate`
  按 `BASH_UPDATE_THROTTLE_MS=100` 合并后才 `onUpdate`，故我们收到的
  `tool_execution_update` 天然 ≤10Hz，客户端无需重复限流。
- 截断数据已在状态里：`on_tool_end` 的 `result: event.get("result").cloned()`
  全量透传，`apply_tool_result_message` 透传 `content/structuredContent/details`。
- 折叠是 `tool_lines` 末尾统一 `take(COLLAPSED_BODY_LINES=3)`，方向（头/尾）与
  行数无 per-tool 差异，也无隐藏行数提示。
- `wrap_for_viewport`（`wrap.rs`）自行贪心换行并按显示列截取，是纯函数且知道
  视口宽度——背景面板的行尾补齐只有它能做。

## Goals / Non-Goals

**Goals:**

- 运行中可见：partialResult 进状态、卡片实时显示、Elapsed 每秒走字。
- 折叠/截断信息完整：取向正确（bash 取尾）、行数提示、警告带数字与路径。
- 头行与 pi 逐项对齐（prompt 样式、行范围、limit、compact 分类）。
- 背景面板整行着色，且不破坏 `wrap_for_viewport` 的宽度/换行语义。

**Non-Goals:**

- 移出项（已挂账 `Docs/roadmap.md`）：edit 执行中预计算 diff、语法高亮、
  结果图片块、卡片点击展开、args 流式分帧渲染。
- 保持现状的规格背离（已记录 `Docs/pi-tui-interaction.md` #16/#17）：状态
  文字不被背景替代、read 折叠保留内容预览——本 change 不改这两条规格行为。
- 滚动回看（独立挂账，见 roadmap）、主题化（pi 的 theme token 体系不引入，
  用固定近似色）。

## Decisions

### D1：`partialResult` 存为 `ToolExecution.partial` 独立字段，不复用 `result`

`ToolExecution` 增加 `partial: Option<serde_json::Value>`。渲染取值
`result.as_ref().or(partial.as_ref())`；`on_tool_end`/`apply_tool_result_message`
的 exec 带 `partial: None`。

merge 规则（`merge_tool`）：

```
partial = if new.status == Ended || base.status == Ended { None }
          else { new.partial.or(base.partial) }
```

即：**任一侧已 Ended 则 partial 清空**（防 end 后晚到的 update 污染最终态）；
否则新 partial 优先、缺省继承。

备选（否决）：复用 `result` 字段存 partial——`merge_result` 会把 partial 与
final 按键合并，partial 独有的键残留到最终结果里，且 `result.is_some()` 从
"有最终结果"退化为"有任何结果"，所有既有判空点语义漂移。

### D2：计时起点 `started_at: Option<Instant>` 落在 `ToolExecution`，渲染用 `elapsed()`

`on_tool_start`（及 update 首见）打点 `Instant::now()`；merge 取
`new.started_at.or(base.started_at)`。渲染规则对齐 pi `toolTimerText`：

- `status==Ended` 且有 `duration_ms` → `Took <fmt(duration_ms)>`（服务端值优先，
  单调且跨重载存活）；
- 否则有 `started_at` → `Elapsed <fmt(started_at.elapsed())>`（Ended 但无
  duration_ms 时也走此支）；
- 两者皆无 → 不显示计时行。

时长格式 `format_tool_duration(ms)`：`4.2s`（<60s）/ `1m 30s`（<1h）/
`2h 3m 4s`（≥1h），替代现有 head 里的裸 `· {ms}ms`。

备选（否决）：App 层 `HashMap<tool_id, Instant>`——渲染签名要携带 map，状态
与数据割裂；`get_messages` 重建后 Running 工具拿不到起点，仍需降级路径，复杂度
更高收益为零。放状态机的代价是部分测试断言需从整体相等改为字段断言（见 Risks）。

### D3：计时刷新靠 `event_loop` Timeout 分支条件重绘，不引独立 timer 线程

Timeout（200ms 粒度）时：若 `SessionState` 存在 Running 工具且距上次 draw
≥1s → `draw`。抽 `SessionState::has_running_tool()`（扫 in_flight blocks）。
embed 复用同一 event_loop，自动获得该行为；多出的帧对 `take_ansi` 消费方无害。

备选（否决）：独立 1s tick 线程 + channel——多一条线程与一种事件，而 200ms
poll 循环已存在，条件重绘是它的自然延伸。

### D4：draw 合并（而非客户端限流）消化高频 update

`event_loop` 的 Rpc 分支处理完一个事件后 `try_recv` drain 积压事件、逐个
处理，**最后只 draw 一次**。配合 pi 源端 100ms 节流，实际重绘上界 ≈10Hz；
突发（如 compaction 批量事件）被合并为一帧。

这是对 proposal"更新节流"一项的实现修正：限流主体在 pi 源端（bash.ts:331），
客户端的措施是合并而非丢弃/延迟——**不丢事件**，只合并重绘。

备选（否决）：客户端 100ms 内延迟 draw——引入"事件已处理但画面滞后"的中间
态，且与 pi 源端节流叠加后延迟翻倍，无必要。

### D5：背景面板 = 卡片行 `Line.style(bg)` + `wrap_for_viewport` 行尾补齐

- `chat.rs` 给工具卡片行设 `Line::style = Style::default().bg(color)`：
  运行灰 / 成功墨绿 / 失败暗红（由 pi dark.json 的 okhsl(229 5% 24%) /
  okhsl(158 46% 25%) / okhsl(19 54% 25%) 转 RGB 近似，任务里落具体值）。
  状态圆点与文字保留（spec #16 要求），叠在面板上。
- `wrap_one` 把父行的 `style` 传播到拆分出的子行；`wrap_for_viewport` 对
  `style.bg.is_some()` 的行在末尾补 `" ".repeat(remaining)` 的同 bg span，
  使面板**整行着色**到视口宽。
- `Line.style` 是 ratatui 0.30 公有字段，拆行不自动传播——必须在 wrap 手动
  传递，这是本决策的核心胶水点。

备选（否决）：只给 spans 上 bg（行尾不齐，视觉明显逊于 pi 的整行面板）；
view 层后处理补 pad（要给 `chat_lines` 加"卡片行范围"出参，侵入所有调用方）。

### D6：折叠策略表上移到 `tool_lines`，per-tool 取向 + 提示行

`tool_lines` 不再统一 `take(3)`，改为查策略表折叠后再追加提示行：

```
bash            → 尾部 5 行 + `... (N earlier lines, ctrl+o to expand)`
grep            → 头部 15 行
find / ls       → 头部 20 行
write / 其余     → 头部 10 行 + `... (N more lines, ctrl+o to expand)`
（提示行仅在实际发生截断时追加；展开态不截断、无提示）
```

edit 的 diff 折叠同"其余"（头部 10 行）。各 `*_body` 保持返回全量行，折叠
是 `tool_lines` 的后处理——与现状结构一致，改动面最小。

备选（否决）：把折叠下推到各 body（pi 的 renderResult(options.expanded)
模式）——更贴 pi，但要改 8 个 body 签名，收益只是省一次全量分配。

### D7：截断警告与头行扩写 `head_arg`/body，数据源全部已在状态里

- bash body：读 `details.truncation` / `details.fullOutputPath`（或
  `structuredContent.full_output_path`，以 fixture 实测字段路径为准）产出
  `[Full output: <path>]` 与 `[Truncated: showing X of Y lines]`。
- grep/find/ls body：各自 `details.matchLimitReached/resultLimitReached/
  entryLimitReached/linesTruncated/truncation` → `[Truncated: ...]`。
- read body：`details.truncation` 三形态（firstLineExceedsLimit / lines / bytes）。
- `head_arg` 扩展：bash 返回 `$ <command>` + ` (timeout Ns)`；read 返回
  `path` + `:offset-limit` 行范围 + compact 分类（SKILL.md→`[skill] <父目录>`、
  AGENTS/CLAUDE 类→`resource`、`docs/`·`README.md`→`docs`）；grep 补
  glob/limit；find/ls 补 limit。
- **compact 分类不解析 cwd**：pi 用 `resolveToCwd` + 包根相对路径，我们按
  raw path 字符串判断（`SKILL.md` 结尾、`AGENTS.md` 等文件名集合、`docs/` 或
  `README.md` 起始）——label 直接用 raw path。绝大多数调用传的是相对路径，
  与 pi 视觉等价；绝对路径下 label 为绝对路径（可接受简化）。
- 未知工具头行：`name key="value" ...`（100 字符截断），展开态逐参数分行
  （复用 generic_body 的 args 段上移）。

### D8：`ToolExecution` 字段变更的测试适配策略

`started_at: Option<Instant>` 与 `partial: Option<Value>` 进入
`#[derive(PartialEq)]` 结构。既有整体相等断言（如 merge/replay 测试）在
字段为 None 时不受影响；仅当测试构造 `Some` 值时需改字段断言。`Instant`
不进任何序列化路径（session 文件由 pi 管），replay 打点为回放时刻——回放中
Running 的工具 Elapsed 从回放起算，end 后立即被 `duration_ms` 取代，行为
可接受且与 pi"组件时钟为兜底"一致。

## Risks / Trade-offs

- [`merge_tool` 的 partial 清空规则被后续事件破坏] → 规则写成"任一侧 Ended
  即 None"并配三个 merge 单测（update→update、update→end、end→晚到 update）。
- [`Line.style` 传播遗漏导致面板断行] → `wrap_one` 拆行/硬切两个路径都要传
  style；wrap 现有测试补"带 bg 行拆行后子行仍带 bg"断言。
- [背景色与既有内容色撞车]（如绿字 diff 在墨绿面板上）→ 面板用暗色（l≈24-25%），
  前景色对比度足够；PTY 手验时重点看 diff/bash 输出行。
- [Timeout 条件重绘在无 Running 工具时不生效，Elapsed 停走] → 单测覆盖
  "有 Running 才重绘"；PTY 手验跑一条 sleep 10 看秒数走动。
- [draw 合并改变事件可见顺序] → drain 保持 FIFO，仅合并 draw，行为等价；
  embed 的 inject_key→帧 回归测试守住该通路。
- [大输出 partial 常驻内存] → partial 在 end 时即被清空，峰值 = 最大单次
  partial 快照，与 pi 组件持有的 result 同量级。

## Migration Plan

纯客户端 change，无部署/回滚序列。落地顺序（每步可独立验证）：

1. 状态层：`ToolExecution` 加字段 + `on_tool_update` 接 partial + merge 规则
   （`cargo test` 既有 transcript 测试全绿为门）。
2. 渲染层：`format_tool_duration` + 计时行 + 折叠策略表 + 截断警告 + 头行。
3. 面板：`Line.style` + wrap 传播/补齐（wrap 单测先行）。
4. 事件循环：Timeout 条件重绘 + Rpc drain 合并。
5. 文档同步：`tui-usage.md` 工具卡片段、`pi-tui-interaction.md` 差异清单
   补对齐行、proposal 中"客户端节流"表述以本 design D4 为准。

## Open Questions

无。截断警告的字段路径（`details` vs `structuredContent`）以 tool_bash 等
fixture 实测为准，属任务内验证项，不改变方案。
