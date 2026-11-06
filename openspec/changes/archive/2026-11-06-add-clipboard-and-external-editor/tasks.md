# Tasks

## 1. 基础原语

- [x] 1.1 `EditorBuffer::insert_text(&str)`：光标处多字符插入（`\n` 走 newline 语义、单次 undo 快照）；**验证**：单测覆盖多行粘贴、光标中段插入、undo 一次回滚整段
- [x] 1.2 `terminal.rs`：`suspend_for_child()` / `resume_from_child()`（kitty Pop/Push、Leave/EnterAlternateScreen、raw 切换）+ 键盘线程 `poll(150ms)` + `Arc<AtomicBool>` 暂停闸门（暂停期不 read，字节留给子进程）；**验证**：单测闸门逻辑（暂停时 poll 到数据不投递、恢复后投递），既有键路 PTY 回归不破

## 2. 外部编辑器

- [x] 2.1 `settings_io::read_external_editor()`（`externalEditor` 键）+ 命令解析（settings → `$VISUAL`/`$EDITOR` → nano/notepad）+ `edit_in_external` 纯函数（写 prompt.md、spawn、exit 0 读回去 BOM/末尾换行、失败返回原因、finally 清理临时目录）；**验证**：单测用假编辑器脚本（exit 0 改写 / exit 1 / 命令不存在）三分支 + 命令解析优先级
- [x] 2.2 `KeyIntent::ExternalEditor` → `UiEvent::OpenExternalEditor` → 事件循环臂：置键盘闸门 → 挂起 → 阻塞跑编辑器 → 恢复 → `terminal.clear()` 全量重绘；成功 `insert`/替换输入、失败黄条保持原文；**验证**：单测意图分发与事件回填，PTY 用假编辑器脚本验证真挂起/恢复/回填

## 3. 剪贴板

- [x] 3.1 新 `src/app/clipboard.rs`：`read_image()`（macOS JXA base64 / Linux wl-paste→xclip / Windows PowerShell，3s 超时）与 `read_text()`（pbpaste / wl-paste→xclip→xsel / PowerShell，2s 超时），失败返回 None；**验证**：单测用 PATH 注入的假后端命令验证探测顺序、超时 kill、坏输出解析失败回退
- [x] 3.2 `KeyIntent::CopyMessage` → 复用 `cmd_copy`；`KeyIntent::PasteClipboard` → 图片链（字节→临时文件→`insert_text` 路径）→ 文本链（`insert_text`）→ 空则无操作（无红条）；**验证**：单测三分支（图片插入路径、文本插入、全空无操作）+ 空剪贴板不产生通知

## 4. 键位与文档

- [x] 4.1 `key_intent` 三臂接线（ctrl+g/x/v，无修饰冲突）+ `hotkeys_rows()` 补三行；**验证**：单测三键意图映射 + 键位表含 ctrl+g/x/v
- [x] 4.2 `Docs/tui-usage.md`（三键用法、外部编辑器命令解析、粘贴平台后端）与 `Docs/pi-tui-interaction.md` 差异 #9 完成（注明"复制选区"与"文件列表粘贴"两项差异）；**验证**：文档与实现一致、无残留"#9 待立项"

## 5. 验收与门禁

- [x] 5.1 假 pi PTY：ctrl+g 用假编辑器脚本完成挂起→改写→恢复→输入回填；ctrl+x 复制最后回复（OSC52 落屏）；ctrl+v 文本粘贴（假 pbpaste）插入光标处；**验证**：脚本断言全 True（ALL OK）
- [x] 5.2 门禁：`cargo test` 全绿、`cargo clippy --all-targets` 零告警、`openspec validate add-clipboard-and-external-editor --strict` 通过；**验证**：三命令输出无 error/告警

## Workflow follow-up

- 提交：`git add -A && git commit`
- 归档：`/opsx-archive add-clipboard-and-external-editor`（同步 tui-app delta）
