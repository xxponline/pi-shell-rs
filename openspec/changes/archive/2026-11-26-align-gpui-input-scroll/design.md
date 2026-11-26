# Design

## Context

- **路由现状**：`route_keystroke` 只有两态（`ToApp` / `ToEditor`）+ 约十臂白名单，
  overlay 打开时全量进共享分发；漏网键（page 系、alt+up、ctrl+p、shift+tab、
  ctrl+x…）全部落 Textarea——会动编辑器光标（如 `ctrl+home` = 行首），语义错误。
- **滚动现状（本变更的咽喉）**：两条互不相识的真相——
  - TUI：`App::scroll`（行级，follow/absorb/pin + `MessageStart` meta），仅 ratatui
    路径消费；GPUI 渲染对它 **0 引用**（已验证）。
  - GPUI：`MessageScrollerState`（gpui-component，条目级，`FollowMode::Tail` 贴底
    跟随）。公开面 = `scroll_to_item(ix)`（条目顶对齐）/ `scroll_to_end()` /
    `logical_scroll_top() -> ListOffset{item_ix, offset_in_item}`（只读）/
    计数与对账方法；内部 `list_state` **私有**，`scroll_to(ListOffset)` 与像素偏移
    **未暴露**；wheel 由 kit 内部处理（共享层 `input.rs` 的加速模块在 GPUI 侧无消费者）。
- **斜杠现状**：`App.slash_subset_only` 标志 + `dispatch_slash` 子集分支 +
  `commands::gpui_slash_supported`（单调用点 `mod.rs:3451`）；TUI 默认全量分发。
- **剪贴板出口**：`mod.rs:342` "sink 注入优先，standalone 落回 OSC52 写 stdout"——
  窗口形态天然复用启动终端，`ctrl+x` / `/copy` / `/export` jsonl 共用此通路。
- **语义基准**：`key_intent` 全表、tui-app 规格「消息区提示跳转与滚动键」
  「消息区滚动条」「剪贴板复制与粘贴」；跳转目标算法的唯一基准是 TUI `scroll.rs`
  现行为（含"视口顶位于消息中部时上跳取哪条"的边界，实施时读取对齐）。

## Goals / Non-Goals

**Goals:**

- 滚动单一真相：GPUI 滚动键直达 `MessageScrollerState`，`App.scroll` 保持 TUI 专属。
- 键位可达性与 TUI 同语义，且新路由键绝不落入输入编辑器。
- 共享层仅加法：TUI 行为零变化（列 A 全绿为验收），GPUI-only 文件自由改动。
- 斜杠全集分发后，每个此前被闸的命令都有可执行的出口（含剪贴板类）。

**Non-Goals（设计级边界）:**

- 不 fork / patch gpui-component（依赖手术否决）；不引入行级 item 枚举
  （行数依赖布局结果，虚拟化无法前置计数）。
- 不桥接 `App.scroll` ↔ scroller（双写必漂移）；不在 GPUI 重建行级滚动状态。
- 不改 TUI 任何渲染与键位行为；不动 `key_intent` / `handle_key` / 复制通路语义。
- 选区复制、ctrl+g、图片粘贴、wheel 手感、瞬态滚动条语义、主题、alt-screen 搜索、
  ctrl+d——见 proposal「明确不做」，本设计不再展开。

## Decisions

### D1 第三路由态 `Route::ToScroller`，键位按三层优先分派

优先序（自上而下，先命中先赢）：

1. `overlay_open` → `ToApp`（不变：浮层消费焦点键）。
2. completion 菜单键（`up/down/pageup/pagedown/tab`，**且未按 ctrl**）→ `ToApp`
   （不变：菜单导航/整页翻页）。收紧点：现有 completion 臂会吞掉 `ctrl+up`——
   必须让 ctrl 组合先落到第 3 层，才能复现 TUI「菜单不消费 `ctrl+↑`、照常跳转」。
3. **新增滚动键臂 → `ToScroller`**：`pageup/pagedown`（无 completion 时）、
   `shift+pageup/pagedown`、`ctrl+home/ctrl+end`、`ctrl+up/ctrl+down`（±shift 同义）。
4. **新增共享键臂 → `ToApp`**：`alt+up`（取回排队）、`ctrl+p` / `ctrl+shift+p`
   （模型循环）、`shift+tab`（thinking 循环）、`tab`（菜单关闭时强制路径补全）、
   `ctrl+x`（复制）。
5. 其余 → `ToEditor`（不变）。

否决的替代：

- **全部 `ToApp`**：共享 `handle_key` 的滚动意图写 `App.scroll`，GPUI 渲染不读——
  "假成功"；且 `ctrl+home` 等落 Textarea 会动输入光标，spec 明令 MUST NOT。
- **桥接 `App.scroll` → scroller**：wheel 走 kit 内部不经 App，双真相必然漂移；
  桥接需行↔条目双向映射，而行枚举依赖布局（见 Non-Goals）。

### D2 滚动命令 → scroller 映射；页语义以 spike 定案（本设计的核心未知数）

- **顶/底**：`scroll_to_item(0)` / `scroll_to_end()`——后者内部恢复
  `FollowMode::Tail`，天然满足 spec「底 = 恢复贴底跟随」。
- **消息跳转（精确）**：目标条目下标 = 非空 User/Assistant 条目序（镜像 TUI
  `msg_seq` 计数规则）；条目即消息起点，`scroll_to_item` 的条目顶对齐 = 视口顶移到
  消息起始行，**粒度天然精确**。当前视口顶读 `logical_scroll_top().item_ix`；
  端点（已最早/已最末）→ 归顶 / 贴底恢复跟随。"视口顶在消息中部时上跳取哪条"
  以 TUI `scroll.rs` 实际算法为准（实施首步读取）。
- **整页/半页（近似，spike S1 前置）**：公开 API 无像素偏移（`list_state` 私有、
  `scroll_to(ListOffset{offset_in_item})` 未暴露），条目级是可达粒度上限。
  - **S1 首选：内容估算步进**——条目几何由我们自己的行构建产出（行数 × grid
    `line_h` + 卡片 chrome 行数已知，markdown 折行是唯一误差源），估算"覆盖约一个
    视口的条目跨度"→ `scroll_to_item(目标)`。**验收门槛：落点距真一页 ≤ 1/3 视口**
    （测试里以 `logical_scroll_top` 前后位移量度）。
    → **spike 实测定案（tasks 1.1/1.2）：采用 S1，S2 不启用**——折行模型 =
    段落拼接 + 容量定律（760px 文本宽 = 64 字符，推进宽常数 `CHAR_ADVANCE_PX`）；
    落点误差 22.5px（代表内容）/ 1.5px（跨长行）≪ 173px 门槛；`scroll_to_item`
    条目顶对齐成立；位移量度用 from 帧内双标记间距（跨帧同位标记会得 0）。
  - **S2 降级：保守固定步进**——S1 不达标时按固定条目步进实现（方向/端点/输入
    不动语义不变），Docs 记"页粒度为条目近似"差异；滚动条拖拽提供精确手动定位兜底。
  - **升级线**：上游若暴露 `scroll_to(ListOffset)`，页语义可无损升级为精确页
    （记录，不阻塞本变更）。
- S1 同时验证 `scroll_to_item` 的对齐语义（顶对齐 vs nearest）：若非顶对齐，
  跳转 spec 场景（"视口顶移到消息起始行"）与实现冲突——**回看用户**，不得静默降级。

### D3 滚动条 = 内建开关，接受组件行为

`MessageScroller::scrollbar(true)` 即得超视口可见 + 拖动定位（spec 已写明不承诺
TUI 瞬态显隐/悬停语义）。风险点是浅色主题下的可见性与配色——冒烟验收，必要时
以 `with_list_style` / 主题 token 微调（不改行为契约）。

### D4 斜杠解闸 = 翻转 GPUI 侧标志 + 拆除子集机制

- GPUI 构造 `App` 时不再置 `slash_subset_only`（TUI 默认值即全量分支）→
  `dispatch_slash` 直通 `commands::dispatch`。**共享行为零变化**：子集分支只在
  GPUI-only 模式下可达，列 A 全绿即证明（additive 规则的验收方式）。
- 随之移除：`commands::gpui_slash_supported`、`GPUI_QUERY/OVERLAY_SUBSET` 常量、
  `dispatch_slash` 子集分支；闸门测试 `gpui_subset_gates_query_and_stubs_rest`
  改写为全集分发断言（含原"未支持桩"场景改为本地执行/桩同款断言）。
- 剪贴板依赖先行验证：`/copy`、`/export` jsonl、`ctrl+x` 共用 sink→stdout 通路。

### D5 `ctrl+j` 换行：验证优先，不入 spec

实施时验证 kit Textarea 是否原生把 `ctrl+j` 映射为换行：原生支持 → 仅补路由与
测试；否则记 Docs 差异。**不桥接** `App.editor` 回写——"GPUI 文本所有权留在
Textarea"是既定边界，为此开洞得不偿失。

### D6 共享层改动清单（仅加法的落地口径）

只碰两处、均可由列 A 证明零回归：`dispatch_slash`（删 GPUI-only 子集分支）、
`route_keystroke`（GPUI-only 文件）。`key_intent` / `handle_key` / `commands::dispatch` /
复制通路 / 滚动语义零改动。

### D7 文档与已知差异清单

`Docs/tui-usage.md` GPUI 节补：新键位表、滚动条、斜杠全集；差异表新增：滚动条
瞬态语义（不做）、页粒度（视 S1/S2 结果）、wheel 手感（kit 内建）、图片粘贴、
ctrl+g、ctrl+d、（ctrl+j 视 D5 验证）。

## Risks / Trade-offs

- [路由臂优先级错放 → 菜单/浮层下键位回归] → `route_keystroke_three_zones`
  矩阵扩展为全键位表 + 菜单/浮层场景端到端测试（键后断言输入区内容不变）。
- [S1 估算落点超容差] → spike 前置于滚动键任务；降级 S2 并记差异，方向/端点
  语义由测试锁死，滚动条拖拽兜底精确操作。
- [`scroll_to_item` 对齐语义非顶对齐] → S1 一并实测；不符则回看用户
  （spec 场景措辞 vs 实现能力），不静默降级。
- [gpui 输出污染 stdout → OSC52 失效] → 冒烟用真实终端验证复制；兜底 = 把出口
  改为显式注入 sink（共享层加法，仍不改 TUI 行为）。
- [斜杠全集暴露依赖 TUI 环境的命令] → 被闸的约 7 个命令逐个冒烟矩阵，
  `copy/export` 先行（剪贴板出口依赖最重）。
- [拆子集机制触碰共享文件] → 列 A 全绿 + `dispatch_slash` 全量分支行为与 TUI
  逐字节同路径（单测覆盖）。

## Migration Plan

任务顺序即部署顺序：S1 spike → 路由（可独立验收）→ 滚动键（依赖 S1 结论）→
滚动条 → 斜杠解闸 → 文档与双矩阵/PTY 冒烟。每组独立全绿后进入下一组；
全部改动位于 `gpui-support` 门控文件与 `dispatch_slash` 单点，回滚 = revert 提交。

## Open Questions

无——所有未知（页语义粒度、对齐语义、ctrl+j、stdout 污染）均已给出 spike/冒烟
决策规则与降级路径，不阻塞任务拆分。
