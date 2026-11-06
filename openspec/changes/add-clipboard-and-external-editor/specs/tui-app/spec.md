# Spec Delta

## ADDED Requirements

### Requirement: 外部编辑器

`ctrl+g` MUST 挂起 TUI 并以继承 stdio 启动外部编辑器编辑当前输入：编辑器命令解析
对齐 pi（全局 settings.json 的 `externalEditor` → `$VISUAL`/`$EDITOR` → 默认
`nano`（Windows `notepad`）），临时文件为 `prompt.md` 且内容为挂起时的输入全文。
编辑器以退出码 0 结束时 MUST 读回内容（去 BOM 与末尾换行）替换输入框；启动失败或
非零退出 MUST 以黄条提示原因且**输入保持不变**。编辑器结束后 MUST 恢复 TUI（退出
编辑器前的会话状态不受影响）并全量重绘；输入区非空时 MUST 直接把当前内容带入
编辑器（不丢字）。

#### Scenario: 成功编辑回填

- **WHEN** 输入框有文本，按 ctrl+g，在编辑器中修改后保存退出（exit 0）
- **THEN** TUI 恢复显示，输入框内容替换为编辑后的全文

#### Scenario: 失败保持原样

- **WHEN** 编辑器命令不存在，或编辑器以非零码退出
- **THEN** TUI 恢复显示，黄条提示失败原因，输入框内容与挂起前一致

#### Scenario: 命令解析顺序

- **WHEN** settings.json 无 `externalEditor`、`$VISUAL` 与 `$EDITOR` 均未设置
- **THEN** 使用默认编辑器（nano/notepad）启动，不 panic

### Requirement: 剪贴板复制与粘贴

`ctrl+x` MUST 把最后一条 assistant 文本复制到终端剪贴板（OSC52，复用 `/copy`
通路），无内容时红条提示。`ctrl+v` MUST 按链式回退粘贴剪贴板：优先图片（按平台
后端读取图片字节）→ 写入临时文件并在光标处插入其路径；无图片则读取文本 → 光标处
插入；剪贴板为空则无操作。平台后端 MUST 覆盖：macOS 图片经 `osascript` JXA
（NSPasteboard）、文本经 `pbpaste`；Linux 经 `wl-paste`/`xclip`；Windows 经
PowerShell；后端不存在或读取失败 MUST 静默降级到下一环（图片失败→试文本→无操作），
不得弹错误、不得阻塞超过数秒。本 TUI 无选区特性，ctrl+x 不做"复制选区"分支（记入
文档差异）。

#### Scenario: 复制最后回复

- **WHEN** 会话已有 assistant 回复时按 ctrl+x
- **THEN** 该文本进入终端剪贴板并有成功提示；无回复时红条

#### Scenario: 图片粘贴为路径

- **WHEN** 系统剪贴板含图片时按 ctrl+v
- **THEN** 图片写入临时文件，输入框光标处插入该文件路径（不插入乱码字节）

#### Scenario: 文本回退与空剪贴板

- **WHEN** 剪贴板只有文本时按 ctrl+v，或剪贴板为空
- **THEN** 文本插入光标处；为空时无操作、无错误提示

#### Scenario: 后端缺失降级

- **WHEN** 平台图片后端命令不存在（如 Linux 无 wl-paste/xclip）
- **THEN** 静默尝试文本后端，仍失败则无操作，界面无红条
