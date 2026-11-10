# Tasks

## 1. 覆盖表与全局/重载语义（mod.rs）

- [x] 1.1 `App` 增 `thinking_overrides: HashMap<usize, bool>`；`KeyIntent::ToggleThinking`
      翻转 `show_thinking` 后 `clear()` 覆盖表（design D3，pi setHideThinkingBlock
      parity）。验证：单测 `ctrl_t_flips_global_and_clears_thinking_overrides`
      ——先塞两键 → 按 Ctrl+T → 表空且全局取反；再按 → 回到初始。
- [x] 1.2 `AsyncKind::Messages` 臂（`load_messages` + 滚动重置同处）追加
      `thinking_overrides.clear()`（design D6）。验证：单测
      `session_reload_clears_thinking_overrides` ——塞键后投递
      `AsyncKind::Messages` 应答 → 表空（既有 4178 行同型测试为模板）。

## 2. 命中管道泛化（wrap.rs / chat.rs / view.rs）

- [x] 2.1 `wrap.rs`：`HitKind{Tool(String), Thinking(usize)}` + `Hit{start,end,kind}`
      取代 `CardHit`；`meta` 类型改 `Vec<Option<HitKind>>`；
      `collect_card_hits`→`collect_hits`、`card_hits` 字段→`hits`、
      `wrap_for_viewport` 签名同步（design D1）。验证：`cargo test wrap::`
      全绿——既有 `card_hits_span_wrapped_lines_and_align_with_window`、
      `card_hits_are_absolute_when_pinned` 更名适配并断言 `kind == Tool(..)`。
- [x] 2.2 `chat.rs`：`chat_lines` 增 `thinking_seq`（内部建、`&mut` 穿透）与
      `thinking_overrides` 参数；`append_blocks` 两参齐增——thinking 臂
      **每个块无条件 `*thinking_seq += 1`（含空块）**、仅非空块的全部行打
      `HitKind::Thinking(seq)`；工具臂改打 `HitKind::Tool(id)`（门控
      `result.is_some()` 不变）（design D2/D5）。验证：`cargo test chat::`
      ——新单测 `thinking_meta_marks_nonempty_and_seq_skips_nothing`
      （空块消耗序号但无标记、非空块整段标记、两次渲染序号一致）+
      既有 `card_meta_only_marks_finished_tool_cards` 更名适配
      `HitKind::Tool` 断言。
- [x] 2.3 `view.rs`：`ViewUi` 增 `thinking_overrides: Option<&HashMap<usize,bool>>`
      （`derive(Default)` 不破）；`render_chat` `unwrap_or(&empty)` 接入
      `chat_lines`（design D5）。验证：`cargo test view::`——新单测
      `render_uses_thinking_override`（全局折叠 + 覆盖 true → 全文可见；
      `None` → 占位不可见），既有 hits/render 测试更名适配。

## 3. 点击分发（mod.rs）

- [x] 3.1 `card_at`→`hit_at` 返回 `HitKind`；`handle_click` 浮层早退后
      `match kind`：`Tool` 走既有工具覆盖分支，`Thinking(seq)` 相对翻转写
      `thinking_overrides` 并 log（design D4；`event.rs` 零改动，既有
      `only_left_down_forwarded_for_add_tool_card_click_boundary` 护航）。
      验证：`cargo test input_tests::`——新单测
      `click_toggles_thinking_block_independently`（seq1 点开：仅键 1 置位、
      seq0/工具覆盖不动；再点恢复；浮层/行号越界沿用既有断言）+ 既有
      `click_toggles_hit_card_and_respects_bounds_overlay` 适配
      `Hit{kind:Tool(..)}` 构造。

## 4. 文档

- [x] 4.1 `Docs/tui-usage.md`：鼠标左键行补"thinking 块"（工具卡片与
      thinking 块均可点击切换）、Ctrl+T 行补"并重置单块状态"、
      示意区 thinking 行补"点击单块切换"。验证：grep 三处均命中
      （`鼠标左键点击.*thinking`、`Ctrl+T.*重置单块`）。

## 5. 验证

- [x] 5.1 PTY 四场景手验（`/tmp/pty_thinking_click_check.py`，PI_BIN 假 pi 注入
      thinking 事件）：① 点击折叠占位展开且他块/全局不变 ② 再点恢复占位
      ③ 流式中点击（thinking_delta 注入 → 点击 → 后续 delta 仍展开）
      ④ 单块切换后 Ctrl+T 全部跟随全局、再点仍可独立。验证：脚本
      `N/N steps passed`（屏幕断言含 `<thinking` 占位与全文特征串）。
- [x] 5.2 全量门禁：`cargo test` 全绿、`cargo clippy --all-targets` 零告警、
      `openspec validate add-thinking-click --strict` 通过。验证：三条命令
      输出（test result ok / 0 warnings / Valid）。

## Workflow follow-up

- 归档：`/opsx-archive add-thinking-click`（tui-app ADDED 同步：37→38 需求）
- 提交两批：`feat(tui)`（代码 + Docs）→ `docs(specs)`（归档 + 主规格），
  按需 push
