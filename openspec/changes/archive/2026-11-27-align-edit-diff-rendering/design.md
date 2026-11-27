# Design

## Context

- pi 参考实现 `modes/interactive/components/diff.js::renderDiff`：
  解析 `-N 内容`/`+N 内容` 行 → 连续 `-` 块与 `+` 块收集 → **恰 1-1** 时
  `renderIntraLineDiff`：剥共同前导缩进 → `diffWords`（npm `diff` 包）词级
  对比 → 首个 removed/added 段前导空白保留不染 → 变更词 `theme.inverse()`
  （反视频）；多行块整行原色。preview（①）在 `edit.js` 外壳：args 完整即
  `computeEditsDiff(path, edits, cwd)` 异步读文件 → 渲染 preview，结果到
  `details.diff` 替换；错误走 `EditDiffError`。
- 我们现状：`edit_body`（tools.rs:617）对 `details.diff` 只逐行前缀着色；
  降级 `line_diff`（LCS 行级）整行红绿；**渲染管线两端已就位**——TUI
  ratatui `Modifier::REVERSED` 原生反视频、GPUI `line→run` 已实现
  REVERSED→前景对调+`background_color`、`lines_to_segs` 携带 `seg.style`。
- 接入时机：`tool_card_model(name, args_json, tool)` 收到的 args 已完整
  （我们无 args 流式分帧，移出项④），preview 窗口 = tool_call 到达后至
  tool_result 到达前的 Pending/Running 态。
- 管道先例：`spawn_async` + `UiEvent` 回填、`load_messages` 单点重建、
  `apply_ui_event` 泵——preview 异步回填可复用同构通路。

## Goals / Non-Goals

**Goals:**

- pi 同款 1-1 行内反视频（词级、缩进规则对齐），双表面呈现。
- ① 执行前异步 preview + 结果替换 + 错误态 + 重建清空。
- 降级 `line_diff` 同染；共享纯函数、无 TTY 可测。

**Non-Goals:**

- 不引入 npm `diff` 等价 crate（自实现词级 LCS，同 `line_diff` 风格）。
- 不做 args 流式分帧渲染（移出项④，纯性能）。
- 不改 `@@` 青色、不引入框架 diff 查看组件（既有 spec 钉定）。
- 不做 preview 的跨工具通用化（仅 edit）。

## Decisions

**D1 词级 LCS 自实现（无新依赖）。**
`intra_line_diff(old, new) -> (Vec<Seg0>, Vec<Seg0>)`，`Seg0 { text, changed }`。
分词规则：连续字母数字为词、连续空白为一段、标点各为一段（对齐
`diffWords` 的可读粒度，不追求逐字符同）。LCS DP 同 `line_diff` 风格，
token 乘积 > 上限（沿用 160k 量级）时放弃分段退回整行（防大行卡顿）。
备选「引 crate」被否——单函数体量不值依赖成本（AGENTS：弱依赖边界）。

**D2 pi 缩进与首段规则逐条对齐。**
① 先剥 old/new 共同前导空白（仅空白字符逐位相同才计入），对剩余部分做
词级对比，缩进原样拼回两行；② word-diff 首个 removed 段与首个 added 段
各自剥掉前导空白并保留为不染文本（防「插入导致整段前移」把缩进也算成
变更）。两规则各有单测钉住。

**D3 1-1 配对在渲染侧做。**
`edit_body` 的 details.diff 路径改为：按行扫描收集连续 `-` 块与紧随的
`+` 块 → 恰各 1 行 → `intra_line_diff` 分段着色（行色红/绿，changed 段
加 `Modifier::REVERSED`）；否则整行（现状）。`line_diff` 降级路径在 LCS
回溯产出 `-`/`+` 相邻对时同样分段（其输入本就是单编辑 old/new，几乎必中
1-1）。上下文/`@@` 不动。备选「把两路径都先转成 pi 格式 diff 文本再统一
渲染」被否——重构面大、收益仅去重。

**D4 反视频落地：`SegRole::InverseFg` 贯穿模型桥（实现期修正）。**
原判断「`lines_to_segs` 携带 style、零渲染改造」有误——实现发现
`Seg` 只有 `{text, role}`、`lines_to_segs` 只映射 fg，且 **TUI/GPUI 都经
`tool_card_model` 的 Seg 桥**，修饰符两端全丢。修正方案：新增
`SegRole::InverseFg(Color)`（fg=行色）——TUI `seg_style` 加
`Modifier::REVERSED`（终端 SGR7 真反视频，与 pi 同）；GPUI `segs_row/
segs_col` 增 `row_bg` 穿参，行色↔行底对调写入 `TextRun.background_color`
（调用点：工具卡传面板色、通知/bash/thinking 传消息区底色）。

**D5 preview 异步管道（app 层状态 + 复用回填通路）。**
- 状态：`App.edit_previews: HashMap<String /*toolId*/, PreviewState>`，
  `PreviewState = Pending | Ready{diff_lines 或 pi 格式文本} | Error{msg}`。
  additive 字段，随 `load_messages`（transcript 唯一整体重写点）清空
  （与历史重建同点，spec「会话重建清空」）。
- 触发：tool_call(edit) 归约落条目后（app 层事件路径），若参数齐全且
  无结果 → 置 `Pending` → `std::thread` 读文件 + 算 diff（**不在渲染线程
  做 IO**，spec「不阻塞事件循环」）→ 经 `report_tx` 回 `UiEvent`（新增
  变体或复用 AsyncData 形态——design 定为新变体 `EditPreview{tool_id,
  result}`，语义清晰）→ 泵应用 → `Ready/Error` → notify 重绘。
- 参数齐全判定：`edits` 数组或 `oldText`/`newText` 字符串均可解析即齐
  （pi `getRenderablePreviewInput` 同款）；编辑工具以名字 `edit` 门控。
- 结果到达：tool_result 归约时移除该 toolId 的 preview（结果 diff 由
  `edit_body` 正常渲染，天然替换）；错误结果同移除。
- 竞态防护：回填按 toolId 幂等覆盖，迟到回填遇到已移除的 id 直接丢弃。

**D6 错误态 = pi `EditDiffError` 同款语义。**
文件读失败 / oldText 不匹配 → `Error{msg}`；渲染头行走既有失败态样式
（红底）+ 错误文案行。oldText 匹配判定：字面包含（`contains`）——与
pi computeEditsDiff 的语义对齐（多处匹配取其一的歧义不展开，错误才报）。

**D7 非目标边界（写进 proposal 已含）**：args 流式分帧、`@@` 变色、
diff 查看组件均不动。

## Risks / Trade-offs

- [词级分词与 pi 视觉不完全一致（`diffWords` 细节）] → PTY 与 pi 并排
  对照典型样例；核心规则（缩进/首段）单测钉死，分词粒度允许微差记已知
  差异。
- [preview 竞态（结果先于回填到达 / 会话切换后迟到回填）] → 回填按
  toolId 幂等 + 移除后丢弃（D5）；测试注入乱序回填钉住。
- [大文件/超长行读取与分段卡顿] → 读取截断（preview 上限行数，超限
  退整行/截断标注）+ D1 token cap；PTY 验收大文件。
- [渲染线程误做 IO] → IO 只在 spawn 的线程；渲染只读状态（review 点）。
- [GPUI 反视频在浅色主题下的可读性] → 反视频=前景对调，主题无关；PTY
  双主题各看一眼。

## Migration Plan

纯增量：`edit_previews` 空 map 时行为与现状逐字节一致；行内分段只在
1-1 对触发（多行/无变化路径不变）。回滚 = revert。规格侧两能力各
MODIFIED+ADDED 一次归档同步。

## Open Questions

（无——范围①+②+降级同染已在 explore 定案；分词粒度与 preview 上限值
属实现期参数，由 Risks 的 PTY 验收把关。）
