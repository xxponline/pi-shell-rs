# Tasks

## 1. 浅色固定（可读性 pass）

- [x] 1.1 抽 `pin_light_theme(cx)` 并在 `window.rs::run` 的 `gpui_kit::init`
      之后调用 `Theme::change(ThemeMode::Light, None, cx)`（不装外观订阅）；
      验证：新增单测在测试窗断言调用后 `theme().mode == Light`
- [x] 1.2 把 terminal.rs 中 16 处硬编码 `rgb(0x..)` 收拢为 `gpui_agent`
      内一处浅色系命名常量块并全量替换；验证：
      `cargo test --features gpui-support` 全绿（既有颜色断言若有按新常量
      更新，语义断言不动）
- [x] 1.3 手动开窗肉眼确认三分区与补全菜单文字可读；验证：
      `cargo run --bin pi-shell-gpui --features gpui-support` 后无"深字深底"，
      并在 `Docs/tui-usage.md` GPUI 段补一句"独立窗口固定浅色模式"

## 2. 共享层可见性（仅加法）

- [x] 2.1 `src/app/mod.rs`：`mod overlay` → `pub(crate) mod overlay`、
      字段 `overlay` → `pub(crate)`（gpui_agent 只读，不新增方法不改行为）；
      验证：默认矩阵 `cargo test` 415 全绿 + `cargo clippy --all-targets`
      零告警，rpc/ session/ 零 diff

## 3. 浮层渲染骨架

- [x] 3.1 新建 `src/gpui_agent/overlay.rs`：`overlay_layer(app, size, cx)`
      入口 + 半透明遮罩 + 居中面板 chrome（主题 token 取色）+ 百分比居中
      纯函数；验证：居中几何纯函数单测 + 模块随 feature 门控编译
      （`cargo test --features gpui-support`）
- [x] 3.2 在 `GPUIAgentTerminal::render` 最后挂载浮层层（gpui 树序置顶，
      尺寸取整窗）；验证：测试窗置 `Overlay::Info` → 快照含标题，
      浮层关闭时无该层

## 4. 列表/查看型变体（逐个渲染 + 逐个冒烟）

- [x] 4.1 `list_panel` 助手 + ModelSelect（loading/items/cursor/计数/选中
      高亮）；验证：冒烟测试——打开→快照含标题与首项→下移→enter 应用→
      esc 关闭；行文对照 `app/terminal.rs` TUI 渲染
- [x] 4.2 ThinkingSelect 与 ScopedModels 冒烟（含 loading 态）；验证：
      各自打开/导航/关闭快照断言
- [x] 4.3 SessionSelect 冒烟（loading、cursor、删除两段式提示行，行文
      对照 TUI）；验证：打开→快照→导航→esc 关闭
- [x] 4.4 `viewer_panel` 助手 + Info（title/lines/loading）；验证：
      多行内容快照完整、loading 态不空面板
- [x] 4.5 Tree（rows/folded/filter/search/loading，行文对照 TUI）；
      验证：打开→快照含树节点→过滤输入态展示→esc 关闭
- [x] 4.6 Hotkeys 快捷键表渲染；验证：打开→快照含键位行→esc 关闭

## 5. 面板/确认型变体（逐个渲染 + 逐个冒烟）

- [x] 5.1 `form_panel` 助手 + Settings（cursor/条目行，对照 TUI 行文）；
      验证：打开→快照→导航→esc 关闭（Toggle 语义由共享层负责，渲染
      只读状态）
- [x] 5.2 Trust 与 ImportConfirm（两段确认 cursor/当前值）；验证：
      各自打开→快照→导航→esc 关闭
- [x] 5.3 ExtConfirm（confirm_selected 两态按钮）与 ExtInput
      （buffer/placeholder）；验证：push_dialog 注入→快照→esc 取消
      （reply=None 路径由共享层验证）
- [x] 5.4 ExtEditor（lines/row/col 光标位展示）；验证：push_dialog
      注入→快照含多行内容→esc 关闭
- [x] 5.5 扩展对话全链路 + 点击行为核对：RPC select/input/editor 事件 →
      `push_dialog` → 渲染 → 关闭时回复载荷发回；核对
      `app/terminal.rs` 对条目点击/遮罩点击的现行为并照抄（TUI 无此行为
      则 GPUI 不实现）；验证：单测断言链路 + 核对结论写入本任务备注

## 6. 键鼠路由（三区模型）

- [x] 6.1 抽路由纯函数 `route_keystroke(overlay_open, completion_open, ks)
      -> Route`（A 区全量接管 / B 区菜单导航 / C 区全局键+打开键 /
      其余交编辑器）；验证：三区×键型矩阵单测，含 ctrl+c 全局优先
- [x] 6.2 拦截层接线：浮层打开 → 全部键经
      `keystroke_to_keyevents` → `App::handle_key` → 同步 Textarea 镜像 →
      `stop_propagation`；浮层打开的滚轮 → 阻断 MessageScroller（TUI
      语义：忽略）；验证：单测断言浮层下输入区不被改、滚轮不滚消息区
- [x] 6.3 打开键 ctrl+l / alt+t 纳入 idle 区拦截（注释锚定
      `key_intent` 对应行，防漂移）；验证：fake-pi E2E——alt+t 开
      ThinkingSelect 快照可见 → esc 关；ctrl+l 开 ModelSelect
- [x] 6.4 语义序锁定：streaming×浮层双开测试——esc 关浮层先于 abort、
      ctrl+c 走全局清空；验证：新增双开测试断言命令序列

## 7. 斜杠入口扩容

- [x] 7.1 共享层新增谓词 `gpui_slash_supported`（查讯型 ∪ 可开浮层型，
      从 `commands.rs` 与分发代码枚举），GPUI 分发改用它替换原
      `GPUI_QUERY_SUBSET` 判定；验证：默认矩阵全绿（谓词仅 GPUI 引用，
      ratatui 行为零变化）
- [x] 7.2 无参 `/model`、`/thinking` 走 TUI 分发真开选择器（移除
      notice 用法桩，有参路径不变）；验证：更新
      `slash_query_commands_show_visible_feedback` 为开浮层断言 +
      `/model <id>` 异步 Info 仍为提示
- [x] 7.3 可开浮层命令表驱动冒烟：`/tree /settings /hotkeys /trust
      /session /import /scoped-models` 无参提交 → 对应浮层打开；
      验证：表驱动测试逐命令断言浮层变体匹配，已知但不支持的命令仍出桩

## 8. 集成与文档

- [x] 8.1 双矩阵全量回归：`cargo test` 与 `cargo test --features
      gpui-support` 全绿，`cargo clippy --all-targets` 两列零告警
- [x] 8.2 `Docs/tui-usage.md` GPUI 段更新：浮层键位/入口表、浅色固定、
      浮层下滚轮忽略说明；验证：文档与实现一致（键位表逐条对照测试）
- [x] 8.3 手动验收：开窗浅色可读、14 变体逐个打开关闭、alt+t /
      ctrl+l、浮层下滚轮与输入隔离、扩展对话回发；
      验证：真实窗口逐项勾选，问题回补任务

## Workflow follow-up

- 评审通过后 `/opsx-archive add-gpui-overlays`（同步 gpui-agent-terminal
  delta 到主规格）。

## 9. 验收返修：补全菜单可视窗口

- [x] 9.1 补全菜单滚动窗口跟随选中（`take(5)` 固定窗 → TUI 同款
      `start = selected-(max-1) min total-max`，复用 `COMPLETION_MAX_VISIBLE`
      单源常量）；验证：新增测试——候选 >5 时 ↓ 越窗后第 7 项入窗、首项
      出窗、计数行 `(当前/总数)` 同步（spec: slash-autocomplete「超出上限
      滚动」既有要求的 GPUI 违反返修）
