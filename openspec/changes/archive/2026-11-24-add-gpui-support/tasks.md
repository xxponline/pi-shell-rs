# Tasks

## 1. 特性门控骨架

- [x] 1.1 Cargo 增加 optional 依赖 `gpui-kit`/`kameo` 与 feature `gpui-support`、
      bin `pi-shell-gpui`（`required-features`）；`lib.rs` 加 `#[cfg]` 条件模块声明
      与空模块树 `src/gpui_agent/{actor,terminal,window}.rs`。验证：默认
      `cargo build`/`cargo test`/`cargo clippy --all-targets` 全绿且依赖树无
      gpui/kameo（`cargo tree --no-default-features` 无该两系条目），
      `--features gpui-support` 可编译。
- [x] 1.2 双矩阵验证脚本/注记：记录 `cargo test` 与
      `cargo test --features gpui-support` 两列命令到 `Docs/tui-usage.md` 或
      AGENTS 常用命令。验证：文档中的命令原样可跑。

## 2. RpcActor 薄层路由

- [x] 2.1 `gpui_agent/actor.rs`：kameo actor 拥有 `RpcClient`（经既有
      `command_sender()`/`take_events()` 组装，不改泵线程）；ask 命令按 id 匹配
      响应/超时，tell 把无 id 事件扇出订阅者。验证：单测——假 pi 脚本回包，
      ask 收到 data、事件被订阅者收到（复用 `rpc/client.rs` 的 fake 模式）。
- [x] 2.2 actor 生命周期：spawn 探活（`get_state` 失败杀子进程返回 Err）、
      shutdown 幂等回收。验证：单测——探活失败路径与 shutdown 后 ask 报错，
      参照 `embed.rs` 测试口径。

## 3. 组件骨架与独立窗口

- [x] 3.1 `gpui_agent/window.rs` + `src/bin/gpui_window.rs`：开窗内嵌
      `GPUIAgentTerminal`，关闭窗口退出会话并回收 pi 子进程。验证：
      `cargo run --bin pi-shell-gpui --features gpui-support` 手动冒烟（启动窗口、
      关闭后 `ps` 无残留 pi）。
- [x] 3.2 `gpui_agent/terminal.rs` 骨架：接 RpcActor，三分区占位布局
      （消息区/输入区/状态行）+ 等宽字符网格度量（cell 宽高 → cols/rows 换算）。
      验证：无头窗口（gpui-kit `test-support`）ElementSnapshot 断言三区分割。

## 4. 复用底子接入

- [x] 4.1 组件内嵌 `SessionState` 归约：RpcActor 事件 → `session/` 状态机 →
      组件状态。验证：单测——回放 `tests/fixtures` JSONL 断言组件侧状态与
      `session` 层一致（离线、无 pi）。
- [x] 4.2 输入翻译层：gpui 键盘/鼠标/滚轮 → `UiEvent` 载荷（映射表抄宿主
      `keystroke_to_keyevents` 思路），IME commit 逐字符注入。验证：单测——
      按键/滚轮/修饰键组合翻译断言（对照 `event.rs` 既有测试口径）。
- [x] 4.3 全局键位拦截 + 内建编辑器文本桥接 + 斜杠补全（候选复用 `autocomplete`、
      应用回写内建编辑器；编辑键不进 `App::handle_key`，语义动作前后内建编辑器
      ↔ `App.editor` 双向同步）。验证：无头测试——enter/esc/ctrl+c 经拦截路径
      生效、Tab 补全应用后编辑器文本正确、编辑不误改 App 镜像。

## 5. P0 渲染面（内建控件优先）

- [x] 5.1 `wrap`/`layout_cache` 抽核换类型（新类型承载 cell 布局，原导出不动；
      注：仅供 ratatui 路径，GPUI 改内建控件后不消费——design D2 修订）。
      验证：抽核函数单测双跑——同一输入断言与现 ratatui 路径同一断行结果。
- [x] 5.2 消息区（内建）：虚拟滚动组件承载条目 + 内建 markdown 渲染助手文本
      （代码高亮）、thinking 折叠、read/bash/edit 工具卡 + 通用卡兜底、错误/重试
      提示、流式贴底跟随（新事件不打断回看）。验证：fixture 无头快照断言
      （markdown/工具卡/错误可见）+ 回看不被打断。
- [x] 5.3 状态行（内建 status_bar：phase/会话名/thinking/model/用量/费用/notice，
      与 `view::header_line` 同源数据）。验证：无头快照断言 model/tokens/notice
      在场。
- [x] 5.4 输入区（内建 TextArea/Editor）：多行渲染、内建光标、高度随行数增长
      但有上限且不挤压消息区。验证：无头快照断言（文本在场 + 高度上限 +
      消息区仍可见）。
- [x] 5.5 性能治理：reduce/回报泵攒批合并通知（流式事件风暴一次重绘）+
      `PI_GPUI_RENDER_PROBE=1` 分段渲染探针。验证：探针输出分段耗时、流式场景
      帧数不随事件数线性增长。
- [x] 5.6 清理网格自绘遗留：消息区/输入区的网格绘制已随 5.2/5.4 移除
      （`cursor_viewport`/`editor_box_lines`/`chat_grid_lines`/`grid_row`/
      `grid_column`/`handle_wheel`/滚轮累积器等）；状态行段绘制
      （`status_grid_line` + `grid_row_with_id`）保留为 `header_line` 的画笔
      （5.3 的渲染机制，非消息区网格遗留）；ratatui 抽核与既有单测不动。
      验证：双列编译/clippy 零死代码告警、全量测试绿。

## 6. P0 交互闭环

- [x] 6.1 启动探活与错误提示（pi 缺失/探活失败可读错误、杀子进程）。
      验证：单测——缺 bin 与 stub 探活失败两路径（参照 embed 测试）。
- [x] 6.2 prompt 流式提交（含流式中 `streamingBehavior:"steer"`）、esc 中断 +
      队列回填、空闲 esc 无操作、ctrl+c 清空——走「全局键位拦截 + 文本桥接」
      新路径。验证：单测——假 pi 脚本驱动，断言 JSON 命令序列与内建编辑器
      文本状态（既有 6.2 测试随路径改造）。
- [x] 6.3 斜杠命令查讯型子集（/model /cost /help 类）+ 未支持提示桩。
      验证：单测——命令分发断言（复用 `commands.rs` 分发逻辑）。

## 7. 双矩阵验收与文档

- [x] 7.1 全量回归：`cargo test`、`cargo clippy --all-targets`、
      `cargo test --features gpui-support`、`cargo clippy --all-targets
      --features gpui-support` 四条全绿零告警。验证：命令输出。
- [x] 7.2 共享代码加法审计：`git diff` 确认 `rpc/`/`session/`/`app/` 仅新增
      API、既有测试全绿（spec「共享代码仅加法演进」）。验证：diff 审阅记录。
      审阅记录（2026-11-24）：`git diff --stat` —— `rpc/` 零改动；`session/`
      仅 `blocks.rs` +6（clippy allow 属性）；`app/` +578/−69。删除行逐条核对
      = ①私有 `fn` 放宽为 `pub(crate)`/`pub` 的旧签名（13 处）、②`wrap` 私有
      断行核换中性类型（全部导出签名不变，双跑单测锁定）、③`view::render_header`
      装配内移为新 `header_line`（同源同输出）、④测试断言 1 处（alt_enter 改
      行级断言以兼容 serde_json preserve_order）。唯一生产调用点改动：
      `submit_as` → `dispatch_slash`（`slash_subset_only` 默认 false 时逐字节
      等价于原 `commands::dispatch`）。无已删公共 API；默认列 415 项测试全绿。
- [x] 7.3 文档：`Docs/tui-usage.md` 补 GPUI 模式启动方式，`Docs/roadmap.md`
      补记 GPUI 构建模式定位（归档时同步）。验证：文档就位且命令可跑。

## Workflow follow-up

- 归档前跑 `openspec validate add-gpui-support --strict`。
- 归档 change 并同步主规格（`gpui-agent-terminal` 落入 `openspec/specs/`）。
