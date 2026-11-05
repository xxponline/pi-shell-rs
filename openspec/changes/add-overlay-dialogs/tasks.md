# Tasks

## 1. overlay 框架与焦点路由

- [x] 1.1 新建 `src/app/overlay.rs`：`Overlay` 枚举（6 变体）+ `centered_rect` + `Clear` 打底的列表/确认/输入/多行编辑渲染骨架，`view::render` 最上层接入（浮层存在时底层消息区/状态行照常渲染）；验证 TestBackend 渲染单测：浮层居中、底层内容仍可见
- [x] 1.2 焦点路由：`handle_key` 前置拦截——ctrl+c 永远全局，浮层打开时按键交 `overlay_key()` 且消费后不进 `key_intent`/输入框；验证单测：浮层内方向/回车/esc/可打印字符均不泄漏到主输入框，关闭后主键位恢复
- [x] 1.3 弹层单槽队列 `VecDeque<Overlay>`（多请求串行）+ 关闭后界面恢复；验证单测：连续入队两个弹层逐个处理，关闭前后 TestBackend 帧一致（无残留）

## 2. extension UI 分发

- [x] 2.1 事件循环在 `state.handle` 之前截获 `extension_ui_request`：对话 4 method 入弹层队列、被动 5 method 立即应用；验证分发路由单测（构造两类请求各一，断言去向）
- [x] 2.2 被动 method 实现：notify→`App.notice`（按 notifyType 着色）、setStatus→`status_text`、setWidget→`widget_lines`（null 清除）、setTitle→crossterm SetTitle 直写、set_editor_text→覆盖 `input`；验证单测：五种各自的状态效果，且断言全程未产生任何 extension_ui_response 发送
- [x] 2.3 lib 扩展：`CommandSender::send_raw(&Value)`（与 RpcClient.send_raw 同走 PumpMessage::Raw）；对话 method 的回复载荷构造（select/input/editor→value、confirm→confirmed、取消→cancelled:true）；验证载荷单测 + 既有 rpc 测试不回归
- [x] 2.4 `UiEvent::AsyncData(kind, Value)` 通道（模型/级别/应用结果三类 kind）与过期数据丢弃（kind 或序号不匹配则忽略）；验证单测：旧数据晚到不覆盖新弹层状态

## 3. 对话 4 method 浮层

- [x] 3.1 ExtSelect/ExtConfirm/ExtInput 浮层完整实现：渲染（标题/选项/占位符）、↑↓ 导航、回车提交回发响应、esc 回发 cancelled 并关闭；验证状态单测 + 用既有 `extension_ui` fixture（select/confirm）回放断言浮层构造与回复载荷正确
- [x] 3.2 ExtEditor 多行编辑浮层：prefill 按行载入、可打印插入、Backspace 行首回并、alt+enter 换行、Enter 提交全文、esc 取消；验证编辑操作单测（含单行/多行/空 prefill 三态）
- [x] 3.3 假 pi PTY 端到端：脚本化 select/confirm/input/editor 四请求依次到达，断言四条 extension_ui_response（含 value/confirmed/cancelled 各形态）与浮层画面出现/消失

## 4. model/thinking 选择器

- [x] 4.1 model 选择器：ctrl+l 打开（async `get_available_models`，加载期 loading 占位）、↑↓ 导航、回车发 `set_model` 并经 AsyncData 更新状态行 model、esc 关闭不发命令；验证状态单测（打开/加载/应用/取消四态）
- [x] 4.2 模型循环：ctrl+p / shift+ctrl+p 发 `cycle_model`，响应更新状态行，`null` → notice "无可用模型"；验证单测（正常更新与 null 两分支）
- [x] 4.3 thinking：shift+tab 发 `cycle_thinking_level`（响应更新）、alt+t 打开级别列表选择应用（`get_available_thinking_levels`/`set_thinking_level`）、esc 取消；验证键位与状态单测，状态行 thinking 段同步
- [x] 4.4 真机与真实 pi 联调：ctrl+l 打开并切换模型、ctrl+p 循环、shift+tab 循环 thinking、alt+t 列表选择，状态行均正确更新；验证手工 PTY/终端操作逐项确认

## 5. write/grep/find/ls 专用卡片

- [x] 5.1 write 卡片：头行 path（key_arg 已支持）+ body 为 args.content 预览、退回 result text；验证 tool_edit fixture 中 write 调用渲染断言 + args.content 合成用例
- [x] 5.2 grep 卡片：头行 `/pattern/ in path`、body 输出行 + `details.linesTruncated` 截断提示；验证合成 shape 单测（含无 details、无 result 降级）
- [x] 5.3 find / ls 卡片：头行 pattern·path / path，body 结果行，缺失降级通用卡片；验证两卡合成单测 + 缺字段不 panic
- [x] 5.4 ctrl+o 对四种新卡片的展开/折叠生效；验证 flags 单测（折叠 ≤1+3 行、展开更多）

## 6. 被动 method 端到端

- [x] 6.1 假 pi PTY：脚本连发 notify/setTitle/set_editor_text/setWidget/setStatus，断言界面出现通知与 widget、标题/输入框生效，且假 pi 的 stdin 记录中**零** extension_ui_response；验证脚本内断言全过

## 7. 文档与真机验收

- [x] 7.1 真机验收清单：弹层打开/关闭底层无损、模型选择与循环、thinking 循环与列表、extension 9 method（4 对话往返 + 5 被动）、4 种新卡片与 ctrl+o、真实 grep 卡片检查（发现结果形状差异则补 probe 场景并记录）；验证逐项目测确认，差异记入 tui-usage 已知差异（含 alt+t 与 pi `/thinking` 的键位差异）
- [x] 7.2 更新 `Docs/tui-usage.md`（新键位/弹层/widget 区/已知差异）并在 `Docs/roadmap.md` 阶段 4 标注 4a 完成、4b 待办；验证文档与实现一致

## 8. 收尾

- [x] 8.1 全量 `cargo test`（既有 89 项 + 新增）与 `cargo clippy --all-targets` 零告警；`openspec validate add-overlay-dialogs --strict` 通过；验证三项命令输出
