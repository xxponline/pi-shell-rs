# Tasks

## 1. 建立 lib crate 与 RPC 连接层（阶段 0）

- [x] 1.1 创建 `src/lib.rs`（crate 名 `pi_rust_tui`）并建立 `src/rpc/` 模块骨架（`mod.rs`、`process.rs`、`jsonl.rs`）；验证 `cargo build` 通过
- [x] 1.2 把 probe 的 `process.rs`（Transport/三线程/spawn/`PI_BIN`）迁入 `src/rpc/process.rs` 并改为 `pub` 接口，把 `jsonl.rs`（WireMessage/OutgoingCommand/Direction）迁入 `src/rpc/jsonl.rs`；验证迁移后的库单测（spawn 失败、fake-pi 回放、stderr 分离、parse_error）通过
- [x] 1.3 删除 probe 私有 `process.rs`/`jsonl.rs`，probe 改为 `use pi_rust_tui::rpc::{...}`；验证 `cargo test` 中原有探针测试全部通过且数量不减少
- [x] 1.4 确认 probe 迁移后行为不变：`cargo run --bin probe -- --list` 正常，且对一个内置场景产物结构（transcript/summary/meta）字段不变；验证方式为人工核对 + 测试

## 2. RpcClient（id 关联与事件分流）

- [x] 2.1 在 `src/rpc/client.rs` 实现 `RpcClient`：包装 Transport，发送命令对象时分配/透传唯一 id，按 id 通道返回响应；验证编译
- [x] 2.2 实现事件分流：无 id 顶层消息（agent_*/message_*/tool_execution_*/extension_ui_request 等）进入事件通道，绝不唤醒命令等待；验证用 fake-pi 单测覆盖乱序响应归属与事件不串道
- [x] 2.3 实现命令响应超时与子进程 shutdown 透传；验证一个永不响应的 fake-pi 场景按超时失败且不挂起
- [x] 2.4 `cargo clippy --all-targets` 无告警；验证无新增 warning

## 3. 无头 session 状态机：结构与生命周期（阶段 1）

- [x] 3.1 建立 `src/session/`（`mod.rs`、`blocks.rs`、`transcript.rs`），定义 `SessionState`（phase/thinking_level/session_name/queue/usage/transcript）与 `Block`/`Entry` 类型；验证编译
- [x] 3.2 实现 `handle(&Value)` 对生命周期事件的归约：agent_start/agent_settled 切换 Idle/Streaming，turn_start/turn_end 维护多 turn，message_start/end 处理 user 条目、忽略 system；验证构造事件单测覆盖 text_only 的流式→空闲
- [x] 3.3 未知事件安全忽略；验证单测：喂入虚构事件类型不报错且状态不变

## 4. assistant 内容块累积与权威校正

- [x] 4.1 实现按 `contentIndex` 的 text/thinking 占位与 `text_delta`/`thinking_delta` 追加；验证单测中拼接结果等于给定最终文本
- [x] 4.2 实现 toolcall_start（从顶层取 id/toolName）/toolcall_delta 追加 args/toolcall_end；验证单测中 toolCall 块 id 与 name 正确
- [x] 4.3 实现 `*_end` 与 `message_end` 的权威校正（用完整 content 替换累积块）；验证单测：人为制造 delta 漂移后 message_end 能校正
- [x] 4.4 用 text_only snapshot 离线断言 assistant text 与 thinking（若有）最终内容等于 message_end 权威消息；验证离线测试通过

## 5. 工具执行归一化

- [x] 5.1 实现 tool_execution_start/update/end 按 toolCallId 关联，记录 name/args/进行中/结束/isError/result/durationMs，含早到事件的 pending 暂存合并；验证构造单测
- [x] 5.2 用 tool_call snapshot 离线断言 read 工具执行已结束、无错误、含结果内容
- [x] 5.3 用 failed_tool snapshot 离线断言工具执行 isError=true，且其后 assistant 文本仍被累积；验证离线测试通过

## 6. transcript、运行态与多 turn

- [x] 6.1 保证 transcript 条目按会话发生顺序排列（user 先于 assistant，工具位于调用与后续回复之间）；验证 tool_call snapshot 顺序断言
- [x] 6.2 实现 queue_update 更新 steering/follow_up、thinking_level_changed、session_info_changed、usage 更新；验证构造事件单测
- [x] 6.3 多 turn 保留：用构造的 follow_up 多 turn 事件序列（必要时补跑一次 follow_up snapshot 入库）断言存在多轮 assistant 条目且不被覆盖；验证测试通过

## 7. JSONL 离线回放入口

- [x] 7.1 在 `src/session/replay.rs` 实现读取 probe transcript JSONL（每行含 dir/raw），只回放 `dir=="in"` 的 raw 调 `handle`；验证单元测试用内嵌 JSONL 字符串可重建状态
- [x] 7.2 增加集成测试，加载 text_only/tool_call/failed_tool 三个已归档 snapshot 离线回放并断言（消息数、块类型、工具成败、非空 transcript），全程不启动 pi；验证 `cargo test` 通过

## 8. 收尾与校验

- [x] 8.1 全量 `cargo test` 与 `cargo clippy --all-targets` 通过；验证 probe 13 个既有测试 + 新增 session/rpc 测试全绿
- [x] 8.2 在 `Docs/rpc-protocol-notes.md` 或 roadmap 中记录新的库模块位置与回放用法（简短一节即可）；验证文档中命令/路径可照做
- [x] 8.3 运行 `openspec validate extract-lib-and-session-state --strict` 并修复问题；验证严格校验通过
