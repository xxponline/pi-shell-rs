# Proposal

## Why

pi 原版的 thinking 块支持左键**逐块**点击展开/收起（`assistant-message.ts`
MouseRegion：`set(runIndex, !hidden)`），而我们目前只有 Ctrl+T 的全有或全无
全局开关——用户想看某一段思考就得把所有 thinking 均摊开。工具卡片点击
（`add-tool-card-click`）刚把命中表管道建好，thinking 是同一语义的天然姊妹：
pi 对两者用的是完全相同的"覆盖 ?? 全局 + 全局切换清覆盖"模型。

## What Changes

- **新增单块点击展开/收起**：左键点击非空 thinking 块的占位行/展开行，在该块
  的展开与折叠间切换（act-on-press，与工具卡片同一分发路径）；点击仅作用于
  被点块，其它 thinking 块、工具卡片与全局展开标志不变。
- **Ctrl+T 语义补齐**：全局切换时同时**清空单块独立状态**（对齐 pi
  `setHideThinkingBlock()` 的 `overrides.clear()`，与工具卡片 Ctrl+O 的
  「全局切换覆盖单卡状态」同构）；此后单块点击仍可独立切换。
- **命中表带类型**：meta/`CardHit` 管道泛化以携带 thinking 块标记（强类型
  分发：工具卡 → 工具覆盖表；thinking → thinking 覆盖表），复用既有
  `UiEvent::Click` → `handle_click` 链路，事件层零改动。
- **单块生效值**：`override ?? flags.show_thinking`，与工具卡片 D1 同模式；
  键取 `Block::Thinking` 的 contentIndex 绝对序号（append-only 下稳定）。
- 文档同步（`Docs/tui-usage.md` 键位/鼠标行、示意）；主规格 tui-app 增加
  ADDED 需求「thinking 块单块展开」。

**明确不做**（本轮范围外）：settings 持久化/可配置（用户已撤回该提议，默认
折叠不变）；连续 thinking 块合并成 run（pi 的 join 语义，实测罕见）；折叠
标签改 `Thinking...` 文案；展开态 markdown 渲染；pi 的空行（needsSpacer）
规则；全局切换状态提示语。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `tui-app`：新增 ADDED 需求「thinking 块单块展开」（左键单块切换、流式中
  点击生效、Ctrl+T 清单块状态）；既有需求「thinking 折叠切换」（默认折叠 +
  Ctrl+T 全局）语义不变，无需修改。

## Impact

- `src/app/chat.rs`：`append_blocks` thinking 两态改读单块覆盖；thinking 行
  打命中 meta（门控 = 非空 thinking 块）。
- `src/app/wrap.rs`：meta/`CardHit` 泛化带类型（具体形态——`HitKind` enum
  或键约定——design 定夺）；`collect_card_hits` 同步。
- `src/app/view.rs`：`ViewUi` 透传 thinking 覆盖表；render 链路。
- `src/app/mod.rs`：thinking 单块覆盖表；`handle_click` 按命中类型分发两路；
  `KeyIntent::ToggleThinking` 增加清覆盖；`AsyncKind::Messages` 重载语义核对
  （键为绝对序号，切换会话后旧覆盖自然失效——design 核实）。
- `src/app/event.rs`：**零改动**（`UiEvent::Click` 已就位）。
- 测试：chat/wrap/mod/view 单测 + PTY 四场景手验；`Docs/tui-usage.md`。
- 不改 session 层（`Block::Thinking` 结构不变）、不改 RPC 协议。
