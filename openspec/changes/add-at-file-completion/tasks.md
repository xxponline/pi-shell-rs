# Tasks — `@文件` 补全

## 1. 纯逻辑（autocomplete.rs）

- [x] 1.1 `extract_at_token`：`@path` / `@"quoted`（未闭合容忍）token 提取，分隔符/行首边界，斜杠 token 不误触发；单测：裸引号/带引号/含空格查询/行中 token/`/cmd` 排除
- [x] 1.2 `list_at_candidates`：目录定位（`~`/绝对/相对/`/` 结尾）→ 前缀匹配 → `scoreEntry` 对等打分 → 排序（分数→深度→长度→字典序）→ top20 去重；fd 输出作注入参数；单测：打分表、目录优先、top20 截断、空/不存在目录静默空
- [x] 1.3 `apply_at_candidate`：保 `@` 前缀与 `~/`、`./`、绝对路径写法；文件尾空格、目录无空格续写、分隔符自动 `@"…"`、光标落位；单测六形态

## 2. fd 生产路径

- [x] 2.1 `fd` 探测与 spawn（300ms 超时、失败/超时/无 fd → None 回落基础候选），输出解析为绝对路径行；单测：解析容错（坏行跳过）、超时降级路径（注入假执行器）

## 3. 菜单接线

- [x] 3.1 候选源接入 completion 菜单状态机（`CandidateKind::AtFile`：打开/重过滤/↑↓ 选中/Tab 应用/Esc 关闭；Enter=应用并按普通提交）；编辑器键序：菜单打开时 ↑↓/Tab/回车/Esc 不落输入框；单测：触发与关闭时机、键序分流、Enter 双段语义
- [x] 3.2 视图：菜单渲染复用现有样式（`@` 候选显示目录 `/` 后缀与路径描述行）；单测：候选行格式

## 4. 真机验收与文档

- [x] 4.1 假 pi PTY 真机：键入 `@` 断言菜单出现 → 续输重过滤 → Tab 应用（文件带空格/目录不带）→ Esc 只关菜单 → 回车应用并提交入流；真实 fd 冒烟（有/无 fd 两态）
- [x] 4.2 文档：`Docs/tui-usage.md` `@` 补全段、`Docs/pi-tui-interaction.md` 差异记录（fd 缺失回落、字面量入上下文 vs pi 附件）
- [x] 4.3 门禁：`cargo test` 全绿、`clippy --all-targets` 零告警、`openspec validate add-at-file-completion --strict`
