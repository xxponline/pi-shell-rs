# Design

## Context

动机见 proposal.md。实测事实（决定方案形态）：

- pi 键位（`tui/src/keybindings.ts`）：`previousPrompt` = ctrl+↑ / ctrl+shift+↑、
  `nextPrompt` = ctrl+↓ / ctrl+shift+↓、`top/bottom` = ctrl+home / ctrl+end、
  `halfPageUp/Down` 与 `lineUp/Down` **默认键为空**（动作留给用户 settings）。
- pi OSC133 自产自销：user/assistant 组件给自己的行包 `133;A` 标记、
  绘制时剥离（`tui-alt-screen:525` 按 `133;A` 找边界）；bash 组件 0 处标记
  → 语义消息 = 用户 + assistant，零 shell 集成依赖。
- 我们侧原语齐备：`ChatScroll::set_top_abs`（clamp + 跟随恢复）与
  `mark_scroll_activity`（滚动条点亮）由 `add-message-scrollbar` 交付；
  `meta: Vec<Option<HitKind>>` / `collect_hits` 已泛化。
- 键位空闲实测：ctrl+↑/↓ 现走 `Edit`→`edit_key` 无绑定（忽略）；ctrl+home/end
  同（编辑器 home 由 ctrl+a 承担）；shift+PageUp 现**误撞全页**
  （`PageUp` 无修饰键 guard）。补全菜单前序门 =
  `modifiers == NONE || SHIFT`（shift+PageUp 被菜单消费、ctrl+键穿透）；
  浮层 `overlay_key` 先于一切吞键。

## Goals / Non-Goals

**Goals:**

- pi 默认键全量对齐：prompt 跳转（两组键）、顶/底、半页。
- 零新滚动 API、零 `event.rs` 改动、`wrap.rs` 零改动（复用既有泛化）。
- 跳转沿用滚动条点亮规则（top 变化才 mark），行为与既有滚动完全一致。

**Non-Goals:**

- 行级滚动（pi `lineUp/lineDown` 默认空绑 + 我们无键位配置系统 → 无处可挂，
  挂账销账注明）；键位可配置化（无 settings 体系，同前几轮决定）。
- OSC133 escape 字节（pi 仅作内部标记并在绘制时剥离——我们结构性收集，
  终端不可见性等价）；通知/`!` bash 条目作为跳转点（pi 未标记）。

## Decisions

### D1 消息起始行载体 = `HitKind::MessageStart(seq)`（每行多标记）

`chat_lines` 对用户/assistant 条目**首逻辑行**追加 `MessageStart(seq)`
（seq 全局递增，仅这两类条目消耗），随 meta 管道穿 `wrap_for_viewport` →
`collect_hits` 分组为绝对屏行区间。

**实施修正（apply 期实测冲突，已验证）**：meta 原为 `Option<HitKind>`
（每行单标记），而消息首行本就常带内容标记——assistant 以 thinking/工具卡
开头时回填会**覆盖** `Thinking`/`Tool`（实测
`left: [MessageStart(0), MessageStart(1), Thinking(2)]` 丢失 `Thinking(1)`）：
折叠 thinking 单行块会整体从点击表消失、卡头行点击失效。故 meta 改为
**`Vec<HitKind>`（每行多标记）**：回填 = `push(MessageStart)` 与内容标记
共存；`collect_hits` 两遍扫描——**先内容类（Tool/Thinking）、后
MessageStart**：`hit_at` 首匹配天然“内容动作优先”，MessageStart 仅在无
内容标记的行被命中（点击 no-op）；跳转按 kind 过滤，与顺序无关。

- **替代：独立 `starts: &[bool]` 平行向量 + render 返回四元组**：仍否——
  chat_lines 返回值 +1 → 约 24 处测试调用点 + render/Cell 连锁；多标记方案
  **零签名变化**（仅 ~10 处测试字面量与 `collect_hits` 重写），波及面小
  一个量级。
- MessageStart **不携带状态**（无覆盖表）→ 无 thinking/tool 覆盖表那类
  会话切换串态问题，跳转永远读当帧表。
- seq 稳定性：transcript append-only → 序号稳定；`load_messages` 整体重写
  后标记由渲染重建，无需清表。

### D2 跳转目标 = 纯函数 `prompt_jump_target`

```rust
// scroll.rs：给定消息起始绝对行、当前 top、max_top、方向 → 目标 top
prev: 最后一个 start < top 的 start；无（top 已在最早消息或之前）→ 0
next: 第一个 start > top 的 start；无（已过最末消息起点）→ max_top（贴底）
```

- 端点 clamp 语义（归顶/贴底恢复跟随）；`pi scrollToPrompt` 的端点细节未
  逐帧核对——PTY 验证端点行为，若与 pi 有出入再修（记录为已知假设）。
- top 恰在某 start 上：prev 取**严格更小**（连按不卡在原地）、next 取
  **严格更大**（跳过当前消息剩余）——与"上一条/下一条"直觉一致。

### D3 打标位置 = `chat_lines` 条目循环回填

条目渲染前记 `logical_start = out.len()`，渲染后（循环尾 `meta.resize` 处）
回填 `meta[logical_start] = Some(MessageStart(seq))`。`append_blocks` 不动；
User 与 Assistant 两分支共用该回填，Notice/Bash 不标不耗 seq。

### D4 `key_intent` 分支布局（guard 顺序即优先级）

```
Up if alt        => Dequeue          （既有，alt 优先于 ctrl——ctrl+alt+↑
                                       归 Dequeue，记录为边缘取舍）
Up if ctrl       => JumpPrompt(-1)   （ctrl+shift+↑ 同臂——两键同动作，pi 同）
Down if ctrl     => JumpPrompt(+1)
Home if ctrl     => ScrollTop
End if ctrl      => ScrollBottom
PageUp if shift  => ScrollHalfPage(-1)   ← 必须先于无 guard 的 PageUp
PageUp           => ScrollPage(-1)   （既有）
…                => Edit             （ctrl+↑/home 现于此被忽略 → 零损失）
```

- 菜单/浮层优先级由**前序块**天然保证（菜单门 `NONE|SHIFT` 消费
  shift+PageUp、放行 ctrl+键；`overlay_key` 浮层全吞——与 ScrollPage 同一
  前序块注释所述规则），新意图与 `ScrollPage` 走同一消费块。

### D5 分发 = 复用原语，top 变化才点亮

- `JumpPrompt(dir)` → `prompt_jump_target` → `set_top_abs(target)`；
  `ScrollTop/Bottom` → `set_top_abs(0 / content_len)`（clamp 自动贴底恢复
  跟随）；`ScrollHalfPage(dir)` → `scroll_up/down_lines(page()/2.max(1))`
  （**零新滚动 API**：`scroll_up_lines` 打断跟随、`scroll_down_lines` 不改
  following，与 `page_up/down` 语义同构，只是量为半页）。
- 所有新意图统一：**前后 `top` 相同则不 `mark_scroll_activity`**（已在顶
  再按上跳不闪滚动条；对齐 pi `scrollTo` 的 `moved !== 0` 规则与本仓库
  ScrollPage/handle_wheel 既有先例）。
- `handle_click` 增 `HitKind::MessageStart(_) => false` no-op 臂——点消息
  起始行无动作，观察等价于 pi 内容行点击。

### D6 半页的菜单语义（实测确认）

菜单门 `modifiers == NONE || SHIFT` → `shift+PageUp` 被菜单消费为
`completion_move(±5)`（整页翻菜单，既有行为不变——spec 场景钉住）；
`ctrl+↑/↓` 因 CONTROL 不满足门条件而穿透到跳转。无需改菜单块。

## Risks / Trade-offs

- [ctrl+↑ 被 Edit 族吞] → ctrl 分支置于 Edit 族 match 之前 + key_intent
  单测钉四组键。
- [shift+PageUp 行为变化（全页→半页）] → 方案一批准的修正；菜单打开时
  仍整页（门条件实测），PTY 双态验证。
- [端点语义与 pi `scrollToPrompt` 未逐帧对齐] → D2 clamp 假设 + PTY 端点
  场景；有出入再修。
- [MessageStart 进点击表的交互面] → 分发 no-op 臂 + 单测；不引入任何状态。

## Migration Plan

纯新增行为，无数据/协议迁移；随归档同步 tui-app（ADDED +1）。回滚 = 还原
本 change 提交。

## Open Questions

（无——PTY 场景编排与跳转向量在 tasks 阶段定。）
