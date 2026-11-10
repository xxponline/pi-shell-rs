# Tasks

## 1. 滚动状态层（src/app/scroll.rs）

- [x] 1.1 新建 `src/app/scroll.rs`：`ChatScroll { top, following, viewport }`
      + `Default`（following=true）+ `PAGE_SCROLL_OVERLAP=4`，`pub use` 挂到
      `app/mod.rs`；验证：`cargo build` 通过且单测覆盖默认值语义
      （default 即贴底跟随）
- [x] 1.2 实现纯方法 `page()`（`max(viewport−4, 1)`，viewport=0 时退化为 1）、
      `page_up()`（打断跟随 + 饱和上移）、`page_down()`（饱和下移，不改
      following）、`absorb(effective_top, at_bottom)`（回写 top、
      `following = at_bottom`）；验证：单测覆盖页长计算、跟随打断、
      饱和下移不越界、absorb 恢复跟随四类断言，`cargo test scroll::` 全绿

## 2. 渲染窗口化与回写

- [x] 2.1 `wrap_for_viewport` 增加 `scroll: &ChatScroll` 入参，返回
      `(Vec<Line>, ChatScroll)`：先换行+样式+pad（现状不动），再按
      `following ? len−height : min(top, len−height)` 开窗口并 `absorb`；
      验证：既有 `over_height_keeps_tail_not_head` 改传
      `ChatScroll::default()` 后断言**不变**（following=现行为回归网），
      新增 pinned 窗口单测（top 中段窗口化 + clamp 到底恢复跟随），
      `cargo test wrap::` 全绿
- [x] 2.2 `ViewUi` 增加 `pub scroll: ChatScroll`（按值 Copy，保持
      derive(Default)）；`render_chat`/`view::render` 透传并返回吸收后的
      `ChatScroll`；验证：`cargo build` 通过，view 既有测试
      （`..Default::default()` 构造）无需改动即编译，新增单测断言
      `render` 返回值在 following/pinned 两态下的 top 与 following
- [x] 2.3 `App::draw` 改 `&mut self`，闭包内经 `Cell` 收集 `render` 返回值、
      闭包外回写 `self.scroll`（调用点 `mod.rs:320/464` 已在 `&mut self`
      方法内）；验证：`cargo build` 编译期证明借用合法，
      `cargo test` 全绿（含既有 draw 路径测试）

## 3. 键位通路

- [x] 3.1 `key_intent`：`PageUp | PageDown` 移出 `Edit` 臂 →
      `KeyIntent::ScrollPage(-1/+1)`；验证：`key_intent_mapping_matches_spec`
      增补两键断言，`cargo test key_intent` 全绿
- [x] 3.2 `handle_key` 在补全菜单消费块之后、`ForcePath` 同构位置拦截
      `ScrollPage`，调 `self.scroll.page_up()/page_down()`；验证：单测断言
      ①菜单关闭时 PageUp 打断跟随且 top 饱和上移、②菜单打开时 PageDown
      仍走 `completion_move(5)`（既有 `completion_menu_opens_on_slash_and_navigates`
      回归）、③浮层打开时 PageUp 不改 `self.scroll`（`overlay_key` 吞键）

## 4. 骤变重置（规格场景 3 主路径）

- [x] 4.1 `AsyncKind::Messages` 分支（会话切换链/重建应答）回写
      `self.scroll = ChatScroll::default()`；验证：单测模拟"长会话 pinned →
      load_messages 切换"后 scroll 恢复贴底跟随（top 归零、following=true）

## 5. 文档与集成验证

- [x] 5.1 `Docs/tui-usage.md`：删除"消息区只自动跟随最新内容，暂不支持向上
      滚动回看"已知差异条目，键位表补 PageUp/PageDown（整页回看、页长−4
      重叠）；验证：文档无"不支持回看"字样、键位表含两键
- [x] 5.2 `Docs/roadmap.md`：移除滚动回看挂账条目（该能力已由本 change 落地）；
      验证：roadmap 不再列消息区滚动为挂账项
- [x] 5.3 全量门禁：`cargo test` 全绿 + `cargo clippy --all-targets` 零告警 +
      `openspec validate add-message-scrollback --strict` 通过；
      验证：三命令输出即证据
- [x] 5.4 PTY 手工验证（脚本注入确定性事件流 + pyte 断言，参考
      `Docs/tui-usage.md`）：①多屏内容 PageUp 回看、输入区不被覆盖
      ②上滚后注入流式 message_update，视口不动 ③PageDown 回底后新内容
      恢复贴底跟随 ④模拟会话切换（load_messages）后视口回底；
      验证：四项屏幕断言全过

## Workflow follow-up

- 实现完成后走 `/opsx-archive` 归档，随归档同步主规格
  （`openspec/specs/tui-app` 新增「消息区滚动回看」需求）。
