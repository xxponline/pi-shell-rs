# path-completion Specification

## Purpose

输入行中以 Tab 触发纯路径补全：按光标前的路径 token 列出当前目录或目标目录的
文件/目录候选（不依赖 `@` 前缀），支持 force 态持续重过滤下钻，使用户无需记忆
或手打完整路径即可用键盘浏览并插入路径。

## Requirements

### Requirement: Tab 触发与 token 提取

补全菜单未打开且焦点在输入区时，按下 Tab MUST 触发纯路径补全：按光标前文本
提取路径 token（token 以空白、`"`、`'`、`=` 或行首为边界；支持未闭合引号形态）；
光标前为空白或输入为空时 token 为空串，即列出 cwd 顶层。光标前文本以 `/` 起始
且其后无空白（斜杠命令上下文）时 MUST NOT 触发。菜单已打开时 Tab 为应用键
（复用 `slash-autocomplete` 键位约定），不重新触发。

#### Scenario: 空白前缀按 Tab 列当前目录

- **WHEN** 输入框内容为 `look `（光标在末尾空格后）或输入框为空，按 Tab
- **THEN** 打开补全菜单，列出 cwd 顶层文件与目录（目录带 `/` 后缀），输入框内容不变

#### Scenario: 路径 token 按 Tab

- **WHEN** 光标前为 `src/mai`，按 Tab
- **THEN** 打开补全菜单，列出 `src/` 下以 `mai` 为前缀的条目

#### Scenario: 斜杠上下文不触发

- **WHEN** 光标前文本为 `/mo`（行首斜杠、其后无空白），按 Tab
- **THEN** 不打开路径菜单（斜杠命令菜单由 `slash-autocomplete` 负责）

### Requirement: 候选生成

候选 MUST 由 readdir 生成，不调用 `fd`：按 token 的目录部分定位搜索目录（相对
cwd、支持 `~` 与绝对路径；以 `/` 结尾则列该目录，空 token 列 cwd），条目为
大小写不敏感 `startsWith` 前缀匹配；排序 MUST 为目录优先→字母序，且 MUST NOT
设条数上限。搜索目录不存在或不可读 MUST 返回空候选（静默，无错误提示）。

#### Scenario: 目录前缀匹配与排序

- **WHEN** token 为 `src/`，该目录含 `lib/`、`app.rs`、`main.rs`
- **THEN** 候选按目录优先、字母序列出（`lib/` 在前），总数不受 20 条截断

#### Scenario: 目录不存在静默收起

- **WHEN** token 为 `src/nope/`，该目录不存在，按 Tab
- **THEN** 不打开菜单，无任何错误提示

### Requirement: force 态生命周期

由 Tab 打开的菜单 MUST 处于 force 态：继续输入时菜单保持打开并按光标前新 token
重新过滤（可逐级下钻）；退格至光标前为空白时 MUST 保持打开并退回 cwd 列表；
候选为空、按 Esc 或应用选中项时 MUST 关闭。非 force 态（斜杠/`@` 自动触发）的
关闭语义不变。

#### Scenario: 下钻目录

- **WHEN** 空白 + Tab 打开 cwd 列表后，继续输入 `src/`
- **THEN** 菜单保持打开，候选刷新为 `src/` 目录内容

#### Scenario: 退格回空白不关菜单

- **WHEN** force 菜单打开且 token 为 `src`，按退格删至光标前为空白
- **THEN** 菜单保持打开，候选退回 cwd 顶层列表

#### Scenario: 零候选关闭

- **WHEN** force 菜单打开，继续输入使 token 指向不存在的目录
- **THEN** 菜单静默关闭，无错误提示

### Requirement: 单条自动应用

force 态下按 Tab 且当前 token 恰有 1 条候选时 MUST 直接把该候选应用到输入框，
MUST NOT 打开菜单。

#### Scenario: 恰一条候选直接插入

- **WHEN** 光标前为 `src/mai`，`src/` 下仅 `main.rs` 匹配，按 Tab
- **THEN** 输入变为 `src/main.rs`（无尾随空格），菜单未打开，光标在插入文本之后

### Requirement: Path 应用与键位

菜单键位 MUST 复用 `slash-autocomplete` 菜单约定（↑↓ 移动、Tab 应用、Esc 只关
菜单、继续输入即时重过滤）；回车 MUST 只应用选中项、MUST NOT 提交该行（对齐 pi：
仅斜杠菜单回车 fall-through 到提交）。应用时 MUST 以 Path 规则替换 token：
插入值不带 `@`、不加尾随空格、目录保 `/`、含路径分隔符或空白的值 MUST 自动包
`"` 引号；`~/`、`./`、绝对路径写法 MUST 保留；光标落位于插入文本之后；应用后
MUST 关闭菜单（单条自动应用除外）。

#### Scenario: 选择文件应用

- **WHEN** 菜单选中 `main.rs` 按 Tab
- **THEN** 输入中 token 被替换为 `src/main.rs`（无尾随空格），光标在其后，菜单关闭

#### Scenario: 选择目录续写

- **WHEN** 菜单选中目录 `lib/` 按 Tab
- **THEN** token 替换为 `src/lib/`，光标在 `/` 后，菜单关闭，可继续输入子路径

#### Scenario: 含空白路径自动引号

- **WHEN** token 为 `my dir/fo`，选中其中文件 `foo.txt` 按 Tab
- **THEN** 插入值为 `"my dir/foo.txt"`（带引号、无 `@`、无尾随空格）

#### Scenario: 回车只应用不提交

- **WHEN** Path 菜单打开、选中任意候选按回车
- **THEN** 候选应用到输入框、菜单关闭，该行不被提交（输入框保留文本）

### Requirement: 菜单主列展示

菜单主列 MUST 展示候选的 label（条目名，目录带 `/` 后缀），label 为空时回落
value；Path 候选 MUST 无描述列。斜杠与 `@` 菜单同样按 label 展示（斜杠
value==label 视觉不变；`@` 菜单主列由完整值变为条目名）。

#### Scenario: Path 菜单列样式

- **WHEN** token 为空，cwd 含 `src/`、`main.rs`
- **THEN** 菜单主列显示 `src/` 与 `main.rs`，无描述列，超出可见条数时显示 `(i/n)` 计数

#### Scenario: @ 菜单主列变为条目名

- **WHEN** `@` 菜单打开，候选 value 为 `@src/main.rs`、label 为 `main.rs`
- **THEN** 菜单主列显示 `main.rs`，描述列显示 `src/main.rs`
