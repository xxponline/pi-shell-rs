# Tasks

## 1. 消息起始行标记（chat.rs / wrap.rs）

- [x] 1.1 `chat_lines`：用户/assistant 条目渲染后回填首逻辑行
      `meta[logical_start] = Some(HitKind::MessageStart(seq))`（seq 全局递增、
      仅这两类消耗；Notice/Bash 不标不耗；`append_blocks` 不动，design D3）。
      验证：`cargo test chat::`——新单测
      `message_starts_marked_on_user_and_assistant_only`（User/Assistant 首行
      有标且 seq 递增、通知/`!`bash 行无标、meta 与行等长）。
- [x] 1.2 `wrap.rs` 分组验证（代码零改动，design D1）：新单测
      `message_start_spans_wrapped_rows_and_stays_absolute`（首行超宽换行成
      多屏行 → 同 seq 连续成一段、start=首屏行绝对号；相邻不同 seq 分段；
      pinned 态绝对号不变）。验证：`cargo test wrap::` 全绿。

## 2. 意图与分发（mod.rs / scroll.rs）

- [x] 2.1 `key_intent` 新映射（guard 顺序即优先级，design D4）：
      `Up/Down if ctrl => JumpPrompt(±1)`（ctrl+shift 同臂）、`Home/End if ctrl
      => ScrollTop/Bottom`、`PageUp/Down if shift => ScrollHalfPage(±1)`
      （先于无 guard 的 PageUp）。验证：`cargo test`——单测
      `jump_and_absolute_keys_map_before_edit_family`（四组键意图正确、
      ctrl+shift+↑ 与 ctrl+↑ 同意图、shift+PageUp=Half 而非 Full、
      alt+↑ 仍 Dequeue、既有 ctrl+t/菜单键回归）。
- [x] 2.2 `scroll.rs` 纯函数 `prompt_jump_target(starts, top, max_top, dir)`
      （prev=严格更小否则 0、next=严格更大否则 max_top，design D2）；
      `handle_key` 前序块分发四意图——`set_top_abs` + 半页
      `scroll_up/down_lines(page()/2.max(1))`、**前后 top 相同不 mark**、
      与 ScrollPage 同块（菜单/浮层前序规则天然继承，design D5/D6）。
      验证：`cargo test`——单测
      `prompt_jump_target_vectors_and_half_page_dispatch`（中部 prev/next、
      top 恰在 start 连按不卡、端点 clamp、空表；半页量；top 不变不 mark；
      浮层打开跳转被吞）。

## 3. 文档

- [x] 3.1 `Docs/tui-usage.md` 键位表补四组（ctrl+↑/↓ prompt 跳转、
      ctrl+home/end 顶/底、shift+PageUp/Down 半页——并注明菜单打开时
      shift+PageUp 仍整页翻菜单）；`Docs/roadmap.md` 滚动挂账更新
      （prompt 跳转/半页落地；行级销账注明 pi `lineUp/lineDown` 默认空绑、
      我们无键位配置系统 → 已对齐）。验证：grep 两处均命中。

## 4. 验证

- [x] 4.1 PTY 手验（`/tmp/pty_prompt_jump_check.py`，假 pi 注入多条
      user/assistant 消息 + 长文本）：ctrl+↑ 视口顶移到上一条消息头并连按
      归顶；ctrl+↓ 中间跳下一条、过最末贴底恢复跟随（滚动条点亮顺带验证）；
      ctrl+home/end 顶/底；shift+PageUp/Down 半页位移（页长一半）；
      `/` 菜单打开时 shift+PageUp 菜单翻页而消息区不动、ctrl+↑ 照常跳转；
      浮层（ctrl+l 选择器）打开时 ctrl+↑ 不跳转。验证：脚本
      `N/N steps passed`。
- [x] 4.2 全量门禁：`cargo test` 全绿、`cargo clippy --all-targets` 零告警、
      `openspec validate add-prompt-jump-scroll-keys --strict` 通过。
      验证：三条命令输出（test result ok / 0 warnings / Valid）。

## Workflow follow-up

- 归档：`/opsx-archive add-prompt-jump-scroll-keys`（tui-app ADDED 同步：39→40 需求）
- 提交两批：`feat(tui)`（代码 + Docs）→ `docs(specs)`（归档 + 主规格），按需 push
