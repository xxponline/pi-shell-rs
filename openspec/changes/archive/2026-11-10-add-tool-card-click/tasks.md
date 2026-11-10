# Tasks

## 1. 单卡展开态（语义层）

- [x] 1.1 `App` 增 `user_tool_expanded: HashMap<String, bool>` 覆盖表；
      生效值 `override.get(id).unwrap_or(expand_tools)` 接入
      `tool_lines` 入参链——`chat_lines`/`append_blocks` 增
      `overrides: &HashMap` 独立参数（`RenderFlags` 不动，保住 Copy），
      `tool_lines(..., expanded)` 改取生效值；验证：单测①无覆盖跟随全局
      ②覆盖优先于全局③波及面（view/chat 测试构造）编译修复后
      `cargo test chat::` 全绿
- [x] 1.2 Ctrl+O 广播语义：`apply_intent` 的 `ToggleTools` 分支翻全局
      `expand_tools` **并清空** `user_tool_expanded`（≡ pi
      `setToolsExpanded` 遍历 `setExpanded(新值)`，design D1）；
      验证：单测"单卡切换后按 Ctrl+O → 覆盖表清空、所有卡跟随新全局"
      （规格第 4 场景），`cargo test` 对应用例全绿

## 2. 命中表管道

- [x] 2.1 `chat_lines` 返回 `(Vec<Line>, Vec<Option<String>>)`：仅
      `Block::ToolCall` 且 `tool.result.is_some()` 的行记 `Some(id)`
      （门控前移到建表，运行中卡不入表），其余路径（用户行/markdown/
      思考/notice/`!` bash）全 `None`；in-flight 同路径；验证：单测
      （结果卡入表、运行中卡不入表、`!` bash 不入表、meta 与行数等长），
      `cargo test chat` 全绿
- [x] 2.2 `wrap_for_viewport` 接 meta：logical→screen 传播（一条逻辑行的
      全部子行同 meta），drain 窗口**之前**全量扫出
      `Vec<CardHit{start, end, id}>`（绝对屏行区间）；验证：单测——
      展开卡跨多行范围连续、following/pinned 两态下 hits 均为绝对号
      （不含窗口偏移）、meta 与窗口化输出对齐，`cargo test wrap` 全绿
- [x] 2.3 `view::render` 返回值扩为 `(ChatScroll, Vec<CardHit>)`，
      `draw` 沿 Cell 模式回写 `self.card_hits`；`ViewUi` 增
      `tool_expanded: Option<&'a HashMap<String, bool>>`
      （None=无覆盖，保住 derive(Default)）；验证：`cargo build` 通过、
      view 既有 `..Default::default()` 测试不改即编译、新单测断言
      render 返回的 hits 含已知卡片 id 与其行区间

## 3. 事件转发

- [x] 3.1 `event.rs`：`MouseEventKind::Down(Left)` →
      `UiEvent::Click { x, y }`（0-based 坐标）；`Up`/`Moved`/`Drag`/
      右键/横向滚轮仍 `None`；**改写前置 change 的边界测试**（原
      "Down/Up 仍丢" → "仅左键 Down 转发、其余仍丢"，测试名注明
      add-tool-card-click 边界，不静默改变前置断言语义）；验证：
      `cargo test event` 全绿（含新转发与仍丢两类断言）

## 4. 分发

- [x] 4.1 `mod.rs` `UiEvent::Click` 臂：浮层打开 → 忽略；
      `card_at(y)`：`row = y-1`、`row >= scroll.viewport` → 无动作、
      `abs = scroll.top + row` 线性扫 hits、命中 → 覆盖表写入
      `!当前生效值` → `need_draw=true` + `log_event`；验证：单测
      ①点折叠卡展开且他卡不变 ②点运行中卡（不入表）无效
      ③浮层打开时点击被忽略 ④行号越界安全无 panic ⑤已展开卡再点恢复折叠，
      `cargo test` 对应用例全绿

## 5. 文档与集成验证

- [x] 5.1 `Docs/tui-usage.md`：操作说明补"左键点击工具卡片切换该卡展开/
      收起；Ctrl+O 全局切换并重置单卡状态"；验证：文档含该条目
- [x] 5.2 `Docs/roadmap.md`：工具卡片移出项 ④（卡片点击展开）删除，
      滚动条挂账不动；验证：roadmap 不再列 ④、滚动条仍在挂账
- [x] 5.3 PTY 手工验证（SGR 点击注入 `\x1b[<0;x;yM` + pyte 断言，
      前置捕获已开）：①点击折叠卡展开、他卡不变 ②再点恢复折叠
      ③运行中卡点击无效 ④单卡切换后 Ctrl+O 全部跟随全局；验证：四项
      断言全过
- [x] 5.4 全量门禁：`cargo test` 全绿 + `cargo clippy --all-targets`
      零告警 + `openspec validate add-tool-card-click --strict` 通过；
      验证：三命令输出即证据

## Workflow follow-up

- **前置**：`add-mouse-wheel-scroll` 必须先 apply 并归档（捕获与事件层
  地基；组 3 的 event.rs 改写以其落地形态为基础）；本 change 不含
  terminal.rs 内容。
- 实现完成后走 `/opsx-archive` 归档，随归档同步主规格
  （`openspec/specs/tui-app` 新增「工具卡片点击展开」需求）。
