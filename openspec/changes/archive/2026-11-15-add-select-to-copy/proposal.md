# Proposal

## Why

鼠标捕获（`add-mouse-wheel-scroll` 引入）后**终端原生选中失效**——这是
`tui-usage.md` 已知差异里挂着的 C2 项（Shift+拖拽透传只是 workaround，
流式重绘会随时清掉原生选区）。pi 的答案是**应用自建选区**
（`tui-alt-screen.ts` 全文实测：fullscreen owns mouse selection——开捕获的
注释原文就是 "viewport scrolling and application-owned text selection"）。
本 change 复刻 pi 的 select-to-copy 全套：选区状态机、反视频高亮、双击
词/三击行、边缘拖拽自动滚动、**释放即自动复制**。

## What Changes

- **背景拖拽选区**（字符粒度）：按下锚定 → 拖动扩展 → 释放定形；选区以
  **绝对屏行 + 显示列**为坐标（与命中表同空间）；纯点击（无拖动）不产生
  选区。渲染 = 整帧绘制后对 buffer 做 **fg↔bg 翻转**（pi 的 ANSI `[7m`
  反视频等价——底层样式保留，仅前景背景互换）。
- **双击选词 / 三击选行**：500ms 窗口、同行同词边界才连击（pi
  `getClickCount`）；词分段 + 连接符集合 **抄 pi 常量**
  `["/", "-", "+", "\\", "~", "_", "."]`（路径/URL 整体可选），粒度锁定
  后拖拽按词/行延展（pi `initialRange` 语义）。
- **边缘拖拽自动滚动**：指针贴近视口上/下缘时定时滚动并让选区随内容延伸
  （pi 50ms；我们挂既有 200ms tick——**粒度差异记录于 design**）。
- **释放即自动复制**：`copyOnSelect` 硬编码 pi 默认 `true` → 鼠标抬起若
  选区非空 → 走既有双通路复制（`sinks.copy_text` 宿主注入优先、OSC52
  兜底）+ 绿色 `Copied!` 提示；空选区不复制不提示。
- **优先级与清除**（对齐 pi 路由）：浮层 → 滚动条列 → 组件行
  （thinking/工具卡：照常点击**并清除选区**）→ **背景起选**（MessageStart
  行属背景、可选）；会话切换/重建（`load_messages`）清除选区——选区坐标
  指向被替换的 transcript（pi #9311 同理）。
- **文本提取**：复制时**重算** `chat_lines + wrap_for_viewport` 纯函数管线
  取全文行，按选区行/显示列切片 → 行尾去空白 → `join("\n")`（append-only
  下旧行号稳定，选区不会因内容增长错位；拒绝常驻全文行缓存——内存翻倍）。
- 文档同步（`Docs/tui-usage.md` 已知差异条目改写为已落地、`Docs/roadmap.md`
  C2 销账）；主规格 tui-app 增加 ADDED 需求。

**明确不做**：URL 点击打开（需 `pressedUrl` 悬停基建，链接交互另议）；
`copyOnSelect`/键位配置化（无 settings 体系，硬编码 pi 默认）；键盘选区
（pi 无）；编辑器内选区（`tui.input.copy` 属输入框上下文，另物）；
普通模式委托终端（我们恒为 alt-screen，N/A）。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `tui-app`：新增 ADDED 需求「鼠标选区与选中复制」（拖拽选区与反视频、
  双击/三击粒度、边缘自动滚动、释放即复制、优先级与清除、提取文本）；
  既有「工具卡片点击展开」「thinking 块单块展开」「消息区滚动条」的点击/
  消费语义不变（选区为最后兜底，不改其优先级），无需修改。

## Impact

- `src/app/mod.rs`：选区状态组（anchor/focus/granularity/lastClick/拖拽
  与边缘滚动状态）、Down/Moved/Up 分发扩展（背景起选/扩展/完成+复制）、
  复制与 `Copied!` 提示（复用 `osc52_sequence`/`sinks.copy_text`/`notice`）、
  会话重建清除。
- `src/app/view.rs`：`draw` 闭包内对 `frame.buffer_mut()` 做选区
  fg↔bg 翻转（宽字符双格一并处理）。
- `src/app/chat.rs` / `wrap.rs`：**零签名改动**（提取 = 复制时按现状
  签名重算）；`event.rs` **零改动**（三类 pointer 事件已转发）。
- 测试 + PTY；Docs×2。不改 RPC、session 层。
