# Tasks

## 1. session-state：bash 归约与重建

- [x] 1.1 `Entry::Bash { command, output, exit_code, cancelled, truncated, full_output_path, excluded }` + `bash_running`/`bash_output` 字段（`load_messages` 一并复位）；`bash_execution_update` 从 no-op 臂改为累积 delta；`finish_bash(result, command, excluded)` 落条目并清缓冲；**验证**：单测覆盖累积→落条目、cancelled/excluded 标记、缓冲复位
- [x] 1.2 `load_messages` 解析 `role: bashExecution`（缺字段不 panic、未知 role 维持跳过）；**验证**：单测含 system/user/bashExecution/assistant 快照重建顺序与字段

## 2. App：提交分流与运行控制

- [x] 2.1 `submit_as` 重排为 pi 次序（斜杠 → `!` 分流 → 压缩排队 → 常规发送）：`!cmd`/`!!cmd` 发 `bash` RPC（attach 携带命令与 excluded）、置 `bash_running`、清输入；`!` 空体落常规发送；**验证**：单测断言压缩期间斜杠立即执行（不再入队）、`!cmd` 发 bash 而非 prompt、`!!` 带 excludeFromContext、`!` 照常发 prompt
- [x] 2.2 运行门控与 Esc：bash 运行中再提交 `!cmd` → 黄条警告且**输入保留**；空闲 Esc（含非空输入）→ `abort_bash` 且不碰双击窗口；流式 Esc 仍为中止生成；`AsyncKind::BashDone` 消费结果（含 SendFailed 且 running 的失败清理）；**验证**：单测覆盖三者优先级、保留输入、BashDone 落条目与 running 复位

## 3. 渲染

- [x] 3.1 chat 条目渲染（命令行/输出弱化/exit·cancelled·truncated·excluded 状态行、500 行截断）+ 运行中 live 块（⟳ $ cmd + 累积输出）；**验证**：渲染单测断言各状态文案与截断提示
- [x] 3.2 输入区边框：`trim_start` 以 `!` 起始 → 黄色边框，否则还原；**验证**：渲染单测断言边框色随输入切换

## 4. 文档

- [x] 4.1 `Docs/tui-usage.md` 补 `!`/`!!` 用法、边框提示、Esc 中止；`Docs/pi-tui-interaction.md` 差异 #6 完成；`Docs/slash-commands.md` 未命中边界段补 `!` 语义；**验证**：三文档无残留"#6 待立项"且与实现一致

## 5. 验收与门禁

- [x] 5.1 假 pi PTY：`!cmd` 执行与流式输出、`!!` 标记、运行中二次提交警告且输入保留、Esc 中止、`!` 空体照常发、压缩期间斜杠立即执行；**验证**：脚本断言全 True（ALL OK）
- [x] 5.2 门禁：`cargo test` 全绿、`cargo clippy --all-targets` 零告警、`openspec validate add-bash-command-mode --strict` 通过；**验证**：三命令输出无 error/告警

## Workflow follow-up

- 提交：`git add -A && git commit`
- 归档：`/opsx-archive add-bash-command-mode`（同步 session-state 与 tui-app delta）
