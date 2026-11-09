# Tasks

## 1. 纯逻辑层：token 提取与候选（autocomplete.rs）

- [x] 1.1 新增 `extract_path_token(text_before, force)`：边界集复用
  `is_token_boundary`（空白/`"`/`'`/`=`）+ 未闭合引号形态 + `stripLeadingWrappers`
  解 `([{<\``；force 语义下空 token 返回 `Some("")`，非 force 空 token 返回
  `None`。验证：单测覆盖 `""`、`"look "`、`"src/mai"`、`"~/Do"`、`"a=b"`、
  未闭合 `"my di`、`"(src/x"` 各形态（`cargo test extract_path_token` 全绿）。
- [x] 1.2 从 `list_at_candidates` 抽出基础目录段为
  `list_dir_candidates(query, base_dir) -> Vec<AtCandidate>`（read_dir +
  大小写不敏感前缀，无 fd、无截断、无打分），`list_at_candidates` 改为调用它
  后叠加 fd/打分/截断。验证：`cargo test` 中既有 `@` 候选单测全部不改断言即
  通过（行为不变），新增 `list_dir_candidates` 排序单测（目录优先→字母序、
  不截断 20 条）。
- [x] 1.3 新增 `CompletionKind::Path` 与 apply 规则：无 `@`、无尾随空格、目录
  保 `/`、含分隔符或空白包 `"`、`~/`·`./`·绝对路径保留、光标落插入文本后。
  验证：单测覆盖文件/目录/带空白引号/`~` 前缀四类 apply 结果与光标位
  （`cargo test apply_completion`）。

## 2. force 态与接线（mod.rs + autocomplete.rs）

- [x] 2.1 `CompletionState` 增加 `force: bool` 字段，`close()` 复位；新增
  `open_path_completion(force, explicit_tab)` 返回
  `enum PathOpen { Menu(CompletionState), Applied, None }`（单条自动应用内联
  判断在接线层）。验证：单测断言恰 1 条 → `Applied` 且值已按 Path 规则拼好、
  0 条/多条 → `None`/`Menu`。
- [x] 2.2 `key_intent` 新增 `KeyIntent::ForcePath`（`KeyCode::Tab` 且菜单未开、
  modifiers 为 NONE），`handle_key` 在菜单消费块之后拦截：斜杠上下文（行首 `/`
  其后无空白）不触发、否则调 `open_path_completion(true, true)` 后 `return`。
  验证：单测 `key_intent_mapping_matches_spec` 增补 Tab 两分支断言 +
  `handle_key` 端到端（空白+Tab 开菜单、`/mo`+Tab 不开、`src/mai` 单条直接插入
  且无 prompt 发出）。
- [x] 2.3 `refresh_completion` 接 force 段：菜单已开且 `force` 时按
  `extract_path_token(force=true)` 重算 Path 候选（零候选关、否则保持/刷新）；
  非 force 上下文不算 Path（普通打字出现 `src/` 不弹菜单）。验证：单测
  force 菜单下 输入 `src/` 刷新、退格至空白退回 cwd 列表不关、指向不存在目录
  静默关三场景。
- [x] 2.4 Enter 按 kind 分流（P1）：`Command | Argument` 应用后继续提交（现状），
  `AtFile | Path` 应用后 `return`（只应用不提交）。验证：单测两组——斜杠回车
  发出 prompt、`@`/Path 回车不发 prompt 且输入框保留文本。

## 3. 渲染（view.rs）

- [x] 3.1 主列改渲染 `item.label`（空回落 `item.value`），label 按 15 列
  truncate 后再补 padding，描述列逻辑不变。验证：改写
  `at_file_completion_renders_dir_suffix_and_path_desc` 断言主列为
  `src/`·`main.rs`（非 `@` 值）、描述列仍为路径；新增 Path 菜单渲染测试
  （无描述列 + `(i/n)` 计数）；斜杠渲染既有测试不改断言即过。
- [x] 3.2 `view.rs` 补 force 菜单高度与滚动窗口测试（超 5 条 `(i/n)`、选中项
  贴底上移）。验证：`cargo test view` 全绿。

## 4. 规格与文档同步

- [x] 4.1 `Docs/tui-usage.md`：补全菜单章节新增 Path 补全小节（Tab 触发、
  force 下钻、回车只应用不提交、readdir 无 fd），注销"tab 补全是后续独立变更"。
  验证：文档描述与 `path-completion` spec 场景逐条对应，无遗留旧说法。
- [x] 4.2 `Docs/pi-tui-interaction.md` 差异清单补行（纯路径 Tab 补全已对齐，
  Enter 语义已修正），`Docs/roadmap.md` 若有对应欠账行同步。验证：grep
  "tab 补全" 仅剩已对齐表述。

## 5. 集成验证

- [x] 5.1 `cargo test` 全绿 + `cargo clippy --all-targets` 零告警。
  验证：两命令输出无 error/warning。
- [x] 5.2 `openspec validate add-path-tab-completion --strict` 通过。
  验证：命令退出码 0。
- [x] 5.3 PTY 手验（参考 `Docs/tui-usage.md`）：空白+Tab 列 cwd → 输入 `src/`
  下钻 → 选目录应用继续输入 → 单条候选直接插入 → `@` 菜单回车不提交、再回车
  提交 → 斜杠菜单回车提交 → Esc 关菜单。验证：七步行为与 spec 场景一致。

## Workflow follow-up

- 实现完成并验收后运行 `/opsx-archive` 归档（同步主规格
  `path-completion` 新建、`file-completion`/`tui-app` 修订）。
