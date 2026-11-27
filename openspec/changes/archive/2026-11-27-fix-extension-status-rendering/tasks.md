# Tasks

## 1. SGR 解析纯函数

- [x] 1.1 新增解析模块（`src/app/ansi.rs`，随 `src/app/mod.rs` 挂载）实现 `parse_ansi_spans(text, base) -> Vec<Span>`：覆盖真彩 `38/48;2;r;g;b`、256 色 `38/48;5;n`、标准 8/16 色、属性 `1/2/3/4` 与 reset `0/22/23/24/39/49`；未知 CSI/OSC/APC 按 pi `ansiCodeLength` 边界语义跳过（`ESC[` 扫至 `m/G/K/H/J`，`ESC]`/`ESC_` 扫至 BEL 或 `ESC\`）；剔除 U+200B。验证：`cargo test` 中解析器单测全绿——含 `\x1b[38;2;0;255;136m25.0 tok/s\x1b[0m` 产出 `Color::Rgb(0,255,136)` 段、`\x1b[2J`/OSC8 跳过不残留、畸形输入（空参数、缺终止符、截断于序列中点）不 panic。
- [x] 1.2 为解析器补"未知/畸形序列"与属性 reset 边界用例（`22` 只清 bold+dim 不清 italic 等），验证：`cargo test` 全绿且无 `#[ignore]`。

## 2. statusKey 分格存储

- [x] 2.1 `src/app/mod.rs` 把 `status_text: Option<String>` 改为 `BTreeMap<String, String>`；`handle_extension_request` 的 `setStatus` 分支按 `statusKey` 写格（缺省字段落固定默认格）、`statusText: null` 只 `remove` 该格。验证：新增单测——key=a/b 并存、a 置 null 后 b 保留、无 statusKey 请求落默认格，`cargo test` 全绿。
- [x] 2.2 同步既有断言与取值路径：更新 `src/app/mod.rs` 中 `status_text` 相关旧测试（如 `assert_eq!(app.status_text, None)`）与 `status_grid_line` / `draw` 里的读取点为"按键排序 join"。验证：`cargo test` 全绿，无编译告警。

## 3. 渲染装配与截断

- [x] 3.1 `view::header_line` 状态段改为对状态文本调 `parse_ansi_spans` 后 `spans.extend(...)`（不再整段套洋红单 Span）；GPUI 侧 `status_grid_line` 复用同源装配、不重复实现。验证：`header_line` 单测断言真彩段产出 `Color::Rgb`、可见文本不含 `[38;2;`，`cargo test` 全绿。
- [x] 3.2 状态段宽度截断改为按可见宽度（解析产出的 spans 按 `Span::width()` 累计，超出可用宽度处截断并加省略标记）。验证：单测——含转义字符串不被提前截断、超宽时省略标记落在可见文本上、零宽字符不占位。

## 4. 集成验证

- [x] 4.1 全量回归：`cargo test` 全绿 + `cargo clippy --all-targets` 零告警；确认 `openspec/specs/tui-app/spec.md` 无需手工改（随归档同步）。
- [x] 4.2 真实终端 PTY 手工验证（参考 `Docs/tui-usage.md`）：启动 `PI_BIN=pi cargo run`，装有 `pi-token-speed` 的会话中流式回复，确认状态行 TPS 为彩色可读文本、无转义残留、会话名/usage 不被挤位。
