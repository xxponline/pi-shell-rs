# Design

## Context

pi 侧（已读源码）：`ctrl+g` → `handleOpenExternalEditor`：命令解析
`settings.externalEditor` → `$VISUAL/$EDITOR` → 默认 `nano`/`notepad`；
`ui.stop()` 挂起 → `editInExternalEditor`（tmpdir `pi-editor-*/prompt.md`、写入当前
输入、spawn 继承 stdio、exit 0 读回去 BOM/末尾换行，否则 "prompt unchanged"）→
`ui.start()` + 全量重绘；`ctrl+x` → `app.message.copy`（我们已有 `/copy` 的 OSC52
通路）；`ctrl+v`（Windows `alt+v`）→ `app.clipboard.pasteImage`，粘贴处理器链为
文件路径 → 图片（`getNativeClipboard` 原生模块 / Linux·Windows 命令回退）→ 文本。
我们侧：`ctrl+g/x/v` 均未绑定（key_intent 无冲突）；`terminal.rs` 有 kitty 协议
Push/Pop；键盘线程是阻塞 `ct_event::read()`；`EditorBuffer` 只有 `insert(char)` 与
尾部 `append_block`，无光标处多字符插入。JXA 读 macOS 图片剪贴板已真机探测可行
（NSPasteboard 类型枚举正常），`pbpaste` 可用。动机见 proposal.md - Why。

## Goals / Non-Goals

**Goals:**
- 三个键位行为对齐 pi（命令解析顺序、回填/失败语义、粘贴回退链、静默降级）
- 挂起/恢复不丢输入、不残留终端状态（raw、alt-screen、kitty 协议、键盘线程）
- 平台后端零第三方依赖（子进程命令 + 超时）

**Non-Goals:**
- pi 的"复制选区"分支（本 TUI 无选区，文档注明）
- 剪贴板**文件列表**粘贴（复制 Finder 文件的 file-URL 分支；我们链为 图片→文本）
- 外部编辑器的多文件/光标定位（pi 同样只给 prompt.md）
- 括号粘贴（bracketed paste）事件处理——终端逐字投递已可用，`ctrl+v` 是显式入口

## Decisions

### D1. 三个键走 KeyIntent，阻塞执行在事件循环
`KeyIntent::{ExternalEditor, CopyMessage, PasteClipboard}`（key_intent 表加三臂，
仅 `ctrl+g/x/v` 且无修饰冲突）。`CopyMessage`/`PasteClipboard` 是快速操作，直接在
`apply_intent` 分支同步执行；**`ExternalEditor` 经 `UiEvent::OpenExternalEditor`
转到事件循环**——只有事件循环持有 `ratatui::Terminal`，恢复后必须 `terminal.clear()`
重置其内部缓冲（否则 diff 渲染对着被编辑器涂花的屏幕算差异，必然花屏）。阻塞事件
循环是刻意的：外部编辑器是模态操作（pi 同样 stop UI）。

### D2. 挂起/恢复原语（terminal.rs 新增）
`suspend_for_child()`：Pop kitty 增强 flags → LeaveAlternateScreen → disable_raw_mode；
`resume_from_child()`：enable_raw_mode → EnterAlternateScreen → 重新 Push kitty flags
（启动时探测结果复用既有静态标记）→ 由调用方 `terminal.clear()`。
子进程：按空白切分命令（pi 同：不走 shell）、`stdio(inherit)`、cwd 继承当前目录、
`status()` 阻塞等待；退出码 0 → 读回临时文件替换输入，否则黄条提示（输入不变）；
临时目录无论成败 finally 清理（对齐 pi 的 `rmSync(..., force)`）。

### D3. 键盘线程加"暂停闸门"（关键）
编辑器子进程与我们的键盘线程**同读一个 stdin**——阻塞 `read()` 会偷走 vim 的按键。
改造：键盘线程改 `ct_event::poll(150ms)` 循环 + 共享 `Arc<AtomicBool> paused`：
暂停期间 **poll 到数据也不 read**（字节留给子进程），空转退避；恢复后继续正常读。
`OpenExternalEditor` 臂置位、子进程退出后复位（panic/异常路径靠 `restore_now`
兜底不清闸门——进程随后本就退出）。**理由**：比杀/重建线程简单且无状态迁移风险。

### D4. 剪贴板后端（新 `src/app/clipboard.rs`）
纯子进程探测，全部带超时（图片 3s / 文本 2s），失败返回 `None` 由调用方降级：

| 平台 | 图片 | 文本 |
|---|---|---|
| macOS | `osascript -l JavaScript`（内嵌 JXA：NSPasteboard 找 png/jpeg/webp/gif 类型 → base64 输出，**已真机探测**） | `pbpaste` |
| Linux | `wl-paste --list-types/--type` → 否则 `xclip -t TARGETS` | `wl-paste` → `xclip` → `xsel` |
| Windows | PowerShell `Get-Clipboard -Raw`（文本）/ .NET Image 保存（图片走 `powershell` 脚本） | PowerShell |

粘贴链：`read_image()` → 临时文件 `pi-paste-{ts}.{ext}` 插入路径 → `read_text()` →
光标处插入 → 皆空则无操作（**全程不弹红条**，spec 的静默降级）。
JXA 输出格式：首行 `TYPE`、其余行 base64；解析失败视为无图片。

### D5. 光标处多字符插入（EditorBuffer 新增 `insert_text`）
按行拆分：普通段逐段插入、`\n` 走既有 `newline()` 语义（拆行+光标落新行行首），
整体走一次 undo 快照（`push_undo` 一次而非每字符）。粘贴与外部编辑器回填共用
（外部编辑器用 `replace` 整体替换——pi 的 setText 语义；粘贴用 `insert_text`）。

### D6. ctrl+x 复用 `/copy` 通路
`CopyMessage` 分支直接调既有 `cmd_copy`（`get_last_assistant_text` → OSC52 →
绿条/红条），零新逻辑；差异（无选区）记文档。

### D7. 文档与键位表
`hotkeys_rows()` 补三行（ctrl+g/x/v）；`tui-usage.md` 补三个键位与外部编辑器命令
解析；`pi-tui-interaction.md` 差异 #9 标记完成（注明选区复制与文件粘贴两项差异）。

## Risks / Trade-offs

- **macOS JXA 自动化权限**：首次调用可能弹"允许 osascript 控制"授权 → 读取失败
  静默降级到文本（文本 pbpaste 也可能被同样拦截 → 无操作）；文档提示一次授权即可
- **键盘线程 poll 改造影响现有键路**：poll(150ms) 与 read() 语义等价（都有超时
  退避），既有 PTY 测试全量回归兜底
- **kitty flags 恢复**：子进程若自行改过终端模式（vim 换屏），我们恢复序列以
  "EnterAlternateScreen + raw + Push" 覆盖；个别终端残留态由 resume 后首次全量
  重绘吸收
- **超时杀不干净的后端**（如 PowerShell 冷启 >5s）：超时即 kill 子进程返回 None，
  用户感知为"这次没粘上"，可重试
- **粘贴路径含空格**：插入的是原始路径文本（pi 同），由用户/模型自行处理引用

## Open Questions

（无——三键语义、命令解析顺序、粘贴回退链均已在 pi 源码确认；JXA 已真机探测。）
