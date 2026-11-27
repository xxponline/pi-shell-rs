# Tasks

## 1. 共享行内 diff 纯函数（design D1/D2）

- [x] 1.1 `intra_line_diff(old, new) -> (Vec<Seg0>, Vec<Seg0>)`（`Seg0
      {text, changed}`）：词/空白/标点分词 + 词级 LCS（token 乘积上限，
      超限退整段 changed）；验证：单测——单词替换、多词、插入/删除、
      无变化、超长行退化不卡
      ——已落成：`Seg0{text,changed}` + `tokenize`（词/空白/标点）+ `lcs_words`（同 line_diff 风格 DP、相邻同态合并）；cap=token 乘积 160k 退整段不分段；测试覆盖单词替换/无变化/插入/删除/超长退化
- [x] 1.2 pi 缩进与首段规则：共同前导空白逐位剥离原样拼回；首个
      removed/added 段的前导空白保留不染；验证：单测钉两条规则
      （首行缩进插入不吞缩进、变更从首个非空白词开始）
      ——已落成：`intra_line_diff` 共同前导空白逐位剥离原样拼回 + 首个变更段纯空白改未变更（pi 首段规则）；两规则各有精确断言测试

## 2. 渲染接线（design D3/D4）

- [x] 2.1 `edit_body` details.diff 路径：连续 `-` 块/紧随 `+` 块收集，
      恰 1-1 → `intra_line_diff` 分段（行色红/绿、changed 段
      `Modifier::REVERSED`）；多行块/上下文/`@@` 维持现状；验证：单测
      ——1-1 行对分段与反视频标记、多行块整行、`@@` 仍青
      ——已落成：共享 `DiffRow` 行模型 + `parse_diff_line`（行号可选）+ `render_diff_rows`（连续 -/紧随 + 恰1-1 → `intra_spans` 行内反视频，否则整行；上下文原样、@@ 青）；`edit_body` 改走共享渲染器；测试钉1-1 对反视频/多行整行/@@/上下文（含头行偏移修正）
- [x] 2.2 降级 `line_diff` 同染：LCS 回溯中相邻 `-`/`+` 单对走同一分段；
      验证：单测——old/new 单编辑对产出分段行，多行差异仍整行
      ——已落成：`line_diff` 改产 `DiffRow`（LCS 不变、大文件整块路径同走渲染器），1-1 单对同染；测试钉降级单对反视频 + 多行整行
- [x] 2.3 GPUI 通路确认：`lines_to_segs`→`line→run` 的 REVERSED 分支
      实测生效（前景对调 + background_color）；验证：GPUI e2e 断言
      1-1 对的 Seg 样式携带 REVERSED（渲染帧 run 背景可断则断，
      否则断到 Seg 样式层）
      ——已落成（设计 D4 修正：`lines_to_segs` 实测丢修饰符、TUI/GPUI 经模型桥两端全丢——已同步改 design.md）：新增 `SegRole::InverseFg(Color)` 贯穿 Seg 桥；TUI `seg_style` 加 REVERSED 真反视频；GPUI `segs_row/segs_col` 增 `row_bg` 穿参（行色↔行底对调 + background_color），8 处调用点按 chat/panel 传底；模型层断言 InverseFg 在场、PTY 看像素

## 3. 执行前 preview 管道（design D5/D6）

- [x] 3.1 `App.edit_previews` 状态 + `PreviewState`（Pending/Ready/Error）
      + `UiEvent::EditPreview{tool_id, result}` 回填变体 + 泵应用臂；
      `load_messages` 单点清空；验证：单测——状态机转换、重建清空、
      迟到回填（id 已移除）丢弃
      ——已落成：`EditPreviewState{Pending,Ready(Vec<DiffRow>),Error}` +
      `UiEvent::EditPreview`（存行模型而非文本——避免上下文行以 `-` 开头
      的再解析歧义，design D5 修正）+ `apply_edit_preview` 幂等回填 +
      `after_session_event` 双路径钩子（TUI Rpc 臂与 GPUI apply_core_event
      同源）+ `load_messages` 同点清空 + 两端泵臂；测试覆盖迟到丢弃/结果
      移除/重建清空
- [x] 3.2 触发与计算：edit tool_call 归约后参数齐全且无结果 → Pending →
      独立线程读文件 + 算 diff（行数上限截断）→ 回填 Ready/Error；
      参数判定 = `edits[]` 或 `oldText/newText` 可解析（pi 同款门控）；
      验证：e2e——Running 卡先显预览、文件缺失/oldText 不匹配 → 红底
      错误态、结果到达后替换且 preview 移除
      ——已落成：`after_session_event` 触发（非 edit/参数不齐全不触发，pi 门控同款）→ Pending + 独立线程 `compute_edit_preview`（相对路径拼 cwd、oldText contains 判定、`line_diff_rows` 复用）→ 经 `UiEvent::EditPreview` 回填；测试含真实临时文件的 Ready/文件缺失 Err/oldText 不匹配 Err 三例 + 门控两例（卡片显示面由 3.3 e2e 覆盖）
- [x] 3.3 渲染接线：`tool_card_model`/`edit_body` 消费 preview 态
      （Pending 骨架/Ready 红绿+行内/Error 红底头行+文案）；验证：双表面
      卡片测试——预览/替换/错误三态渲染正确、无重复残留
      ——已落成：`PreviewCtx{map,rev}` 穿布局链（rev 入 LayoutKey → 预览变迁整键重建，免逐条目指纹失配）；`edit_body` 三优先级（结果 > Ready/Error > args 降级）；`tool_card_model_ctx`/`tool_lines_themed_ctx`/`refresh_ctx` 包装器（测试/oracle 零改动）；TUI 红底头行（Error → tool_error_bg）+ GPUI 红底卡（pal.card_bg_err）+ 预览行计入滚动高度估算；测试四枚：优先级/Ready 反视频/Error 红底（chat 面板级）、cache rev 重建、GPUI e2e（事件流建卡 → Error → 文案入卡 + 红底探针）

## 4. 文档

- [x] 4.1 `Docs/tui-usage.md` 工具卡段：补行内反视频高亮（1-1 规则）与
      执行前预览（含错误态、会话重建清空）；验证：文档与实现一致、
      照做可用
      ——已落成：工具卡片段新增「edit 行内变更高亮」（1-1 规则/缩进不染/
      降级同染）与「edit 执行前预览」（先行展示/错误态/结果替换/重建清空）
      两条 bullet

## 5. 集成验收

- [x] 5.1 双矩阵全绿：`cargo test` + `cargo clippy --all-targets`（列 A）
      与 `cargo test --features gpui-support` + clippy（列 B）零告警；
      验证：命令输出全绿、clippy 0
      ——A 434+10+3 / B 530+10+3 全绿，clippy 双列 0（scenario 目标两次
      负载敏感闪断，全量重跑两遍均零 FAILED——已知 flaky 型）
- [x] 5.2 `openspec validate align-edit-diff-rendering --strict` 通过；
      验证：命令输出 valid
      ——`openspec validate align-edit-diff-rendering --strict` valid
- [x] 5.3 PTY 人工冒烟（对照 pi 手感）：真实 edit 调用看执行前预览先行
      展示/结果替换/错误态；单行改一词的行内反视频与 pi 并排对照；
      多行块整行不退化；大文件预览不卡顿；双主题下反视频可读；
      验证：用户实机验收通过
      ——用户验收通过（2026-11-27 实机冒烟，对照 pi 手感）

## Workflow follow-up

- 实现完成后 `/opsx-archive align-edit-diff-rendering`（规格同步：
  tui-app MODIFIED+ADDED、gpui-agent-terminal MODIFIED+ADDED → 主规格）。
