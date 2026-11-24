# Tasks

## 1. 外壳改造（纯外观，方案 b：白底 + 黑框）

- [x] 1.1 `palette` 常量块调整：增 `INPUT_BORDER`（纯黑，lighting）、
      退役 `INPUT_BG`（输入区改与消息区同底=白）；验证：
      `rg INPUT_BG src/` 无残留、`cargo check --features gpui-support` 零告警
- [x] 1.2 输入容器改渲染：`border_1()`（聚焦 `border_2()`）+
      `INPUT_BORDER` + 去灰底；`Textarea::new(...)` 加 `.appearance(false)`
      （剥 kit 底色/圆角/常态边框/聚焦变色/focus ring）；验证：
      `cargo test --features gpui-support` 全绿 + 开窗肉眼见黑色 1px 直角框、
      无圆角无光环
- [x] 1.3 焦点切换重绘核验：开窗把焦点移入/移出输入区往返数次，边框
      1px↔2px 跟随；若不刷新，补 `cx.on_focus_change(焦点柄, notify)`
      一行；验证：手动往返切换生效 + `builtin_textarea_edits_and_height_cap`
      等既有焦点/编辑测试保持绿

- [x] 1.4 验收反馈间距微调：补全容器 `mb 10px`（输入框整体离底边 10px，
      不贴底）+ 状态行 `mb 10px`（状态行再上拉、与输入区留 10px 消除重叠）；
      验证：开窗目视——输入框不贴底、状态行与输入区无重叠（并入 3.1 一起验）

## 2. 回归与文档

- [x] 2.1 双矩阵回归：`cargo test` 与 `cargo test --features gpui-support`
      全绿（基线 415 / 461），`cargo clippy --all-targets` 两列零告警；
      验证：命令输出全绿
- [x] 2.2 `Docs/tui-usage.md` GPUI 段补一句输入区形态（白底黑框、聚焦加粗、
      无内建装饰）；验证：文档与实现一致

## 3. 手动验收

- [x] 3.1 开窗逐项：常态黑色 1px 直角边框（无圆角/无光环/白底）、聚焦加粗
      2px、打字/换行/撤销/提交正常、补全菜单与浮层外观不受影响、关窗
      进程干净退出；验证：真实窗口逐项勾选，问题回补任务

## Workflow follow-up

- `design.md` **有意跳过**（design 指令为条件型：本变更单模块纯外观、
  无新依赖、无安全/迁移复杂度，技术决策已在 explore 敲定——四条均不适用）。
- 评审通过后 `/opsx-archive simplify-gpui-input-chrome`。
