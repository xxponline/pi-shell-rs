# Design

## Context

动机与范围见 proposal.md。与实现现状相关的约束：

- 补全由三层构成：`view.rs` 渲染 `CompletionState`、`mod.rs` 的
  `refresh_completion`（每次按键后按光标前文本重算，`None` 即关）、
  `autocomplete.rs` 纯逻辑（token 提取、候选、apply，无 TTY 可单测）。
- 现有 `refresh_completion` 是**无状态重算**：它不知道菜单"为什么开着"，天然
  表达不了 force 态（退格到空白要保持打开、退回 cwd 列表）。
- `key_intent`（`mod.rs:2156`）无 `Tab` 臂，菜单未开时 Tab 落入 `Ignore`；
  菜单内 Tab 在 `handle_key` 的消费块（`mod.rs:1656`）已处理，两处互不重叠。
- pi 侧行为已实测定案（源码 `pi-tui/dist/components/editor.js:1902`
  `handleTabCompletion`、`chunk-4EYXVSA5.js` 的 `extractPathPrefix(force)` /
  `getFileSuggestions` / `applyCompletion`）：force 无条件提取 token、readdir
  候选无上限、恰 1 条直接应用、非 `/` 前缀回车只应用不提交。

## Goals / Non-Goals

**Goals:**

- Tab 触发的纯路径补全全链路落地（触发/候选/force 生命周期/单条自动应用/apply）。
- `@` 与 Path 菜单回车语义对齐 pi（只应用不提交）；主列渲染对齐 pi（label）。
- 新逻辑全部落在 `autocomplete.rs` 纯函数 + `mod.rs` 薄接线，无 TTY 单测覆盖。

**Non-Goals:**

- token 边界补 CJK 标点、解 `([{<\`` 包裹（`@` 补全同样受影响，挂账）。
- 斜杠菜单触发加行首（`cursorLine==0`）限制（挂账）。
- Path 候选接入 `fd` 模糊（pi 明确是 readdir-only，接入反而偏离）。
- 历史键（pi 默认未绑定）、扩展注入触发字符。

## Decisions

### D1：`CompletionState` 增加 `force: bool`

菜单"为什么开着"是关闭语义的必要状态：force 态下退格到空白不关（退回 cwd
列表）、零候选才关；非 force 态维持现状（token 消失即关）。

备选（否决）：把"最近一次由 Tab 打开"存成 App 级变量——与菜单状态分离，Esc 关
菜单后残留，下次自动触发会误判为 force。

### D2：`refresh_completion` 按 kind 优先级三段分流

```
text_before ─► ① 斜杠 suggestions()      （现状，行首 /）
            ─► ② @   at_suggestions()    （现状）
            ─► ③ Path：仅当 菜单已开且 force，
                  或（菜单未开且 key_intent 判定 Tab 触发）时计算
```

Path 段**只在 force 上下文计算**——否则普通打字出现 `src/` 就会弹菜单，而 pi
的纯路径补全是 Tab 专属（自动触发字符只有 `@`/`#`）。Tab 触发本身不走
`refresh_completion` 的常规路径：`key_intent` 新增 `KeyIntent::ForcePath`，
在 `handle_key` 消费块之后、`apply_intent` 之前拦截，调用一次性
`open_path_completion(force=true, explicit_tab=true)`，然后 `return`（跳过末尾
重算，与现有菜单消费键同构）。

备选（否决）：让 `refresh_completion` 从"最近按键是不是 Tab"推断——把键位语义
塞进重算函数，无法表达"Tab + 恰 1 条直接应用"这类一次性行为。

### D3：token 提取复用/扩写 `extract_at_token` 的边界，新增 `extract_path_token`

边界集与 pi `PATH_DELIMITERS` 一致（空白、`\t`、`"`、`'`、`=`）+ 未闭合引号
形态 + `stripLeadingWrappers` 解 `([{<\``。与 `extract_at_token` 共享
`is_token_boundary`，但**不共享函数**：Path token 不要求 `@` 起始，且要返回
`force` 语义下的空串（`extract_at_token` 空 token 返回 `None`）。

挂账项（CJK 标点边界）本次不进，两函数共享边界集，日后一处改两处生效。

### D4：候选 = 现有 `list_at_candidates` 的基础目录段抽出复用，fd 段不走

`list_at_candidates` 的 ① 基础目录候选段（read_dir + 前缀匹配）与 Path 需求
一致；抽出为 `list_dir_candidates(query, base_dir)` 供两者共用，Path 调用时
**不传 fd 输出**且**不截断 20 条**（截断与打分是 `@` 专属，包在 `@` 的调用侧）。
排序在 Path 侧固定为 目录优先→字母序（现有排序是 分数→深度→长度→字典序，
不适用于无分数场景）。

### D5：单条自动应用在 Tab 触发路径内联判断

`open_path_completion` 返回 `enum PathOpen { Menu(CompletionState), Applied }`：
force + explicit_tab + 恰 1 条 → 走 `apply_completion` 直接改编辑器并返回
`Applied`（跳过开菜单）。放在接线层而非纯逻辑层，避免纯函数产生编辑器副作用。

### D6：Enter 按 kind 分流（P1）

`handle_key` 的 Enter 消费块：`CompletionKind::Command | Argument(_)` 应用后
**不 return**（继续落到 `Submit`，现状）；`AtFile | Path` 应用后 **return**
（只应用不提交）。与 pi `applyCompletion` 后 `prefix.startsWith("/")` 的
fall-through 判断逐字对应。

### D7：主列渲染改 label（R1）

`view.rs:316` `item.value` → `item.label`，空 label 回落 `item.value`
（对齐 pi `getDisplayValue`）。`name_w = 15` 改为按本帧可见项 label 宽度
truncate 到 15，防止长文件名挤掉描述列（描述列已有 `saturating_sub` 兜底，
truncate 是收紧行为而非崩溃修复）。

### D8：规格落点

新能力 `path-completion`（与 `slash-autocomplete`、`file-completion` 同级，
按触发源分能力）；`file-completion` 走 REMOVED+ADDED（回车语义反转 + openspec
不支持场景改名，需求"菜单应用与键位"以新名"菜单键位与应用"重立）；`tui-app`
走 MODIFIED（`spec.md:85` 的 tab 表述）。菜单键位约定仍以 `slash-autocomplete`
为基准被引用，不复制文本。

## Risks / Trade-offs

- [force 态与 overlay 互斥] overlay 打开时 `refresh_completion` 强制关菜单（现状），
  force 标记一并清零——`CompletionState::close()` 复位整个结构，天然覆盖。
- [Enter 语义变化影响肌肉记忆] `@` 菜单回车不再发送，老用户需多按一次回车；
  这是 P1 明确接受的对齐代价，spec 场景与 `tui-usage.md` 同步写清。
- [label 渲染改变 @ 菜单外观] 主列从 `@main.rs` 变 `main.rs`；测试与文档断言
  同步更新，描述列仍显示完整路径，信息不丢失。
- [readdir 无上限] 超大目录（如 `node_modules` 顶层）会生成数千候选；菜单可见
  区恒 5 条 + `(i/n)` 滚动，渲染开销线性可接受；pi 同样无上限，截断反而偏离。
- [PTY 验证] force 下钻/单条自动应用/回车不提交需真实终端手验（纯逻辑单测覆盖
  提取/候选/apply，渲染与键位走 PTY，参考 `Docs/tui-usage.md`）。

## Open Questions

- 无。范围（A）、回车（P1）、渲染（R1）、规格落点均已在探索中定案。
