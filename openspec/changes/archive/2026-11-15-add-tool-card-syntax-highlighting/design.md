# Design

## Context

动机与挂账表述修正见 proposal.md。实测事实：

- pi：`read.ts:125 highlightCode(replaceTabs(output), lang)`（lang 来自
  `getLanguageFromPath`）、`write.ts:41/64` 行级+全文；`highlightCode` 在
  `theme.ts:935`（highlight.js 引擎，带共享缓存变体）。edit 卡片走
  `renderDiff`（`diff.ts:82` 行内**变更**高亮）——非 highlightCode。
- 我们：`markdown.rs` syntect 管线完备——`syntax_set()`/`theme()` 进程单例、
  `highlight_cached(lang, body)`（内容哈希缓存）、`highlight_uncached`
  （tab→4 空格归一 = pi replaceTabs 同义、任何失败降级纯文本）。
- 卡片路径：`tools.rs` `read_body(_args, tool)`（args 现未用——`path` 键
  可用）、write 卡 `args.path`/`filePath`；`tool_lines(name, args_json,
  tool, expanded)` 签名稳定（`chat.rs` 调用，零改动面）。
- 样式分层：`chat.rs` 给工具行设 `line.style = bg`（三态面板色），span 级
  样式叠加其上——高亮 span 若携带 syntect 主题 bg 会污染面板色。

## Goals / Non-Goals

**Goals:**

- read/write 内容按扩展名着色，与聊天区代码块**同主题同引擎**（单一
  syntect theme，全 TUI 一种高亮风格）。
- 降级零回归：语言未知/高亮失败 → 与现状逐字节一致的纯文本。
- 面板底色保留（fg-only 分层）；折叠/展开着色一致。

**Non-Goals:**

- edit diff 行内变更高亮（pi renderDiff 独立语义，单列待议）；
  bash/grep/find/ls 输出高亮（pi 未用 highlightCode）；
  高亮主题可配置化（无 settings 体系）；像素级 pi 色值对齐（roadmap
  明确不追求像素级复刻——规格要求的是"与我们代码块风格一致"）。

## Decisions

### D1 入口 = `markdown::highlight_by_path(path, body) -> Option<Vec<Line>>`

`pub(crate)`，扩展名 → syntect `find_syntax_by_extension`：

- 命中语法 → 走既有 `highlight_cached` 链（tab 归一、哈希缓存、theme 单例
  全部自动继承），产出**仅取 fg**（含 italic/bold 字形属性、丢 bg）的行。
- 未命中（无扩展名/无语法）→ 返回 `None`，**调用方原样走现状纯文本路径**
  ——降级 = 现状行为逐字节一致（比"入口内部降级"更强的回归保证）。

- **替代：入口内部永远返回 Vec（内部降级）**：否——降级路径的行样式若与
  现状有任何差异都是静默回归；`None` 让现状代码成为降级本身。

### D2 高亮喂料 = 全文（实施修正：pi 同款管线顺序）

**实施修正（apply 期冲突 + pi 源码实证）**：原案“折叠态只喂头 10 行
切片”与折叠提示行冲突——`collapse` 的隐藏行数 = `body.len() − keep`，
切片后 `body.len() == keep` → 提示行消失，违反本 change spec 的
“内容、行数与折叠行为不变”。pi `read.ts formatReadResult` 的实际顺序：

```
highlightCode(全文) → trimTrailingEmptyLines → slice(0, expanded ? len : 10)
                    → remaining = lines.length − maxLines（计数取全文）
```

即**先高亮全文、再切片、提示计数取全文**——pi 从不在管线中间切片，
因此没有该冲突。且 read 结果本身受 pi read 工具字节上限截断（卡片中的
`[Truncated...]` 警告即其产物），全文 key 的每帧 clone+哈希**有界**，
量级 ≈ markdown 代码块既有先例（10KB×100 帧 <2s 实测）。

- **A 案落地**：`read_body` 全文喂 `highlight_by_path`，collapse/提示行
  **零改动**；`(lang, body 哈希)` 缓存命中跳过高亮；折叠/展开共用同一
  key（同 lang 同 theme → 规格“着色一致”天然成立）。
- **已否决替代（原折叠切片案）**：需 `read_body` 回传总行数 +
  `collapse` 增 total 参数——结构改动换 ~百字节 key，在截断有界前提下
  收益不足，且 pi 未这么做（逐行同构优先）。

### D3 fg-only 分层（面板底色保留）

入口产出的 span style 只带前景色与字形属性（italic/bold），**丢弃 syntect
主题 bg**——叠加在 `chat.rs` 设置的 `line.style`（三态面板色）之上时底色
完整。与滚动条末列"清 bg 让位"同属一类分层规则（fg 属内容、bg 属容器）。

- 若 `highlight_uncached` 现有 Style 转换已不含 bg，则复用同一转换函数
  （自动与 markdown 代码块完全一致）；含 bg 则在入口剥离——tasks 阶段
  核对转换函数后二选一，两种情形规格行为相同。

### D4 调用点（`tools.rs`）

- `read_body`：`_args` 转正取 `path` → `highlight_by_path(path, 展开?全文
  :预览切片)`，`None` 走现状。
- write 卡渲染：同上（`path`/`filePath`）。
- 其余卡片（bash/grep/find/ls/edit）**不接**（Non-Goals）。
- `tool_lines` 签名、`chat.rs`、`view.rs`、`event.rs` 零改动。

## Risks / Trade-offs

- [面板底色被高亮 bg 污染] → D3 fg-only + 单测断言高亮行 cell 的 bg =
  面板色 + PTY 肉眼验证。
- [降级路径静默回归] → `None` 走现状原路径（D1）+ 单测逐字节对比
  降级输出与现状输出。
- [大文件展开态性能] → 折叠切片（D2）+ 既有哈希缓存；PTY 用小 fixture，
  性能沿 markdown 先例不另测。
- [syntect 与 pi highlight.js 色值不同] → 非目标（roadmap 像素级复刻
  明确不做）；规格锚定"与我们代码块同主题"。

## Migration Plan

纯新增行为，无数据/协议迁移；随归档同步 tui-app（ADDED +1）。回滚 = 还原
本 change 提交（无二进制体积变化——syntect 已在依赖）。

## Open Questions

（无——read/write 的 args 键名与 Style 转换细节在 tasks 阶段按 fixture/
源码核对，不影响方案形态。）
