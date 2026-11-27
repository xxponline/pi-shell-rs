# Tasks

## 1. 共享历史核心（`EditorBuffer`，design D1/D3/D5）

- [x] 1.1 `EditorBuffer` 增历史状态三件套（`history` 最新在前 /
      `history_index: Option<usize>` None=草稿态 / `history_draft` 快照）
      与 `push_history(text)`（trim、空跳过、相邻同文去重、上限 100 头插
      淘汰——pi 同源）；验证：单测覆盖入栈顺序/去重/上限/空输入
      ——已落成：`EditorBuffer` 增 `history/history_index/history_draft` 三字段 + `push_history`（trim/空跳/相邻去重/头插/上限100）；单测覆盖顺序/去重/上限淘汰
- [x] 1.2 `navigate_history(±1)` 纯逻辑：首次进入捕获草稿 + 推 undo 快照、
      越界 no-op、回草稿还原 lines/row/col、取旧条目光标落行首/新条目落
      行尾；验证：状态机矩阵单测（-1↔0↔…↔n-1、空历史、单条历史、
      双向翻到界）
      ——已落成：`navigate_history(±1)` 状态机（首进捕获草稿+push_undo、`-1`=草稿态、越界 no-op、Up 落行首/Down 落行尾、翻过最新 `HistoryStep::Draft` 还原）；修一处边界 bug（`next as usize` 在 -1 回绕，改 isize 比较）；矩阵+多行两测绿
- [x] 1.3 `rebuild_history(user_msgs)`（**先清后灌**，幂等）与
      `exit_history_browsing()`（index/draft 双清、文本留驻）；验证：单测
      ——同一消息序列重建两次结果逐字节相同（pi 重复入栈缺陷的对照钉子）、
      重建覆盖 push 结果、退出后 navigate 视为重新进入
      ——已落成：`rebuild_history`（先清后灌、覆盖 push、复位浏览态）+ `exit_history_browsing`（草稿丢弃文本留驻）；幂等对照钉子测试（同序列重建两次逐字节相同）绿

## 2. TUI 分派接线（design D2/D4）

- [x] 2.1 编辑区无修饰 Up/Down 历史臂：首行(空|浏览中|col==0)→navigate、
      首行其余→`home()`（现 `move_up` 首行 no-op 的适配）、`row>0`→原生；
      末行浏览→navigate、末行其余→`end()`（`move_down` no-op 适配）、
      非末行→原生；补全菜单开时仍归菜单；验证：e2e（空输入直入历史、
      首按回行首次按进历史、到界 no-op、菜单优先），并按新语义核对/更新
      既有首末行 no-op 键位测试
      ——已落成：`edit_key` Up/Down → `history_or_move_up/down`（首行三条件进历史/其余回行首、末行浏览进历史/其余行尾、非首末行原生——替代两处 no-op 适配 pi）；e2e `history_arrow_dispatch_follows_pi`（空直入/首按行首/次按进/到界 no-op/Down 回空草稿/非首末行原生/末行行尾）绿；无既有 no-op 键位测试需改
- [x] 2.2 内容变更退出浏览：`key_intent` 为 Edit 类的键先
      `exit_history_browsing()` 再执行（草稿丢弃、文本留驻），Move 类不退；
      验证：测试——浏览中插字符后 Up/Down 不再跳变、光标移动不退出
      ——已落成：`key_mutates_content` 判定（字符插入/kill/yank/undo/退格删除=true，纯光标与历史步进=false）后置于 `edit_key` 统一退出；Newline/ExitIfEmpty 意图臂显式退出；Paste 改返回 bool（空剪贴板不误退）；测试 `content_change_exits_browsing_but_cursor_does_not` 绿
- [x] 2.3 提交入栈：发送路径调 `push_history`（含排队/steer 同文本）；
      验证：测试——提交后历史头部为该文本、相邻重复提交只留一条
      ——已落成：`submit_as` 进入即退出浏览（斜杠/bash/常规一并复位）、bash 分流后统一 `push_history`（斜杠与 `!bash` 不入栈——它们非 transcript user 消息，与重建源一致）；测试含提交入栈/清空/退浏览、斜杠不入栈

## 3. 重建接线点（design D3）

- [x] 3.1 全量重建点接线：启动首载、切会话/new、树导航/fork、reload →
      `rebuild_history(当前 transcript user 消息)`；验证：测试——树导航
      重建后按 Up 恰为当前分支 user 消息各一条、顺序正确、与重建次数
      无关（幂等）
      ——已落成：单点挂钩 `AsyncKind::Messages`（`load_messages` = transcript 唯一整体重写点）→ `rebuild_input_history()`（取 `Entry::User` 时间序先清后灌）；覆盖启动/切会话/new/fork/树导航/import；/reload 不重渲 transcript 故无需挂钩（设计注记）；测试 `messages_response_rebuilds_history_idempotently` 绿
- [x] 3.2 流式 append 不触碰 history（只 push 于提交）；验证：测试——
      流式中收到 assistant/工具事件后历史不变，提交后才增
      ——已落成：push 仅在 `submit_as`、重建仅在 Messages 整体重写；测试断言流式 user/assistant core 事件后 `history_len` 不变、提交后才增

## 4. GPUI 路由接线（design D2/D4/D6）

- [x] 4.1 `route_keystroke` Up/Down 历史臂：分派条件（逻辑行首/末 +
      空/浏览/col==0）成立 → 共享 navigate + 回写 Textarea（复用
      `push_editor_via_active_window` 桥）；条件不成立 → 放行内建原生
      移动；菜单/浮层优先级不变；验证：e2e——空输入翻历史并可翻回空、
      非条件时原生移动且历史不动、菜单开 Up=菜单导航、浮层开不翻历史
      ——已落成：`history_route` 前置覆盖（修饰键/浮层/菜单不入；行位读 Textarea 实时光标与行数——原生移动不回写镜像、镜像行位会滞后）→ ToApp 走共享 `history_or_*`（ToApp 分支自带 sync+push 桥）；修 `push_editor_to_textarea` 早返回吞光标回写的潜在 bug（文本同只跳 set_value、光标始终对齐）；实现期修正 GPUI delta 一处与 TUI 的矛盾（末行非浏览 Down 补「到行尾」+ 场景改多行首末行表述）；e2e `history_keys_route_to_shared_and_back` 四相位绿
- [x] 4.2 GPUI 内容变更退出：`InputEvent` 文本变化 → 共享
      `exit_history_browsing()`（带一次性回写 guard，程序化回写不算用户
      变更）；验证：e2e——浏览中键入字符后不再跳变、回写触发的 change
      不误退出
      ——已落成：`InputEvent::Change` 订阅内以「进 Change 前的浏览条目」为基准——同步后文本偏离 → 用户改内容 → 退出；程序化回写推的就是条目原文（相等）→ 不退出，免去 design D4 预想的一次性 guard（设计修正）；e2e `typing_exits_browsing_but_programmatic_push_does_not` 绿（esc 回写不断、键入即退）

## 5. 文档

- [x] 5.1 `Docs/tui-usage.md` 键位表补 Up/Down 历史行（首按回行首、
      草稿还原、重建语义），GPUI 节注明与 pi 的两处已知差异（逻辑行近似、
      纯点击不退出浏览）；验证：文档与实现一致、照做可用
      ——已落成：键位表新增 ↑/↓ 历史行（进入条件/草稿还原/退出/重建语义）；
      已知差异段删除「无输入历史」旧文；GPUI 节新增历史 bullet（共享分派+
      实时光标行位）并记两处已知差异（逻辑行近似 pi 视觉行、纯点击不退出
      浏览）

## 6. 集成验收

- [x] 6.1 双矩阵全绿：`cargo test` + `cargo clippy --all-targets`（列 A）
      与 `cargo test --features gpui-support` + clippy（列 B）零告警；
      验证：命令输出全绿、clippy 0
      ——A 426+10+3 / B 521+10+3 全绿，clippy 双列 0（scenario 目标曾
      负载敏感闪断一次，重跑 10/10 过——已知 flaky 型）
- [x] 6.2 `openspec validate add-input-history --strict` 通过；验证：命令
      输出 valid
      ——`openspec validate add-input-history --strict` valid
- [x] 6.3 PTY 人工冒烟（对照 pi 手感）：TUI/GPUI 各验空输入直入、首按
      回行首、草稿还原、浏览中打字退出、菜单优先；树导航后历史无重复；
      GPUI 长条目逻辑行边界可接受；验证：用户实机验收通过
      ——用户验收通过（2026-11-27 实机冒烟，TUI+GPUI）

## Workflow follow-up

- 实现完成后 `/opsx-archive add-input-history`（规格同步：tui-app
  MODIFIED+ADDED、gpui-agent-terminal MODIFIED+ADDED → 主规格）。
