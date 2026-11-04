# Tasks

## 1. 依赖与终端骨架

- [x] 1.1 用 `cargo add` 引入兼容版本的 `ratatui` 与 `crossterm`，验证 `cargo build` 通过且版本组合无冲突
- [x] 1.2 新增 `src/app/` 模块骨架（mod/terminal/event/view/chat），让默认 bin 从 `src/main.rs` 装配启动一个能进入并干净退出 alt-screen 的空 TUI；验证在真实终端能启动后按退出键返回、终端状态正常
- [x] 1.3 实现 `terminal.rs` 的 RAII `TerminalGuard`（enable_raw_mode、EnterAlternateScreen，Drop 恢复；安装 panic hook 在恢复后再打印）；验证正常退出与人为 panic 两种情况下终端都被恢复（手工 + 可写的恢复逻辑单测）

## 2. SessionState 只读访问器（修改既有能力）

- [x] 2.1 在 `src/session/transcript.rs` 增加 `in_flight() -> &[Block]` 与 `is_in_flight() -> bool`，不改动归约逻辑；验证 `cargo build` 通过
- [x] 2.2 新增单测：delta 期间 in_flight 非空、message_end 落盘后清空、空闲为空、读取不改变 transcript；验证 `cargo test --lib` 通过且既有 session 测试不回归

## 3. 事件循环与 RPC 桥接

- [x] 3.1 定义 `app/event.rs` 的 `UiEvent`（Key/Rpc/Closed/Tick）与统一 mpsc；验证编译
- [x] 3.2 键盘线程用 crossterm `event::read()` 产出 Key 事件；验证真实终端按键能进入主循环（可加临时日志/状态显示手工确认）
- [x] 3.3 RPC 线程从 `RpcClient.events()` 读取并转发 `ClientEvent::Event(raw)` 为 Rpc 事件、`Closed` 为退出信号、`ParseError` 为提示；验证用库既有 fake-pi 思路无法直挂 TUI 时，至少保证转发分支被单测/编译覆盖
- [x] 3.4 主循环把 Rpc 原始事件喂给 `SessionState::handle` 并在变更后重绘；验证真实对话时界面随事件更新（手工真机）

## 4. 启动与探活

- [x] 4.1 main 启动流程：建 guard → `RpcClient::spawn(resolve_binary(), &[])` → `get_state`（15s, RequireSuccess）→ 进入 run；验证编译
- [x] 4.2 失败路径：pi 不可执行或 get_state 失败/超时时，恢复终端并打印可读错误（含"先用原生 pi 登录"提示），以非零码退出；验证用 `PI_BIN` 指向不存在路径手工确认报错干净、终端恢复

## 5. 视图纯函数（无 TTY 可测）

- [x] 5.1 `chat.rs` 实现把 `SessionState.entries()` + `in_flight()` 映射为 ratatui `Vec<Line>`：user 前缀、assistant text 原文、thinking 单行占位、toolCall 单行状态、notice 单行；验证纯函数单测覆盖各类条目文本
- [x] 5.2 `view.rs` 用 ratatui Layout 划分 header(1)/body(剩余)/editor(1~2) 三区，状态行显示 phase 与会话名（可用时），body 用 Paragraph wrap 并自动跟随到底；验证布局纯函数/可测部分有断言且不 panic
- [x] 5.3 超长文本/大量条目不导致 panic 或覆盖输入区；验证构造超长内容的纯函数测试通过

## 6. 输入编辑与发送

- [x] 6.1 输入缓冲支持可打印字符追加、Backspace 删除；验证编辑逻辑抽成可单测的函数并有测试
- [x] 6.2 Enter：非空时清空输入并通过一次性线程发送 `prompt`（不阻塞渲染），空/全空白不发送；验证空输入不发送的单测 + 真机手工确认发送
- [x] 6.3 用户提交时立即在消息区显示该用户消息（与 session 落盘的 user 条目一致、避免重复）；验证真机手工确认不出现重复气泡

## 7. 流式显示与中断

- [x] 7.1 渲染进行中 in_flight 块（text 实时累积，thinking/toolCall 用占位），收到 agent_settled 后以落盘 transcript 为最终显示；验证真机手工确认 delta 实时出现且结束后内容稳定
- [x] 7.2 esc 在流式中通过一次性线程发送 `abort` 并回到可输入状态，空闲时 esc/q 退出；验证真机手工确认两条路径
- [x] 7.3 Ctrl-C 安全退出并恢复终端（不依赖默认终止行为）；验证真机手工确认

## 8. 收尾

- [x] 8.1 全量 `cargo test`、`cargo clippy --all-targets` 零告警；验证 probe 与库既有测试全绿
- [x] 8.2 在 `Docs/` 增加简短的 TUI 启动/键位说明（如何运行、Enter/esc/Ctrl-C、PI_BIN），并与 roadmap 阶段 2 交叉引用；验证文档命令可照做
- [x] 8.3 `openspec validate add-minimal-ratatui-tui --strict` 通过；验证严格校验无错误
