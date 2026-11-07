# Tasks

## 1. 显示列换算实现

- [x] 1.1 在 `src/app/view.rs` 新增 `slice_display(line, skip_w, take_w) ->
      (片段, 实际起始列)`(按显示列跳过/截取,宽字符不切断,跨偏移整字保留,
      中文文档注释三要素) — verify: `cargo build` 通过
- [x] 1.2 输入区渲染改用显示列:`prefix_w` 按 `unicode-width` 求光标前缀宽,
      `col_off` 改显示列,可见行走 `slice_display`,`cx` 用实际起始列换算并
      保留原右边界夹取 — verify: `cargo build` 通过且纯 ASCII 路径数学上
      退化为原式

## 2. 测试

- [x] 2.1 新增 view 单测:输入 `你好😊hahah` 光标行尾 → 断言光标 x 等于
      显示列换算值(12 + 输入区左边距),而非字符序号值(9 + 左边距) — verify:
      `cargo test` 该用例通过
- [x] 2.2 新增 view 单测:全角长行 + 行尾光标触发水平滚动 → 断言光标 x 在
      输入区内且可见文本不含被切断的半个宽字符导致的错位 — verify:
      `cargo test` 该用例通过
- [x] 2.3 回归:既有 view/编辑器全部单测(全 ASCII)保持全绿 — verify:
      `cargo test` 全绿

## 3. 验证与收尾

- [x] 3.1 `cargo clippy --all-targets` 零告警(AGENTS.md 门槛) — verify:
      clippy 输出无 warning
- [x] 3.2 `openspec validate fix-input-cursor-display-column --strict` 通过 —
      verify: 命令输出 valid
- [x] 3.3 下游联证:在 balabala-agent-gui 运行
      `cargo test diagnostic_real_pi -- --ignored --nocapture`,注入
      `你好😊hahah` 后 `cursor_position` 应为 `(row, 12)` — verify: 诊断输出
      光标列 12

## Workflow follow-up

- 实现完成后由维护者走 `/opsx-archive fix-input-cursor-display-column`
  归档并同步主规格(AGENTS.md 约定,不手工改 openspec/specs/)。
