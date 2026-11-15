# Tasks

## 1. 高亮入口（markdown.rs）

- [x] 1.1 新增 `pub(crate) fn highlight_by_path(path: &str, body: &str)
      -> Option<Vec<Line<'static>>>`：扩展名 → syntect
      `find_syntax_by_extension`；命中走既有 `highlight_cached` 链（tab 归一/
      哈希缓存/theme 单例自动继承）且 span **仅取 fg**（丢 syntect bg，
      design D3——核对 `highlight_uncached` 现有 Style 转换，已不含 bg 则
      复用同一函数）；未命中返回 `None`（design D1）。验证：
      `cargo test markdown::`——单测 `highlight_by_path_hits_extension_and
      _returns_none_for_unknown`（`.py` 产出含非默认 fg 的行、`.xyzzy`/
      无扩展 → None、fg-only 无 bg 携带）。
- [x] 1.2 降级等价性：单测 `unknown_extension_falls_back_byte_identical`
      ——`None` 分支交由调用方走现状路径，断言现状 read 纯文本输出在
      高亮开关前后逐字节一致（以既有 read 卡 fixture 对照）。验证：
      `cargo test` 全绿。

## 2. 卡片接线（tools.rs）

- [x] 2.1 `read_body`：`_args` 转正取 `path`，按态喂料（折叠=头 10 行
      切片、展开=全文，design D2），`highlight_by_path` 命中替换行、
      `None` 走现状原路径。验证：`cargo test tools::`——新单测
      `read_card_highlights_collapsed_and_expanded_with_panel_bg`
      （`.py` 折叠 10 行与展开全文均有着色、同 token 两态 fg 一致、
      高亮行 bg 仍为面板色；未知扩展与现状逐字节一致）。
- [x] 2.2 write 卡渲染接同一入口（`args.path`/`filePath` 双键，design D4）。
      验证：`cargo test`——新单测 `write_card_highlights_by_extension`
      （`.rs` 内容着色、未知扩展降级、面板 bg 保留；edit 卡 diff 行红绿青
      回归不变——既有 `edit_card_from_fixture_shows_colored_diff` 保持绿）。

## 3. 文档

- [x] 3.1 `Docs/tui-usage.md` 工具卡说明补"read/write 内容按语言着色
      （与代码块同主题）"；`Docs/roadmap.md` 工具卡片挂账项 ② 销账
      （四项→三项，注明 edit 走 renderDiff 行内变更高亮、属另一语义不在
      本项）。验证：grep 两处均命中。

## 4. 验证

- [x] 4.1 PTY 手验（`/tmp/pty_card_highlight_check.py`，假 pi 注入 read
      `.py` 结果 + `.xyzzy` 降级 + write `.rs` 卡）：`.py` 卡片可见非默认
      前景色 cell 且面板底色保留；`.xyzzy` 卡纯文本无花色；write 卡着色；
      展开（ctrl+o）后着色仍在。验证：脚本 `N/N steps passed`（pyte
      fg/bg 断言）。
- [x] 4.2 全量门禁：`cargo test` 全绿、`cargo clippy --all-targets` 零告警、
      `openspec validate add-tool-card-syntax-highlighting --strict` 通过。
      验证：三条命令输出（test result ok / 0 warnings / Valid）。

## Workflow follow-up

- 归档：`/opsx-archive add-tool-card-syntax-highlighting`（tui-app ADDED 同步：40→41 需求）
- 提交两批：`feat(tui)`（代码 + Docs）→ `docs(specs)`（归档 + 主规格），按需 push
