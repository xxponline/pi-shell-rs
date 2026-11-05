# pi TUI 操作逻辑整理（复刻基准）

> 从 pi 源码梳理的完整操作逻辑，作为我们复刻交互行为的对照基准。
> 来源：`core/keybindings.ts`、`modes/interactive/components/custom-editor.ts`、
> `modes/interactive/interactive-mode.ts`、`packages/tui/src/keybindings.ts`、`packages/tui/src/tui.ts`。
> 配套文档：[`roadmap.md`](./roadmap.md)、[`tui-usage.md`](./tui-usage.md)。

## 1. 按键分发模型（三层优先级）

```
原始字节 → crossterm 解析
    │
    ▼
┌─ 第1层: 焦点组件 ─────────────────────────────────────┐
│ tui.ts: focusedComponent.handleInput(raw)             │
│ · 选择器/弹层打开时 → 焦点在弹层，编辑器完全不收键      │
│ · 关闭后焦点回编辑器                                   │
└──────────────────┬───────────────────────────────────┘
                   ▼ 焦点在编辑器时
┌─ 第2层: CustomEditor.handleInput 优先级链 ────────────┐
│ 1. 扩展注册快捷键 (onExtensionShortcut) —— 最高优先    │
│ 2. autocomplete 打开时的 pageUp/pageDown → 交给菜单    │
│ 3. ctrl+v 粘贴 (app.clipboard.pasteImage)             │
│ 4. esc (app.interrupt)：autocomplete 打开时让给菜单    │
│ 5. ctrl+d (app.exit)：仅编辑器为空时；有字时落入编辑器  │
│    （作为 delete-char-forward）                        │
│ 6. 历史键 (historyPrevious/Next) 优先于 app 动作       │
│    —— 用户可把 ctrl+p 绑回历史而不触发模型循环          │
│ 7. 其余已注册 app 动作（按注册顺序遍历 actionHandlers）│
│ 8. 都不匹配 → super.handleInput                       │
└──────────────────┬───────────────────────────────────┘
                   ▼
┌─ 第3层: Editor 基类 ──────────────────────────────────┐
│ tui.editor.* 编辑操作（光标/删除/yank/undo…）          │
│ tui.input.* ：enter=提交、shift+enter/ctrl+j=换行、    │
│               tab=自动补全                             │
│ autocomplete 菜单交互                                  │
└──────────────────────────────────────────────────────┘
```

**要点**：动态行为靠 `onEscape` / `onCtrlD` / `onSubmit` 三个可替换回调实现——
同一按键在不同状态下行为不同；弹层打开时焦点转移，编辑器整层退出分发链。

## 2. 状态相关的核心操作语义（复刻的精髓）

### Enter（提交，`onSubmit`）

```
trim 空 → 忽略
├─ 本地 slash 命令（见 §4）→ 本地执行，清空输入
├─ "!cmd" / "!!cmd" → bash 模式（!! = 不进上下文）
│    └─ 已有 bash 在跑 → 警告 "Press Esc to cancel it first"，保留文本
├─ compaction 中 → 排队（steer 语义；扩展命令除外，立即执行）
├─ 流式中 → prompt(text, {streamingBehavior:"steer"})   ← 打断重定向
└─ 空闲 → 正常 prompt → addToHistory
```

### Esc（动态 handler，按状态分派）

```
流式中 → abort + 把排队消息恢复回编辑器
bash 在跑 → abortBash
bash 模式（输入以 ! 开头）→ 清空输入、退出 bash 模式
编辑器为空 → 双击 esc（500ms 内）→ 按设置打开 tree/fork（默认 none）
空闲且有文本 → 什么都不做（pi 的 esc 不清空、不退出）
```

### Ctrl+C / Ctrl+D / Ctrl+Z

```
ctrl+c (app.clear)  → 单击=清空编辑器；500ms 内双击=退出程序
ctrl+d (app.exit)   → 仅编辑器为空时退出（有字时=向前删字符）
ctrl+z              → 挂起后台
```

### 队列操作

```
alt+enter (followUp)：
  流式中 → prompt(followUp) 排队，等当前生成完再执行
  compaction 中 → 排队
  空闲时 → 等同普通 Enter
alt+up (dequeue)   → 取回【全部】排队消息到编辑器
                      （空队列 → 状态栏 "No queued messages to restore"）
```

### 编辑器辅助

```
ctrl+g (external) → 打开外部 $EDITOR 编辑当前输入
ctrl+x (copy)     → 复制选区或最后一条 assistant 消息
ctrl+v (paste)    → macOS 粘贴文件 / 图片 / 文本
输入以 ! 开头     → 进入 bash 模式（边框变色，onChange 检测）
```

## 3. 键位全表

### 应用层（`APP_KEYBINDINGS`，macOS 默认）

| 键 | 动作 | 键 | 动作 |
|---|---|---|---|
| esc | interrupt/abort | ctrl+c | clear（双击退出） |
| ctrl+d | exit（空输入） | ctrl+z | suspend |
| shift+tab | thinking 级别循环 | ctrl+s | 保存 thinking 级别 |
| ctrl+p | 模型循环下一个 | shift+ctrl+p | 模型循环上一个 |
| ctrl+l | 模型选择器 | ctrl+o | 工具输出展开 |
| ctrl+t | thinking 块显隐 | ctrl+g | 外部编辑器 |
| ctrl+x | 复制消息 | alt+enter | 排队 follow_up |
| alt+up | 取回队列 | ctrl+v | 粘贴文件/图片 |
| ctrl+n | 会话命名过滤 | | |

**会话树内**（弹层上下文，同名键复用）：ctrl+p/s/d/r、shift+l/t、
ctrl+a/t/u/l/o、shift+ctrl+o、alt+left/right（折叠/展开）。

**Windows/Linux 差异**：模型循环上一个=alt+p、followUp=ctrl+q、dequeue=alt+q、
paste=alt+v、undo=ctrl+z、search=ctrl+f 等（`windowsKeybindings` 分支；本项目仅按
macOS 实现）。

### 编辑器层（`tui.editor.*`，Emacs 风格）

| 类别 | 键 |
|---|---|
| 光标 | ↑↓、left/ctrl+b、right/ctrl+f、home/ctrl+a、end/ctrl+e、alt+left/right（词）、ctrl+] / ctrl+alt+]（跳转）、pageUp/pageDown |
| 删除 | backspace、delete/ctrl+d、ctrl+w/alt+backspace（词）、alt+d（向前词）、ctrl+u（到行首）、ctrl+k（到行尾） |
| 历史 | historyPrevious/Next（**默认未绑定**，留空供用户绑） |
| yank | ctrl+y 粘贴、alt+y yank-pop |
| undo | ctrl+- |

### 输入 / 选择 / 全屏层

| 类别 | 键 |
|---|---|
| 输入 | enter=提交、shift+enter/ctrl+j=换行、tab=补全、ctrl+c=复制选区 |
| 选择列表 | ↑↓、pageUp/Down、enter=确认、esc/ctrl+c=取消 |
| 全屏(alt-screen) | ctrl+shift+f 搜索、enter/ctrl+g 下一个、shift+enter 上一个、esc 关闭、ctrl+up/down 上/下一条 prompt、pageUp/Down、ctrl+home/end 顶/底 |

### 可配置性

用户可在 `~/.pi/agent/keybindings.json` 覆盖任意绑定（含旧名迁移表
`cursorUp → tui.editor.cursorUp` 等）。

## 4. 本地 slash 命令（提交时拦截，不发给模型）

`/settings` `/model` `/thinking` `/scoped-models` `/session` `/new` `/resume`
`/tree` `/fork` `/clone` `/compact` `/name` `/export` `/import` `/share`
`/bug` `/copy` `/login` `/logout` `/trust` `/reload` `/debug` `/hotkeys`
`/changelog` `/quit`（+ 两个彩蛋命令）

—— `/thinking` 无参即开级别选择器（我们用 alt+t 替代的原因）。

## 5. 与当前实现的差异（复刻待办清单）

| # | pi 行为 | 我们现状 | 优先级 |
|---|---|---|---|
| 1 | **Enter 流式中 = steer**（prompt 带 streamingBehavior） | ✅ 已对齐（流式 prompt 自动携带 steer） | ~~高~~ 已完成 |
| 2 | **ctrl+c = 清空，双击退出**；esc 空闲不退出 | ✅ 已对齐（q 退化为普通字符，退出=ctrl+d 空/双击 ctrl+c） | ~~高~~ 已完成 |
| 3 | esc 流式 = abort **+ 恢复队列回编辑器** | ✅ 已对齐（abort + clear_queue 回填） | ~~中~~ 已完成 |
| 4 | alt+enter 空闲时 = 普通 Enter | ✅ 已对齐（空闲=prompt，流式=follow_up） | ~~中~~ 已完成 |
| 5 | **双击 esc**（500ms）开 tree/fork（可设置） | 无 | 低（依赖 4b） |
| 6 | `!cmd` bash 模式（边框变色、!! 排除上下文） | 无 | 中 |
| 7 | 本地 slash 命令（26 个） | 无（斜杠设施未建） | 分批（/thinking 已用 alt+t 替代） |
| 8 | 编辑器 Emacs 全套（词移动/删除、yank、undo、历史） | ✅ 已对齐（多行+光标+词行删除+kill-ring/yank+undo；历史 pi 默认未绑定→不做） | ~~高~~ 已完成 |
| 9 | ctrl+g 外部编辑器、ctrl+x 复制、ctrl+v 粘贴图片 | 无 | 中 |
| 10 | shift+enter 换行（编辑器内多行） | ✅ 已对齐（shift+enter/ctrl+j 换行，输入区动态增高） | ~~高~~ 已完成 |
| 11 | 扩展快捷键最高优先级 | 未实现 | 低 |
| 12 | 选择列表分页（pageUp/Down）、历史键可覆盖 | 选择器仅 ↑↓ | 低 |
| 13 | Windows 键位分支 | 仅 macOS | 忽略（个人项目） |

## 6. 已实现且对齐的部分

- Enter 提交（空输入忽略）、流式中断（esc → abort）
- 模型选择器（ctrl+l）与循环（ctrl+p / shift+ctrl+p）、thinking 循环（shift+tab）
- thinking 块折叠切换（ctrl+t）、工具输出展开（ctrl+o）
- follow_up 排队（alt+enter）与取回（alt+up，按 pi 语义修正为 clear_queue 全部取回）
- 扩展 UI 9 种 method、read/bash/edit/write/grep/find/ls 工具卡片
- 空闲退出（我们为 esc/q，pi 为 ctrl+d 空输入——差异 #2）

## 7. 后续建议归属

- ~~差异 **#1、#2、#3、#4、#8、#10** → "操作逻辑对齐" change~~
  → ✅ 已由 change `align-interaction-semantics` 完成（真机 13 项验收通过）。
- 差异 **#5、#7** 依赖 4b（会话树）与斜杠命令设施，随 4b 立项。
- **#6 bash 模式**依赖 `bash`/`abort_bash` RPC（已有命令），可独立小 change。
- 其余低优先项按需后置。
