# Tasks

## 1. 脚手架与依赖

- [x] 1.1 在 `Cargo.toml` 声明第二个 bin target（`name = "probe"`, `path = "src/bin/probe/main.rs"`），加入最小依赖 `serde`（derive）与 `serde_json`；验证 `cargo build` 与 `cargo run --bin probe -- --help` 均成功
- [x] 1.2 创建 `src/bin/probe/` 模块骨架（main/process/jsonl/scenario/classify 空模块 + `scenarios/mod.rs`），验证 `cargo clippy` 无告警
- [x] 1.3 在 `.gitignore` 忽略原始运行产物 `tests/fixtures/probe/**/<timestamp 形态>/`（或运行根 `probe-out/`），但保留目录占位 `.gitkeep`；验证 `git status` 不显示本地产物

## 2. 子进程通道

- [x] 2.1 实现 `process.rs`：从 `PI_AGENT_BIN` 或 PATH 启动 `pi-agent --mode rpc`，拿到独立 stdin/stdout/stderr；启动失败时给出明确错误并以非零码退出（验证：把 `PI_AGENT_BIN` 指向不存在的路径，错误信息包含原因且不挂起）
- [x] 2.2 实现 writer / stdout reader / stderr reader 三线程与 channel 汇聚；stderr 带时间戳写入产物目录 `stderr.log`（验证：运行后 stderr.log 存在，且 stdout 解析从不读取 stderr 内容）
- [x] 2.3 实现 stdout 行解析：合法 JSON 行分类投递，非法行以 `parse_error` 原样记录而不崩溃（验证：可用注入假 pi 脚本输出脏行的方式单测，见 2.4）
- [x] 2.4 抽出 transport 层使其可用一个"按行吐 fixture 的假 pi 子进程"做集成测试；验证：用一段 shell/node 假脚本按文件回放行时，探针能完整接收

## 3. Command / 响应关联与场景引擎

- [x] 3.1 实现 command 封装：自动附加/透传唯一 `id`，按 id 建立等待通道；事件（无 id）不唤醒响应等待（验证：单元测试覆盖乱序响应归属与事件不串道）
- [x] 3.2 定义 `Scenario`/`Step`（Send、WaitEvent、WaitSettled、Sleep、AnswerNextUi）并实现 runner，步骤严格顺序执行、每步带超时；验证：用假 pi 回放跑通一个多步场景
- [x] 3.3 实现 `AnswerNextUi`：缓存下一个匹配 method 的 `extension_ui_request`，自动回发 `extension_ui_response`，请求与回复都进 transcript；验证：假 pi 发出 select 请求后能收到带相同 id 的 value 响应
- [x] 3.4 超时/失败路径：等待超时或场景失败时 kill 子进程、保留已抓产物、summary 写明失败步骤；验证：构造一个永不 settle 的假场景，确认按超时失败且退出码非零、产物完整

## 4. 产物持久化与分类统计

- [x] 4.1 实现 transcript 写入：每行 `{seq, ts_ms, dir, raw}` 封装且 raw 为原始消息体；stderr 单独文件；验证产物每一行业务行是独立合法 JSON 且可被测试逐行回放
- [x] 4.2 实现 `classify.rs`：顶层 type 计数，以及 `message_update.assistantMessageEvent.type`、`tool_execution_*`、`extension_ui_request.method` 的二级计数；验证用构造输入单测计数准确（含 text_delta/toolcall_start/tool_execution_start 混合用例）
- [x] 4.3 写 `summary.json`（计数、每步成败、截断记录）与 `meta.json`（pi-agent 版本、cwd、git sha、场景名）；验证真实/假运行后两文件字段齐全
- [x] 4.4 实现 `--snapshot`：复制到受 git 跟踪的 `tests/fixtures/probe/<name>/snapshot/`，对超阈值字符串字段按 JSON pointer 截断并在 summary 记录原长；验证截断后 JSON 仍可解析、summary 含截断记录
- [x] 4.5 更新稳定路径 `latest/`（复制或符号链接）；验证测试代码可不依赖时间戳加载 latest

## 5. CLI 与内置场景（真实 pi-agent）

- [x] 5.1 实现 CLI：`--list` / `--scenario <name>` / `--all` / `--out` / `--timeout-ms` / `--snapshot`，全部场景结束时任一失败则退出码 1 且不阻断后续场景；验证参数行为
- [x] 5.2 启动探活：运行任何场景前先发 `get_state`，失败时明确提示先用原生 pi 完成登录；验证未登录环境的报错可读
- [x] 5.3 场景 `text_only`：get_state → 小 prompt → WaitSettled；真实跑通，产物含 agent_start/message_update(text_delta)/message_end/agent_end/agent_settled
- [x] 5.4 场景 `tool_call`：prompt 要求读取仓库内固定文件；验证事件中 toolcall_start→tool_execution_start/update/end→toolcall_end 的交织顺序被完整记录
- [x] 5.5 场景 `thinking`：临时 set_thinking_level high 后提问；验证 thinking_delta 出现，并在场景末尾恢复原 level
- [x] 5.6 场景 `steer`：prompt 后在生成中（不等 settled）发 steer，验证 prompt 响应 disposition 与 steer 的排队/打断行为被记录
- [x] 5.7 场景 `follow_up`：生成中连续排两条 follow_up，验证 queue_update 与最终依次执行
- [x] 5.8 场景 `compact`：set_auto_compaction 关闭后手动 compact，验证 compaction_start/end 及 result 字段
- [x] 5.9 场景 `failed_tool`：构造一次必然失败的工具调用，验证 tool_execution_end.isError=true 与错误 result 形状
- [x] 5.10 先做只读探查确认 pi 的扩展加载约定（cwd 下 `.pi/extensions/` 或 CLI/env 注入），记录结论；据此放置最小 fixture 扩展（触发 `ctx.ui.select` 等）
- [x] 5.11 场景 `extension_ui`：触发扩展弹窗并用 AnswerNextUi 回复，验证 9 种 method 至少覆盖 select/confirm/notify（其余 method 在同扩展内尽量覆盖，无法触发的在 summary 注明）

## 6. 文档回填与归档检查

- [x] 6.1 用各场景证据逐条回填 `Docs/rpc-protocol-notes.md` 第 8 节 8 个问题：标注"已验证 + 场景名"或"与类型不符 + 差异说明 + fixture 引用"；验证没有无证据即关闭的条目
- [x] 6.2 根据真实 fixture 体积决定 snapshot 提交策略并在 design 的 Open Question 处落定（更新 design.md 记录决策）；验证 `cargo test` 能读取 snapshot 逐行解析
- [x] 6.3 为 transport/回放路径补一个基于已归档 snapshot 的集成测试（不依赖真实 pi/网络），验证 `cargo test` 离线通过
- [x] 6.4 运行 `openspec validate build-rpc-probe-client --strict` 并修复所有报错；验证严格校验通过
