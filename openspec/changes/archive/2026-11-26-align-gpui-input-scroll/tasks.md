# Tasks

## 1. 滚动粒度 spike（S1/S2 定案，前置于滚动键实现）

- [x] 1.1 S1 估算步进验证：实现"内容估算步进"函数（条目行数 × grid `line_h` +
      卡片 chrome 已知，markdown 折行为误差源），gpui 测试里以 `logical_scroll_top`
      度量 PageDown 落点位移；验证：落点距真一页 ≤ 1/3 视口的断言测试全绿，
      实测数据记录在任务结论（不达标 → 在此记录降级 S2 的选择，后续任务按结论走）
      —— **结论：采用 S1，S2 不启用**。实测（`page_step_lands_within_tolerance`，
      视口 519×800、真一页 420px、门槛 173px）：标定 text_line=26、folded=62.5、
      推进宽 11.875（=760/64 容量定律，D1/D2/D3 精确、带空格 E1 +1 行）；
      WRAP 412 字符实际 7 行 = 估算 7 行；落点误差 跨度①（A 区）22.5px、
      跨度②（跨 WRAP）1.5px，均 ≪ 173px。位移量度法 = from 帧内双标记间距
      （跨帧同位标记相减会得 0）
- [x] 1.2 `scroll_to_item` 对齐语义验证：测试断言 `scroll_to_item(ix)` 后
      `logical_scroll_top` 为 `item_ix == ix` 且 `offset_in_item ≈ 0`（条目顶对齐）；
      验证：测试绿 → 3.2 按顶对齐实现；**非顶对齐 → 停下回看用户**
      （spec「视口顶移到消息起始行」与能力冲突，不得静默降级）
      —— **结论：顶对齐成立**（`scroll_to_item_aligns_to_item_top`：{3, 0}），
      3.2 跳转按条目顶对齐实现

## 2. 键路由补线（簇1）

- [x] 2.1 `route_keystroke` 新增共享键臂（`alt+up`、`ctrl+p`、`ctrl+shift+p`、
      `shift+tab`、菜单关闭时 `tab`、`ctrl+x`）并收紧 completion 臂（`up/down/pageup/
      pagedown/tab` 不再吞 ctrl 组合，使 `ctrl+↑` 在菜单开着时仍进滚动层）；
      验证：`route_keystroke_three_zones` 矩阵扩展为全键位表、全绿
      （含 D1 优先序：overlay → 共享键 → 菜单键(!ctrl) → 滚动键 → 编辑器）
- [x] 2.2 新路由键端到端：gpui 测试逐键断言"进共享分发且**不落输入编辑器**"
      ——`alt+up` 取回排队回填输入框、`tab` 打开路径补全菜单、`shift+tab` 状态行
      thinking 级别变化、`ctrl+p` 模型循环命令发出且输入区内容不变；
      验证：新增 e2e 测试全绿（`p1_shared_keys_route_to_shared_dispatch`；
      注：假 pi clear_queue 应答回填固定文案；测试调度器需显式 `pump_ui_events`
      拉回报泵；ctrl+c 全程仅一次——500ms 双击会关窗）
- [x] 2.3 剪贴板键：`ctrl+x` 走共享复制通路（sink 注入优先、stdout 落回）——
      测试注入 sink 断言最后一条 assistant 文本被复制、无回复红条；`ctrl+v` 文本
      粘贴验证 kit Textarea 原生 Paste 行为（注入文本粘贴动作 → 光标处插入）；
      验证：两测试绿；**粘贴不通则停下回看**（spec 契约依赖内建行为，不得静默降级）
      ——三测全绿：`ctrl_x_copies_last_reply_via_sink`（get_last_assistant_text
      应答 → pump 回报泵 → sink 落文本）、`ctrl_x_without_reply_is_red_notice`、
      `ctrl_v_pastes_clipboard_text`（内建 Paste 动作原生即通，未触发回看）；
      注：TerminalSinks 以 pub(crate) 重导出供注入；测试拉回报泵需轮询
- [x] 2.4 `ctrl+j` 换行验证（design D5）：测试 kit Textarea 对 `ctrl+j` 的原生
      行为；验证：原生换行 → 仅补路由断言；否则在本任务结论记录"已知差异"，
      不桥接 `App.editor`
      —— **结论：内建键位表无 ctrl-j 绑定（state.rs KeyBinding 全表核对 +
      实测 `ctrl_j_newline_native_behavior` 绿），记已知差异，不桥接**
- [x] 2.5 更新 `Docs/tui-usage.md` GPUI 节：新键位条目（取回/模型循环/thinking
      循环/路径补全/复制/粘贴）与 ctrl+j、图片粘贴差异注记；
      验证：文档键位表与实现逐项对得上（P1 键位条目 + 键位差异条目已入 GPUI 节）

## 3. 滚动命令（簇2，依赖组 1 结论）

- [x] 3.1 新增第三路由态 `Route::ToScroller` 与滚动键臂（`pageup/pagedown`、
      `shift+page*`、`ctrl+home/end`、`ctrl+up/down`±shift），保持
      overlay → completion(无 ctrl) → 滚动键 → 共享键 → 编辑器 的优先序；
      验证：路由矩阵测试全绿（含菜单开着时 `ctrl+↑` 不进菜单臂）
      ——矩阵并入 `route_keystroke_three_zones` 全键位表；拦截层 ToScroller
      分支直达 `handle_scroller_key`（不碰编辑器镜像）
- [x] 3.2 顶/底与消息跳转：`ctrl+home/end` → `scroll_to_item(0)` / `scroll_to_end()`
      （恢复贴底跟随）；`ctrl+↑/↓` 目标 = 非空 User/Assistant 条目序（镜像
      `msg_seq`），端点归顶/贴底；"视口顶在消息中部时上跳取哪条"以 TUI
      `scroll.rs` 现行为基准（实施首步读取对齐）；验证：单测断言
      `logical_scroll_top.item_ix`（跳转落点、端点、输入区不变、菜单/浮层下不滚）
      ——TUI 基准 = `prompt_jump_target`（prev 严格低于顶、顶在消息中部取该
      消息起点、无则归顶；next 严格高于顶、无则贴底）；单测
      `jump_target_vectors` + 端到端 `scroller_keys_scroll_and_leave_input_alone`
- [x] 3.3 整页/半页按组 1 结论实现（S1 估算步进 或 S2 保守步进），方向/端点/
      输入不动语义由测试锁死；验证：PageDown/PageUp/shift 半页位移方向与
      量级断言测试绿（S2 时按近似语义断言）
      ——按 S1 实现（`page_down/up_target` + `ScrollMetrics::production`），
      端到端断言方向与量级全绿（含菜单/浮层不滚、输入不变）
- [x] 3.4 更新 `Docs/tui-usage.md` GPUI 节滚动键段与差异注记（页粒度按 1.1 结论）；
      验证：文档与键位表、spec 场景逐项对应（滚动键段 + 页粒度差异已入 GPUI 节）

## 4. 消息区滚动条

- [x] 4.1 启用内建滚动条（`MessageScroller::scrollbar(true)`）并适配浅色主题
      可见性（必要时 `with_list_style` 调色）；验证：测试断言超视口时滚动条元素
      在场、不足一屏不在场、浮层打开时不可交互
      ——默认即 true，显式 `.scrollbar(true)` 自文档化；行为测试
      `scrollbar_drag_only_when_overflow_and_unoccluded`（右缘 16px 轨道、
      thumb 中心=右缘−6，扫掠定点；hover 入场需真实时间多帧）；浅色可见性
      交 6.3 PTY 冒烟（如不可见再 `with_list_style` 调色）；连续拖动手感 =
      内建组件行为（其自身套件覆盖）
- [x] 4.2 更新 `Docs/tui-usage.md` GPUI 节滚动条条目 + 与 TUI 瞬态显隐语义的
      差异注记；验证：文档条目与实现一致（滚动条条目 + 瞬态语义差异已入 GPUI 节）

## 5. 斜杠全集解闸

- [x] 5.1 移除 GPUI 侧 `slash_subset_only` 置位并拆除子集机制
      （`commands::gpui_slash_supported`、`GPUI_*_SUBSET` 常量、`dispatch_slash`
      子集分支），闸门测试 `gpui_subset_gates_query_and_stubs_rest` 改写为全集
      分发断言；验证：列 A `cargo test` 全绿（TUI 行为零变化）+ 改写测试绿
      ——字段/常量/分支/置位全部拆除，闸门测试改写为
      `dispatch_slash_full_set_matches_tui`；列 A 415 + 列 B 491 全绿
- [x] 5.2 被闸命令逐个验证（`/new /clone /name /export /reload /compact /copy`
      等）：gpui 端到端断言各自可观测效果（RPC 发出 / 本地状态变化 / 剪贴板
      出口 / 桩提示同款），`/copy`、`/export` jsonl 优先（依赖组 2 出口）；
      验证：逐命令测试矩阵全绿
      ——`gpui_slash_full_set_e2e`：new/clone/name/reload/copy RPC 入日志、
      /export 执行提示非桩、/share 桩同 TUI、未知交普通 prompt；注：实测
      /export jsonl = 本地文件复制（fs::copy）不经剪贴板——spec 原句系误读
      测试名，已在 delta 中更正；jsonl 出口依赖不成立，剪贴板出口由 /copy
      与 ctrl+x 同源测试覆盖
- [x] 5.3 更新 `Docs/tui-usage.md` GPUI 节斜杠段（子集 → 全集，桩集合与 TUI
      一致）；验证：文档命令表照做可用（斜杠段已改全集分发）

## 6. 集成验收

- [x] 6.1 双矩阵全绿：`cargo test` + `cargo clippy --all-targets`（列 A，共享
      零回归）与 `cargo test --features gpui-support` + clippy（列 B）零告警
      ——A 415+10+3 / B 492+10+3 全绿，clippy 双列 0（Route 同前缀 lint 加
      allow、spike 标定未用变量改 _ 前缀）
- [x] 6.2 `openspec validate align-gpui-input-scroll --strict` 通过
- [x] 6.3 PTY 人工冒烟：真实终端跑 `pi-shell-gpui`，对照 `Docs/tui-usage.md`
      键位表与 `pi-tui-interaction.md` 逐项验证组 2-5 的可观测行为
      （键不落输入、滚动方向/端点、滚动条拖拽、斜杠全集、ctrl+x 复制到终端）
      ——用户验收通过（2026-11-26 实机窗口冒烟）

## Workflow follow-up

- 评审通过后 `/opsx-archive align-gpui-input-scroll`（规格同步 + 归档）。
