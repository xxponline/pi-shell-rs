# Design

## Context

`Docs/rpc-protocol-notes.md` 已经从 pi 源码的 TS 类型定义整理出 RPC 协议全貌，但第 8 节列出了 8 个只能靠实测回答的问题。当前仓库只有 `src/main.rs` 的 hello world，还没有任何 JSONL/子进程代码。这个探针是**一次性工具，但会被反复重跑**（pi 升级后也要跑），所以架构上偏向：少依赖、代码直白、产物稳定可 diff。

协议侧硬约束（已在 notes 中确认）：
- stdin/stdout 都是逐行 JSON；stdout 混合 `response`（带 id）/ 事件（无 id）/ `extension_ui_request`（带 id）/ `extension_error` 四种消息。
- 关联响应只能靠 `id`；事件只能靠时间序和状态机。
- 一次 prompt 的结束信号是 `agent_settled`，不是 `response{command:"prompt"}`（那只是受理）。
- `message_update` 在 wire 上已被剥掉 `message` 与 `partial`，只有 delta。

## Goals / Non-Goals

**Goals:**
- 用最少的 Rust 代码跑通真实 pi-agent，产出可重放、可 diff 的 JSONL fixtures。
- 让"场景定义"是声明式数据，而不是写死在 main 里——后续加场景只改一个数组。
- 探针产物能直接作为未来 TUI 渲染层单元测试的输入（带方向的消息记录）。
- 二进制与未来的主 TUI 二进制完全隔离（`src/bin/probe/`），主库暂时零代码。

**Non-Goals:**
- 不写通用 RPC client SDK（不做自动重连、不做连接池、不抽象 transport trait）。
- 不做任何终端 UI（不引入 crossterm/ratatui）。
- 不做交互式 REPL；场景全部来自内置定义或 `--scenario <name>`。
- 不解析 `AgentMessage` 的全部嵌套字段——探针侧大部分消息体用 `serde_json::Value` 透传，只在分类统计需要的路径上做结构提取。
- 不安装任何东西到 `~/.pi/agent/extensions/`（测试扩展只通过 pi 的扩展加载参数注入，见决策 4）。

## Decisions

### 1. 运行时：用 `std::process::Child` + 三个 OS 线程，不引入 tokio

**选择**：std 阻塞 IO + 三个线程：
- writer 线程：从 `mpsc::Receiver<String>` 取行写入 child.stdin。
- reader 线程：`BufRead` 逐行读 child.stdout，解析顶层 `type`，投递到中央 dispatcher。
- stderr 线程：逐行读 child.stderr，打时间戳写入产物目录的 `stderr.log`。

**理由**：
- 探针唯一的并发模型就是"两个流 + 一个场景驱动器"，线程 + channel 已经足够直白。
- 减少第一个变更的依赖面，编译快、diff 干净；未来 TUI 主二进制再单独决定 async runtime。
- 超时用 `recv_timeout` / 条件变量即可，不需要 timer runtime。

**备选**：tokio + tokio::process —— 被否，原因是收益小、依赖多；等真正写 TUI（要处理键盘事件、渲染 tick、子进程、信号）时再引入更合理。

### 2. 模块布局：`src/bin/probe/` 单 bin target，场景数据驱动

```
src/bin/probe/
├── main.rs           // CLI 参数、场景调度入口
├── process.rs        // spawn pi-agent、三线接入
├── jsonl.rs          // 行封装（写入时附加 id、读取时顶层分类）
├── scenario.rs       // Scenario / Step 数据结构 + runner
├── classify.rs       // 顶层 type 与 message_update 二级分类统计
├── scenarios/        // 内置场景定义（一个文件或按场景拆文件）
│   ├── mod.rs
│   ├── text_only.rs
│   ├── tool_call.rs
│   ├── thinking.rs
│   ├── steer.rs
│   ├── follow_up.rs
│   ├── compact.rs
│   ├── failed_tool.rs
│   └── extension_ui.rs
└── fixture_extension/
    └── limit-or-ping.ts   // 场景 8 使用的最小扩展（见决策 4）
```

`Cargo.toml` 显式声明：
```toml
[[bin]]
name = "probe"
path = "src/bin/probe/main.rs"
```
保留默认 `src/main.rs` 的 hello world 不动（未来再演进为 TUI bin）。

### 3. 场景模型（声明式）

```rust
enum Step {
    Send { command: serde_json::Value, expect_response: bool, timeout_ms: u64 },
    WaitEvent { event_type: &'static str, timeout_ms: u64,
                // 可选的二级谓词，例如 assistantMessageEvent.type == "toolcall_start"
                predicate: Option<...> },
    WaitSettled { timeout_ms: u64 },
    Sleep(u64),
    AnswerNextUi { method: &'static str, response: UiAnswer },
}
struct Scenario { name: &'static str, description: &'static str, steps: Vec<Step> }
```

场景运行器持有一个 `HashMap<id, oneshot::Sender<Value>>`（用 `std::sync::mpsc` + 每命令一个 channel），以及一个"UI 请求队列"用于 `AnswerNextUi`。事件不进 command 响应通道，只被分类器和 predicate 观察。

**等待结束的判定**：`Send{prompt}` 本身只等 `response{command:"prompt"}`；是否等到 `agent_settled` 由场景显式写下一步 `WaitSettled`。这样场景定义能精确表达"生成中发 steer"之类的时序。

### 4. 场景 8（extension_ui_request）如何注入测试扩展

探针需要让 pi-agent 加载一个**只在本次运行存在**的扩展，该扩展提供一个 slash command（或监听某个 hook）触发 `ctx.ui.select`。

**选择**：在产物/临时目录生成扩展文件，通过 pi 已有的"项目级扩展目录"（`.pi/extensions/`，见 pi 文档）+ 在临时 cwd 下运行探针的方式加载；若该机制对 cwd 有要求，则改为把扩展路径通过 pi 支持的扩展加载参数（CLI flag 或 env）显式注入——**实现时先用一条只读探查命令确认 pi 的扩展加载约定，再落具体方式**，不假设尚未验证的 flag 名。

**备选**：直接安装到 `~/.pi/agent/extensions/` —— 被否，污染宿主环境，且这是个人开发机的全局目录。

### 5. 产物目录布局

```
tests/fixtures/probe/<scenario-name>/<timestamp>/
├── transcript.jsonl   // 每行: {seq, ts_ms, dir:"in"|"out", raw: <原始对象>, stderr?:...}
├── stderr.log         // 原始 stderr，带行号/时间戳
├── summary.json       // 分类计数 + 每步成败 + 截断记录
└── meta.json          // pi-agent 版本、cwd、场景名、git sha
```

再额外维护一个**稳定路径** `tests/fixtures/probe/<scenario-name>/latest/`（符号链接或复制），供测试代码不依赖时间戳加载。

**截断规则**：transcript 不截断（全量保留，diff 价值在这）；`summary.json` 不内嵌大字段。体积控制靠 `.gitignore` 策略：
- 默认**不**提交 `<timestamp>/` 原始目录（可能含会话内容/密钥痕迹）。
- 提供单独的 `--snapshot` 标志：把当前 run 复制到 `tests/fixtures/probe/<name>/snapshot/`（受 git 跟踪），复制时对超过阈值的字符串字段（tool result、image base64）做可配置截断，并在 `summary.json` 记录被截断的 JSON pointer 与原长度。
- snapshot 就是 spec 里"归档到仓库内固定位置、供测试加载"的那份产物。

> 注：是否在首个变更就默认提交 snapshot，取决于真实 fixture 大小；tasks 里先做机制，跑一遍后再决定提交策略（见 Open Questions）。

### 6. 顶层消息分类

不把消息反序列化成强类型 enum（pi 字段会演进），而是：
- 只提取判别字段到 `MessageKind`：
  - `type == "response"` → 再取 `command` + `success`
  - `type == "message_update"` → 再取 `assistantMessageEvent.type`
  - `type` 以 `tool_execution_` 开头 → 二级
  - `type == "extension_ui_request"` → 再取 `method`
  - 其它 → 仅按 `type` 计数
- 其余整体作为 `serde_json::Value` 原样写入 transcript。

### 7. CLI 形态

```
probe --scenario <name> [--all] [--out <dir>] [--timeout-ms <n>] [--snapshot]
probe --list
```

默认 `pi-agent` 从 PATH 查找；可用 `PI_AGENT_BIN` 环境变量覆盖。退出码：全部场景成功 0，否则 1。

## Risks / Trade-offs

- **[stdout 混入非 JSON 行]** → 笔记第 8 节的待验证项之一。reader 对无法解析的行不丢弃，原样记入 transcript 并标记 `parse_error`；这本身就是要抓的证据。
- **[等待条件死锁/挂起]** → 每个等待步骤强制带超时（场景定义或全局默认）；超时时保留已抓数据、kill 子进程、产物落盘后报错退出。
- **[场景依赖真实模型，结果不稳定/花钱]** → 场景 prompt 尽量小而确定（如让 read 工具读仓库内固定文件）；fixture 校验基于"事件结构/顺序"而非具体文本；不做跨 run 字节级 diff。
- **[扩展加载方式假设错误]** → 决策 4 已留缓冲：实现时先做只读探查确认加载约定，再写场景 8；若两种机制都不成立，场景 8 降级为"手动步骤 + 文档说明"，其余场景不受影响。
- **[fixture 泄露敏感信息]** → snapshot 默认截断大字段；原始 timestamp 目录 git-ignore；meta/transcript 在写入 snapshot 前经过同一截断路径。
- **[探针代码与未来真实 client 重复]** → 明确接受：探针只做分类统计所需的最小解析，未来的消息累积状态机在 TUI 侧重新实现，不提前抽象。
- **[auth 缺失导致所有场景失败]** → 启动前先跑一次 `get_state` 探活；失败信息明确提示"先用原生 pi 完成登录"。

## Migration Plan

纯新增，无迁移。落地顺序：通道 → 单场景（text_only）打通 → 场景框架与产物 → 其余场景 → 回填文档。回滚方式：删除 `src/bin/probe/` 与 fixture 目录即可，无其它系统受影响。

## Resolved Questions

- **snapshot 提交策略（已定）**：真实快照体积 68–124 KB（4 个场景，截断阈值 4000 字符），体积可接受。决定**按需 `--snapshot` 生成、并将 `tests/fixtures/probe/<name>/snapshot/` 纳入 git 跟踪**；原始时间戳 run 仍走 git-ignore。已用离线回放测试消费这些 snapshot。
- **扩展加载方式（已定）**：可执行文件实际是 **`pi`（v1.5.0）**，子进程命令为 `pi --mode rpc`（`PI_BIN` 可覆盖）；扩展通过 **`-e <path>`（`--extension`）显式注入**，无需安装到 `~/.pi/agent/extensions/`，也不依赖 cwd。fixture 扩展位于 `probe-fixture-extension/probe-ui.ts`，注册 `/probe-ui` 命令调用 `ctx.ui.select/confirm/notify`。
- **compact 场景（已定）**：compaction 阈值 `keepRecentTokens = 20000`，探针级小会话无法成功 compact；`compact` 会返回业务错误但仍发 `compaction_start/end`。场景用 `Send{expect_error:true}` 容忍该错误并抓取事件。

## Open Questions

- 暂无（后续若加 direct-bash / resume 场景，再评估 `bash_execution_update` 与 `entry_appended` 的 wire 语义）。
