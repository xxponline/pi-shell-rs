# Design — 树内搜索

## Context

树浮层现状（`Overlay::Tree` + `src/app/tree.rs`）：`walk()` 生成缩进行，`passes_filter()` 执行 5 种过滤模式，折叠集合做子树裁剪；无任何查询状态。pi 的 `TreeSelector`（`interactive/components/tree-selector.ts`）以 `searchQuery` 字符串实现即时搜索：可打印字符追加、Backspace 删、Esc 先清查询；`applyFilter()` 先过滤模式、再 `searchTokens.every(token => text.includes(token))`（空白分词 AND、小写子串）。可搜索文本由 `getSearchableText()` 按 entry 类型拼接（label、role/content、bash 命令、session_info name、model/thinking 变更等）。

约束：零上游依赖（数据全部来自已渲染的 `get_tree` 节点 JSON）；不加 RPC；纯 TUI 侧。

## Goals / Non-Goals

- **Goals**：键序/匹配语义/叠加顺序对齐 pi；搜索状态、命中计数、零命中提示可见且不丢选择
- **Non-Goals**：标签编辑（差异 #14 排除）；正则/模糊打分排序（pi 是子串 AND，不引入打分）；跨会话搜索（只搜当前树数据）

## Decisions

- **D1 查询状态放 `Overlay::Tree` 新字段 `search: String`**（打开时清空）。替代：独立 overlay 层——不必要，树浮层是唯一消费方。
- **D2 纯逻辑全部下沉 `tree.rs`**：`searchable_text(&Value) -> String`（对照 pi `getSearchableText` 各分支，按我们节点实际字段裁剪：会话名、首条消息预览、bash 命令、模型/thinking 变更、标签）；`walk()` 增加 `search` 参数，在 `passes_filter` 之后逐节点做分词 AND 匹配。**逐节点过滤（非子树隐藏）**：祖先不命中但后代命中时保留后代行（缩进层级不变）——与 pi 的 flat 过滤一致；折叠裁剪仍在最后。
- **D3 键序（`overlay_key` Tree 臂）**：无修饰的可打印字符（char ≥ 0x20、非 Enter/Tab）→ 追加查询并**保持当前选择**（可见集变化后按索引钳制）；Backspace（`\x7f`）→ 删末字符；**Esc：查询非空先清空（面板/选择不动），已空才关闭**；↑↓/ctrl+o/折叠/回车语义不变。注意排除 alt/ctrl 修饰键与斜杠命令键。
- **D4 状态行**：树标题区展示 `过滤模式 · search:"query" (n)`；零命中显示 `0 matches — backspace 清除` 样式提示（复用现有 notice 黄色风格）。行渲染在 view.rs 树浮层分支。
- **D5 选择保持**：查询变化后若原选择仍可见则保持，否则钳到最近有效索引（pi `lastSelectedId` 的简化）；空结果不 panic。

## Risks / Trade-offs

- 可搜索字段覆盖面受我们节点数据限制（我们只有 entry JSON——与 pi 同源，风险低）；遗漏字段在任务里对照 pi 分支清单核验
- 大树逐行重算 searchable_text：树规模小（会话级 JSON），可忽略；如需缓存后续再做
