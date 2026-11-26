# Proposal

## Why

`align-gpui-card-rendering` 把 GPUI 的**内容语义**对齐到了 TUI，但**输入与滚动面**
仍是 P0 残缺：整页/半页/prompt 跳转/顶底、取回排队、换模型、thinking 循环、强制路径
补全、`ctrl+x` 复制等键全部错落进输入编辑器（`route_keystroke` 白名单没接）；消息区
没有滚动条；斜杠命令仍被"查讯 ∪ 浮层"子集闸门拦着、其余已知命令只给桩。这些语义在
共享层（`key_intent` / `handle_key` / `commands::dispatch`）早已全量存在且有测试——
GPUI 缺的只是路由接线与滚动面，现在补上即可完成"功能语义对齐 TUI"的输入侧一章。

## What Changes

- **键路由补线**（`route_keystroke` 白名单扩展，共享语义零改动）：
  `alt+up` 取回排队、`ctrl+p` / `ctrl+shift+p` 模型循环、`shift+tab` thinking 级别
  循环、`tab` 强制路径补全、`ctrl+x` 复制最后回复；`ctrl+j` 换行按内建编辑器实测
  定去留（原生支持则仅验证，否则记差异）。
- **滚动命令架构（本变更的咽喉）**：新增第三路由态——消息区滚动键直达内建虚拟
  滚动状态（`MessageScrollerState`），**滚动真相归 scroller**（`App.scroll` 保持
  TUI 专属，不引入双真相）；`PageUp/PageDown` 整页、`shift+Page*` 半页、
  `ctrl+home/end` 绝对顶/底（底 = 恢复贴底跟随）、`ctrl+↑/↓` 按消息边界跳转
  （消息 = 用户/assistant 条目，与 TUI `msg_seq` 规则同源）。页语义精度
  （条目级 ±N vs 行级像素偏移）以一次 spike 实测定案，决策规则写入 design。
- **消息区滚动条**：启用 gpui-component 内建滚动条（内容超视口可见、可拖动
  定位）；行为以 kit 组件为准——与 TUI 瞬态显隐/悬停加粗语义的差异记录在案，
  不做逐语义对齐。
- **斜杠命令解闸**：移除 `gpui_slash_supported` 子集门，全部已知命令与 TUI 同
  分发（查讯/浮层/本地执行/桩同款）；前置条件是 OSC52/复制出口在窗口形态可用
  （`ctrl+x`、`/copy`、`/export` jsonl 共用同一通路，写启动终端的 stdout）。
- **明确不做（记差异，不进本变更）**：鼠标选区复制（独立笔）、外部编辑器
  `ctrl+g`（窗口形态无终端可挂起，设计题单拎）、`ctrl+v` 图片粘贴链（文本粘贴
  按内建行为验证）、wheel 加速表手感（接受 kit 内建滚动手感）、主题系统（D 线）、
  alt-screen 搜索 `ctrl+shift+f`（路线图挂账，TUI 侧亦未做）、`ctrl+d` 退出语义
  （窗口形态走窗口关闭键，不映射到按键）。

## Capabilities

### New Capabilities

（无——全部为既有能力的需求演进）

### Modified Capabilities

- `gpui-agent-terminal`：全局键位覆盖扩展（新增键组与不落输入编辑器的断言）、
  新增消息区滚动命令（scroller 单真相）、新增滚动条可见行为、新增窗口形态
  剪贴板复制出口；`斜杠命令查讯型子集（v1）` 改为全集分发。

## Impact

- `src/gpui_agent/terminal.rs`：`route_keystroke` 白名单 + 第三路由态 +
  滚动条开关 + 滚动命令处理；`src/gpui_agent/` 新增滚动桥模块（页语义 spike 落点）
- `src/app/commands.rs`：`gpui_slash_supported` 闸门移除（共享分发不变）
- `src/app/`（仅验证不动语义）：`key_intent`/`handle_key`/复制出口在窗口形态的
  复用确认；共享层遵守"仅加法演进"，既有测试必须全绿
- 依赖面：gpui-component `MessageScrollerState`/内建 `Scrollbar`（已有依赖，无新增）
- 验证：双矩阵 `cargo test` + `--features gpui-support`、PTY 冒烟清单
  （对照 `Docs/tui-usage.md` 键位表与 `pi-tui-interaction.md`）
