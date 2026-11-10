# Proposal

## Why

规格与实现/文档三方矛盾：`tui-app`「三分区布局与状态指示」写着"消息区内容超出
高度时 MUST 可滚动查看而不覆盖输入区"，但实现是 `wrap_for_viewport` 底部锚定
**直接丢弃头部行**（旧内容从渲染帧消失），`Docs/tui-usage.md` 还把它登记为
"已知差异"。这不是新需求，是补欠账——规格要求的能力一直缺失。pi 侧由
`ScrollView(follow:"end")` 提供该能力，其 follow 三态语义（贴底跟随/上滚钉住/
回底恢复）是本 change 要对齐的核心。

## What Changes

- **滚动状态**：App 增加消息区滚动 offset 与 follow 标志，经 `ViewUi` 传入渲染。
- **窗口化渲染**：`wrap_for_viewport` 从"保留末尾 height 行"改为"按 offset 开
  窗口 `[top, top+height)`"——现有底部锚定成为 follow=true 的特例。
- **follow 三态**：新内容到达时，贴底跟随则保持贴底；用户上滚后视口钉在原内容
  上（新内容不抢位）；滚回底部恢复跟随。
- **键位**：征用死键 PageUp/PageDown 做整页滚动（页长 = 视口高 − 4 行上下文
  重叠，对齐 pi `PAGE_SCROLL_OVERLAP`）；菜单/浮层的键位优先级保持不变。
- **内容骤变**：session 切换/compact 导致行数骤变时 clamp 到底并恢复跟随。
- **文档**：`tui-usage.md` 移除"不支持回看"已知差异条目。

**移出本 change（roadmap 已挂账）**：鼠标滚轮（需开 `EnableMouseCapture` 并
打破 event.rs"鼠标即丢"约定）、transient 滚动条、alt-screen 搜索、prompt 跳转
（OSC133）、行级/半页滚动键。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `tui-app`：新增 ADDED 需求「消息区滚动回看」——钉住可观察行为：PageUp 整页
  滚动且输入区不被覆盖、上滚后流式新内容不抢位且滚回底部恢复跟随、内容骤变
  clamp 到底恢复跟随。

## Impact

- `src/app/wrap.rs`：`wrap_for_viewport` 增加 offset 入参（窗口化替代丢头部）
- `src/app/view.rs`：`render_chat`/`ViewUi` 传递滚动状态
- `src/app/mod.rs`：App 滚动字段、`key_intent`/`handle_key` 接 PageUp/PageDown、
  绘制时组装 ViewUi；embed 宿主经 `inject_key` 走同一通路免费获得
- `Docs/tui-usage.md`：删除已知差异条目、键位表补 PageUp/PageDown
- 测试：wrap 窗口化/偏移单测、follow 三态纯函数单测、key_intent 断言、
  PTY 手验（上滚→流式不抢位→回底恢复）
