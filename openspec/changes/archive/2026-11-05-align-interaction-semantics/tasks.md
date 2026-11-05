# Tasks

## 1. EditorBuffer 核心与输入链路迁移（语义不变的重构）

- [x] 1.1 新建 `src/app/editor.rs`：`EditorBuffer`（lines/row/col/undo/kill_ring/yank_pos）+ `insert/newline/backspace/text/replace/append_block/is_empty`；验证纯函数单测（插入、行首行尾边界、空缓冲、CJK 字符安全、text 拼接）
- [x] 1.2 `App.input: String` 迁移为 `App.editor: EditorBuffer`，接入所有消费点（submit/can_submit/restore_queued/set_editor_text/view）；验证编译 + 既有测试中直用 `input` 字段处全部适配
- [x] 1.3 全量回归：`cargo test` 中既有 112 项（含 App 测试适配后）全部通过；`cargo clippy` 无新增告警；验证两项命令输出

## 2. 光标移动族

- [x] 2.1 `EditorBuffer` 移动操作：left/right（行首行尾跨界）、up/down（目标行 clamp col）、home/end；验证单测（四边界 × 跨行 × 空行目标）
- [x] 2.2 词级移动 word_left/word_right（空白分词 + 标点边界，alt+←→/ctrl+… 语义）；验证单测（连续词、连续空白、行边界回绕到上/下一行首尾）
- [x] 2.3 key_intent/apply_intent 接线全部移动键（←→/↑↓/home/end + ctrl+b/f/a/e + alt+←→）；验证键位映射单测（含普通字符键不受影响）

## 3. 删除族

- [x] 3.1 `delete_forward`（光标后字符；行尾合并下一行）与 word 级删除 `kill_word_back`（ctrl+w/alt+backspace）、`kill_word_forward`（alt+d）；验证单测（行尾合并、行首回上行、空行、CJK）
- [x] 3.2 行级删除 `kill_to_line_start`（ctrl+u）、`kill_to_line_end`（ctrl+k，含行尾合并）；验证单测
- [x] 3.3 接线 delete/backspace/ctrl+w/alt+backspace/alt+d/ctrl+u/ctrl+k 到 key_intent 分发；验证键位单测（ctrl+d 非空→DeleteForward 分支预备，见组 6）

## 4. kill-ring 与 undo

- [x] 4.1 kill 操作入环（ctrl+k/u/w/alt+d 的删除文本 push，cap 32）+ `yank`（ctrl+y 粘贴环顶）+ `yank_pop`（alt+y 回退游标）；验证单测（kill→yank 还原、连续 kill→yank→yank-pop 替换、环空 yank 无操作）
- [x] 4.2 undo：编辑操作前压快照（cap 100）、`undo()` 回退（含多行/光标位置）、越界无变化不 panic；验证单测（多步回退、cap、跨行编辑回退）
- [x] 4.3 接线 ctrl+y/alt+y/ctrl+-；验证键位单测

## 5. 渲染：动态高度、光标与水平偏移

- [x] 5.1 view 输入区动态高度 `clamp(行数+2, 3, 8)`，消息区/队列/widget 相应伸缩；验证 TestBackend 单测（1 行=3 高、20 行=8 高上限、消息区仍可见）
- [x] 5.2 水平偏移 scroll_x 保证光标列可见 + `frame.set_cursor_position` 光标定位；验证单测（长行光标在末尾时偏移正确、光标坐标落在文本区、CJK 近似容忍）
- [x] 5.3 多行渲染与超长多行压力（数十行 × 超长行）不 panic、不挤压消息区；验证构造压力测试

## 6. 键位语义切换（#2 + 多行键）

- [x] 6.1 ctrl+c 双语义：单击清空输入并记时、500ms 内双击退出（时间可注入便于单测）；验证单测（超时清空不退出、窗口内退出、空输入单击也只清空）
- [x] 6.2 ctrl+d 双语义：空输入→Quit、非空→光标处向后删；**移除 q 退出分支**（q 落入普通字符）；**空闲 esc 改为无操作**（Esc 守卫仅 streaming→Abort）；验证键位单测四条新语义 + 输入含 q 的单词不退出
- [x] 6.3 多行键：shift+enter / ctrl+j 换行（判定顺序 SHIFT→newline、ALT→followUp、普通→Submit）；验证 Enter 三分支键位单测

## 7. 提交与中断语义（#1/#3/#4）

- [x] 7.1 `submit_as("prompt")` 流式态 payload 附加 `streamingBehavior:"steer"`；空闲态不带该字段；验证假 pi PTY：流式中回车，脚本记录的 prompt 行含 steer、空闲行不含
- [x] 7.2 `SendFollowUp` 分派：流式→follow_up、空闲→普通 prompt（等同 Enter）；验证状态/键位单测两分支
- [x] 7.3 流式 esc 升级：一次性线程顺序发 `abort`→`clear_queue`，响应经既有 `RestoreQueued` 回填输入框，失败走红条；验证假 pi PTY：流式 esc 产生两条 stdin 且排队文本回填输入框、空队列时输入不变

## 8. 验收与收尾

- [x] 8.1 真机 PTY 验收清单：多行编辑（shift+enter/跨行光标/词行删除）、kill/yank/undo、流式 Enter 无红条且打断生效、流式 esc 恢复队列、ctrl+c 单/双击、ctrl+d 空退出、键入含 q 单词不退出、空闲 esc 无操作；验证逐项目测/目测确认，交互细节差异记入 `tui-usage.md` 已知差异
- [x] 8.2 文档：`tui-usage.md` 键位表与退出方式大改（q/esc 语义变化醒目提示）、`pi-tui-interaction.md` 差异表勾销 #1/2/3/4/8/10 并注明剩余项、`roadmap.md` 记录本次对齐；验证文档与实现一致
- [x] 8.3 全量 `cargo test` 与 `cargo clippy --all-targets` 零告警；`openspec validate align-interaction-semantics --strict` 通过；验证三项命令输出
