# Proposal

## Why

工具卡片对齐挂账项 ②：read/write 卡片的文件内容目前是纯文本，pi 按文件
语言语法着色（`read.ts:125 highlightCode(replaceTabs(output), lang)`、
`write.ts:41/64` 行级与全文）。我们侧引擎**已经在跑**——markdown 管线的
syntect（进程内 `SyntaxSet`/theme 单例、tab→4 空格归一、内容哈希缓存
10KB×100 帧 <2s 实测）——本 change 只差把入口接到卡片渲染段，且与聊天区
代码块**天然同主题**。

**挂账表述修正**（探查所得）：edit 卡片走 pi 的 `renderDiff`（行内**变更**
高亮，`diff.ts:82`），**不是** highlightCode——diff 行内变更是另一语义，
不含在本 change（我们已有行级红绿/青着色，保持现状）。

## What Changes

- **read 卡片**：文件内容按扩展名 syntect 高亮——折叠预览（头 10 行）与
  展开全文一致着色；路径从 `args.path` 取。
- **write 卡片**：新文件内容同样按扩展名着色（`args.path`/`filePath`）。
- **高亮入口**：新增 `markdown` 模块的按路径入口（复用既有
  `highlight_cached`/`highlight_uncached`：tab 归一、哈希缓存、失败降级
  纯文本全部自动继承）；语言未知（扩展名无语法）→ 纯文本降级，内容与
  行数不变。
- **样式约束**：高亮 span 仅取前景色——工具卡三态**背景面板底色保留**
  （与滚动条末列清 bg 同类分层规则）。
- **明确不做**：edit 卡片 diff 行内变更高亮（pi 走 renderDiff 独立组件，
  属 diff 语义增强）；bash/grep/find/ls 输出高亮（pi 未用 highlightCode）；
  语法高亮可配置化/主题切换（无 settings 体系）。
- 文档同步（`Docs/tui-usage.md`、`Docs/roadmap.md` 挂账项 ② 销账）；
  主规格 tui-app 增加 ADDED 需求。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `tui-app`：新增 ADDED 需求「工具卡片语法高亮」（read/write 按语言着色、
  未知语言降级、折叠/展开一致、面板底色保留）；既有「工具调用卡片」与
  write/grep/find/ls 专用卡片需求的结构/状态/折叠语义不变，无需修改。

## Impact

- `src/app/markdown.rs`：`highlight_cached` 系增加按路径公开入口
  （`pub(crate)`，扩展名→语法查找 + 降级）。
- `src/app/tools.rs`：`read_body`（`_args` 转正取 path）与 write 卡渲染接
  入口；折叠态喂预览切片（小 body 走缓存）、展开态喂全文。
- `src/app/chat.rs`、`src/app/view.rs`、`event.rs`：**零改动**
  （`tool_lines` 签名不变）。
- 测试 + PTY（着色可见性/降级/底色保留）；Docs×2。
- 不改 RPC、session 层；二进制体积不变（syntect 已在依赖）。
