# Proposal

## Why

差异表 #9：pi 提供三个编辑/剪贴板快捷键——`ctrl+g` 挂起 TUI 打开外部编辑器（编辑
prompt 后回填）、`ctrl+x` 复制最后一条回复、`ctrl+v` 粘贴剪贴板（图片→临时文件路径
插入，否则文本插入）。我们的 TUI 三者全无：长 prompt 无法用 vim/nice 编辑器打磨、
复制要打 `/copy`、粘贴完全不支持。

## What Changes

- **`ctrl+g` 外部编辑器**：命令解析对齐 pi（settings.json `externalEditor` →
  `$VISUAL`/`$EDITOR` → 默认 `nano`/`notepad`）；挂起 TUI（退出 alt-screen、恢复
  raw）→ 以继承 stdio 启动编辑器于临时 `prompt.md`（含当前输入）→ 退出码 0 读回
  （去 BOM 与末尾换行）替换输入框，否则黄条提示"prompt 不变" → 恢复 TUI 并全量重绘
- **`ctrl+x` 复制**：最后一条 assistant 文本 → OSC52 剪贴板（复用 `/copy` 通路）；
  无内容红条。pi 的"优先复制选区"分支不适用（本 TUI 无选区特性，记入文档差异）
- **`ctrl+v` 粘贴**：链式回退对齐 pi——图片（平台后端）→ 写临时文件、插入路径；
  否则文本（平台后端）→ 光标处插入；剪贴板为空 → 无操作。平台后端：
  macOS 图片走 `osascript` JXA（NSPasteboard，零第三方依赖，已真机探测可行）、
  文本 `pbpaste`；Linux `wl-paste`/`xclip`；Windows PowerShell（与 pi 的命令回退一致）
- **键位与文档**：`key_intent` 三键接线；`/hotkeys` 键位表补三行；
  `Docs/tui-usage.md`、`Docs/pi-tui-interaction.md` 差异 #9 完成

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `tui-app`: ADDED"外部编辑器"需求（命令解析、挂起/恢复、成功回填与失败保持）；
  ADDED"剪贴板复制与粘贴"需求（ctrl+x 复制语义、ctrl+v 图片/文本链、平台后端与降级）

## Impact

- `src/app/mod.rs`：`KeyIntent::{ExternalEditor, CopyMessage, PasteClipboard}` 分发、
  三个执行函数（外部编辑器挂起链、复制复用 copy 通路、粘贴链）
- `src/app/terminal.rs`：挂起/恢复原语（LeaveAlternateScreen + raw 切换 + 重绘标记）
- 新 `src/app/clipboard.rs`：平台后端探测与图片/文本读取（子进程超时、失败回退）
- `src/app/overlay.rs`：hotkeys 表 +3 行
- `src/app/settings_io.rs`：`externalEditor` 读取（复用同一 settings.json）
- 文档两处；不改 RPC 协议
