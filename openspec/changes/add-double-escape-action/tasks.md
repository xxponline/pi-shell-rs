# Tasks

## 1. 设置读写助手

- [x] 1.1 新建 `src/app/settings_io.rs`：`DoubleEscapeAction`（tree/fork/none）+ `read_from(path)`（缺失/非法→tree）+ `write_from(path, action)`（可解析才写、tmp+rename 原子、保留其它键）+ 默认路径包装；**验证**：单测 roundtrip（写→读回→改键后其它键保留）、非法值回落 tree、损坏文件写入被拒且返回错误
- [x] 1.2 `mod.rs` 注册模块并接通 `/settings` 打开时的现读（供浮层初值）；**验证**：`cargo build` 通过 + 打开浮层初值单测（临时路径注入）

## 2. 双击判定接线

- [x] 2.1 `App` 增加 `last_escape: Option<Instant>` 字段；`handle_key` 在浮层/补全分支之后拦截"空闲 + Ignore + Esc"走 `handle_double_escape()`：仅输入为空时更新时间戳，500ms 内第二次按 `read_double_escape_action()` 触发（tree→开树、fork→开树、none→不动作），触发后清时间戳；**验证**：单测覆盖三态动作、50ms 双击触发、600ms 超时不触发、输入非空不更新窗口、触发后时间戳复位
- [x] 2.2 优先级单测：流式 Esc 仍走 Abort 且**不**更新窗口；浮层 Esc 关浮层、补全菜单 Esc 关菜单均不进入双击判定（连按不误开树）；**验证**：`cargo test --bin pi-rust-tui escape` 断言浮层/菜单状态与时间戳不变

## 3. settings 浮层 doubleEsc 行

- [x] 3.1 `Overlay::Settings` 增加 `double_esc: Option<String>` 字段：打开时现读文件填充、可切换行 0..2（Down 夹取）、渲染 `doubleEsc  tree/fork/none` 行；**验证**：渲染单测含该行与夹取行为
- [x] 3.2 `apply_settings` idx=2：循环 `tree→fork→none` 写回文件；写失败红条且行值回读；**验证**：单测临时路径下三轮循环写读一致、损坏文件场景红条且值不假更新

## 4. 文档

- [x] 4.1 `Docs/slash-commands.md`：settings 命令行补 doubleEsc 行为与"仅读全局 settings.json（项目级覆盖未实现）"差异；**验证**：文档与实现一致
- [x] 4.2 `Docs/tui-usage.md` 键位表补"双击 esc（空输入）开树"；`Docs/pi-tui-interaction.md` 差异 #5 标记完成并更新尾部待办；**验证**：两文档无残留"#5 待立项"字样

## 5. 验收与门禁

- [x] 5.1 假 pi PTY：空输入双击 esc 打开会话树、间隔>500ms 不开、settings 浮层三轮循环后文件值变化且双击行为跟随新值；**验证**：脚本断言全 True（ALL OK）
- [x] 5.2 门禁：`cargo test` 全绿、`cargo clippy --all-targets` 零告警、`openspec validate add-double-escape-action --strict` 通过；**验证**：三命令输出无 error/告警

## Workflow follow-up

- 提交：`git add -A && git commit`
- 归档：`/opsx-archive add-double-escape-action`（同步 tui-app delta）
