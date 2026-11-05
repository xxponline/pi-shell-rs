# AGENTS.md

## 项目简介

用 Rust 复刻 pi 的终端 TUI：拉起 `pi --mode rpc` 无头核心，用 ratatui 在通用终端
渲染出与 pi 使用体验一致的界面，并在其上做定制。定位、功能分层与阶段路线见
[`Docs/roadmap.md`](Docs/roadmap.md)。

## 关键文档

| 文档 | 内容 |
|---|---|
| `Docs/roadmap.md` | 产品定位、复刻范围三档、工程结构、阶段 0-4 路线图（防漂移基准） |
| `Docs/rpc-protocol-notes.md` | RPC 协议实测笔记（事件流、message_update 增量语义、实测结论） |
| `Docs/library-and-replay.md` | `rpc` / `session` 库模块 API 与离线回放用法 |
| `Docs/tui-usage.md` | TUI 启动方式、键位、与原生 pi 的已知差异 |
| `Docs/probe-usage.md` | 协议探针用法（抓取真实事件流归档） |
| `Docs/pi-tui-interaction.md` | pi TUI 操作逻辑整理（分发模型/键位全表/差异清单——复刻基准） |
| `openspec/specs/` | 持久能力规格（rpc-probe / rpc-client / session-state / tui-app） |

## 编码规范

### 函数注释（强制）

**每个函数都必须写中文文档注释**，说明三件事：

1. **功能**：这个函数做什么
2. **输入**：参数是什么、含义、取值约束
3. **输出**：返回什么 / 产生什么副作用 / 错误情形

格式（`///`）：

```rust
/// 从探针归档的 transcript JSONL 重建会话状态。
///
/// 输入: reader — 逐行 JSON，形如 {"seq":..,"dir":..,"raw":..}
/// 输出: 只喂入 dir=="in" 的 raw 后得到的 SessionState；
///       非 JSON 行与出站行被跳过，不会报错
pub fn replay_jsonl<R: Read>(reader: R) -> std::io::Result<SessionState> { ... }
```

- 中文书写；简单函数可紧凑为"功能；输入…；输出…"一两行，但三要素不可省略。
- 提到的行为必须与实现一致，不得凭空描述。
- 结构体/枚举/公开字段也用 `///` 注释（中文）；现有英文注释在触碰该代码时顺手翻译。

### 其它 Rust 约定

- 提交前：`cargo test` 全绿 + `cargo clippy --all-targets` 零告警。
- 弱类型边界（`serde_json::Value` 进）集中判别，输出用强类型；协议演进不改归约语义。
- session 层不依赖终端；app 层不做状态归约——`app → session → rpc → pi` 单向依赖。
- 纯逻辑（状态映射、编辑缓冲、文本截断）抽成无 TTY 可测的函数并配单测；
  真实终端行为用 PTY 手工验证（参考 `Docs/tui-usage.md`）。

## 工作流

- 需求变更走 OpenSpec：`/opsx-propose` 规划 → `/opsx-apply` 实现 → `/opsx-archive` 归档同步。
- 提交前运行 `openspec validate <change> --strict`；主规格修改随归档同步，不手工改
  `openspec/specs/`（归档流程除外）。
- 新场景/新能力先查 `Docs/roadmap.md` 是否越界（阶段 3/4 的功能不要提前做）。

## 常用命令

```bash
cargo run                        # 启动 TUI（default-run）
cargo run --bin probe -- --list  # 协议探针
cargo test                       # 全部测试（含 fixture 离线回放）
cargo clippy --all-targets       # 静态检查
PI_BIN=/path/to/pi cargo run     # 指定 pi 可执行文件
```
