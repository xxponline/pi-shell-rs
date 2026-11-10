# Tasks

## 1. 状态层：partialResult 与计时起点（design D1/D2）

- [x] 1.1 `ToolExecution` 增加 `partial: Option<serde_json::Value>` 与
  `started_at: Option<Instant>` 字段；`merge_tool` 实现规则"任一侧 Ended 则
  partial 清空，否则新 partial 优先继承；started_at 取最早"。
  验证：新增 3 个 merge 单测（update→update 继承、update→end 清空、
  end→晚到 update 不污染）全绿，`cargo test` 既有 transcript 测试不改断言通过。
- [x] 1.2 `on_tool_update` 接住 `partialResult` 存入 partial（无该字段的
  update 不清空已有 partial），`on_tool_start` 与 update 首见路径打点
  `started_at`。验证：新增 transcript 单测——运行中记录部分结果、完整结果
  替换部分结果且结束后 partial 为 None（对应 spec session-state 两个新场景）。

## 2. 渲染对齐：计时、折叠、截断警告、头行（design D2/D6/D7）

- [x] 2.1 新增 `format_tool_duration(ms)`（`4.2s`/`1m 30s`/`2h 3m 4s`）并接入
  卡片计时行：Ended 且有 duration_ms 显示 `Took …`，否则有 started_at 显示
  `Elapsed …`（每帧重算），两者皆无不显示；替换 head 里的裸 `· {ms}ms`。
  验证：`format_tool_duration` 三档边界单测 + 卡片计时行三态单测全绿。
- [x] 2.2 `tool_lines` 折叠改策略表（D6）：bash 尾部 5 行、grep 15、find/ls 20、
  write/其余头部 10 行；实际截断时追加 `... (N earlier/more lines, ctrl+o to
  expand)` 提示行，展开态不截断。
  验证：改写 `collapsed_body_is_limited`，新增 bash 折叠保留**尾部**（含 exit
  code 行）与提示行断言，`cargo test tools` 全绿。
- [x] 2.3 截断警告（D7 后半）：bash `[Full output: <path>]` + 行数/字节详情，
  grep/find/ls 各自 limit 警告，read 三形态；以 fixture 实测字段路径
  （details vs structuredContent）为准。
  验证：按既有 fixture 模式新增各工具警告单测，断言含具体数字/路径而非仅省略号。
- [x] 2.4 头行扩展（D7 前半）：bash `$ <cmd>` + ` (timeout Ns)`；read 行范围
  `:1-20` + compact 分类（SKILL.md→`[skill]`、AGENTS/CLAUDE→resource、
  docs/README→docs，raw path 判断不解析 cwd）；grep 补 glob/limit、find/ls
  补 limit；write 预览行数与总行数；未知工具头行 `name key="value"`（100 字符
  截断、展开态分行）。
  验证：各头行单测全绿；既有 read/bash/grep 卡片测试按新格式改写断言。
- [x] 2.5 文档同步：`Docs/tui-usage.md` 工具卡片段（运行中流式输出/计时格式/
  折叠取向与提示/截断警告/头行样式）；`Docs/pi-tui-interaction.md` 差异清单
  补"工具卡片渲染对齐"已完成行。
  验证：grep tui-usage 无与新行为矛盾的旧表述（如仅 `· Nms`）。

## 3. 背景面板（design D5）

- [x] 3.1 `wrap_one` 两个拆行路径传播父行 `Line.style`，`wrap_for_viewport`
  对带 bg 的行按显示列补行尾同色 span 到视口宽。
  验证：wrap 新增单测——带 bg 行硬切/贪心拆行后子行仍带 bg、行补齐后总宽
  等于视口宽，`cargo test wrap` 全绿。
- [x] 3.2 `chat.rs` 工具卡片行按状态设 `Line.style` bg（运行灰/成功墨绿/
  失败暗红，色值由 pi dark.json okhsl 转 RGB 近似），状态圆点与文字保留。
  验证：view/chat 渲染单测——三态面板整行着色、状态文字仍可见、非卡片行
  无背景，`cargo test` 全绿。

## 4. 事件循环（design D3/D4）

- [x] 4.1 `SessionState::has_running_tool()` + Timeout 分支条件重绘：有运行中
  工具且距上次 draw ≥1s 才 draw（纯判断抽为可测函数）。
  验证：`has_running_tool` 与重绘判定单测全绿（in_flight 与已落盘 transcript
  两种 Running 来源都覆盖）。
- [x] 4.2 Rpc 分支 drain 合并：处理完当前事件后 `try_recv` drain 积压逐个
  处理、最后只 draw 一次（FIFO 不丢事件）。
  验证：既有 embed 通路回归测试（`inject_key_produces_frame_with_new_text` 等）
  全绿，`cargo test embed` 通过。

## 5. 集成验证

- [x] 5.1 `cargo test` 全绿 + `cargo clippy --all-targets` 零告警。
  验证：两命令输出无 error/warning。
- [x] 5.2 `openspec validate align-tool-card-rendering --strict` 通过。
  验证：命令退出码 0。
- [x] 5.3 PTY 手验（参考 `Docs/tui-usage.md`）：跑一条长 bash 看运行中输出
  实时增长 + Elapsed 秒数走动 → 结束后 `Took 4.2s` 格式 → bash 折叠显示尾部
  5 行 + earlier-lines 提示 → 截断输出见带数字警告 → 三态背景面板与状态文字
  并存 → 头行 `$ cmd (timeout …)` 样式。验证：六步行为与 spec 场景一致。

## Workflow follow-up

- 实现完成并验收后运行 `/opsx-archive` 归档（同步主规格
  `tui-app`/`session-state` 修订）。
