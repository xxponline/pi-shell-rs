# session-management Specification

## Purpose

提供会话生命周期与导航的用户可观察行为：列出并切换历史会话、浏览会话树并分支、新建/重命名/删除会话、手动压缩，以及切换后界面如实反映新会话内容——这些能力经斜杠命令入口触发，是恢复与组织长期工作的核心导航面。

## Requirements

### Requirement: 会话列表与切换

`/resume` MUST 打开会话选择器，列出会话目录（`PI_CODING_AGENT_SESSION_DIR` > settings.sessionDir > `~/.pi/agent/sessions`）下的全部 `.jsonl` 会话，按修改时间倒序，每项显示"显示名称（缺失时用首条消息预览）+ 修改时间"，当前会话有可见标记。选中一项 MUST 发送 `switch_session` 并在成功后用 `get_messages` 重建状态，使消息区立即显示目标会话内容；esc 取消不发送命令。目录不存在或为空 MUST 显示空态提示且不崩溃；单个损坏文件 MUST 被跳过而不影响列表。

#### Scenario: 列出并切换会话

- **WHEN** 用户执行 /resume，在列表中选中一个历史会话回车
- **THEN** 发送 switch_session，成功后消息区替换为目标会话的对话内容，界面回到空闲

#### Scenario: 取消与空态

- **WHEN** 会话选择器打开时按 esc，或会话目录为空
- **THEN** 取消时不发送任何命令；空目录显示提示且界面不损坏

#### Scenario: 损坏文件被跳过

- **WHEN** 会话目录中混入非 JSONL 或首行非法的文件
- **THEN** 列表跳过该文件，其余会话正常显示

### Requirement: 会话树浏览与分支

`/tree` MUST 打开会话树浮层（数据 `get_tree`）：按父子结构缩进展示条目、标记当前 leaf；↑↓ 导航，折叠/展开子树；`ctrl+o` MUST 在 5 种过滤模式间循环（default / no-tools / user-only / labeled-only / all，语义对照 pi 的树过滤：default 隐藏记账类条目与无文本纯工具消息、no-tools 隐藏工具结果、user-only 仅用户消息、labeled-only 仅带标签条目、all 全部），当前模式在标题可见；回车 MUST 对选中节点发送 `fork(entryId)` 并在成功后重建状态（界面反映分支后内容），esc 取消。

#### Scenario: 树展示与导航

- **WHEN** 会话存在多级分支时打开 /tree
- **THEN** 条目按层级缩进显示、当前 leaf 有标记，↑↓ 移动选择、折叠键收起/展开子树

#### Scenario: 过滤模式循环

- **WHEN** 用户多次按 ctrl+o
- **THEN** 模式按 default→no-tools→user-only→labeled-only→all 循环，标题与可见条目随之变化

#### Scenario: 分支到选中节点

- **WHEN** 树中选中一个历史节点回车
- **THEN** 发送 fork，成功后消息区显示该分支内容并出现分支成功通知；esc 则不发送任何命令

### Requirement: 新建与重命名会话

`/new` MUST 发送 `new_session`，成功后清空消息区进入新会话（cancelled 时无变化并给出提示）；`/name <名称>` MUST 发送 `set_session_name` 并使状态行会话名更新，`/name` 无参 MUST 提示用法而不发送命令。执行成功的斜杠命令 MUST 清空输入框。

#### Scenario: 新建会话

- **WHEN** 用户输入 /new 回车
- **THEN** 发送 new_session，消息区清空、状态行会话名更新，输入框清空

#### Scenario: 重命名会话

- **WHEN** 用户输入 /name 新名字 回车
- **THEN** 发送 set_session_name，状态行显示新名字，输入框清空；/name 无参时仅显示用法提示

### Requirement: 删除会话

会话选择器内 `ctrl+d` MUST 以两段式确认删除选中会话（首次进入确认态，再次按下执行；esc 取消确认）：拒绝删除当前活跃会话并提示；执行前 MUST 校验目标路径 canonicalize 后位于会话目录内且以 `.jsonl` 结尾，先尝试 `trash` 命令、失败回退 `unlink`；成功后从列表移除并提示，失败显示错误且列表不变。

#### Scenario: 两段式确认删除

- **WHEN** 在会话选择器中对非活跃会话按一次 ctrl+d 再按一次
- **THEN** 会话文件被删除、列表刷新并提示成功；首次按下后按 esc 则取消

#### Scenario: 活跃会话与路径安全

- **WHEN** 对当前活跃会话按 ctrl+d，或目标路径不在会话目录内
- **THEN** 拒绝删除并提示原因，不发生任何文件操作

### Requirement: 手动 compact 流程

`/compact [自定义指令]` MUST 发送 `compact`（可带 customInstructions）；进行中界面 MUST 显示压缩状态（compacting 标志与通知），完成 MUST 显示结果通知（压缩前后 token 量），业务失败（如会话过小）MUST 以红色错误通知呈现。**compaction 期间提交输入 MUST 本地排队而不立即发送**（直接发送会被 pi 拒绝），压缩结束后按序补发，补发的 prompt MUST 携带 `streamingBehavior:"steer"` 以兼容进入流式的情形。**例外**：命中动态扩展命令的输入 MUST 立即发送（pi 的扩展命令执行先于其压缩检查，排队反而会延迟执行）；skill 与模板命令无此例外，仍走排队。

#### Scenario: 触发与完成通知

- **WHEN** 用户执行 /compact 回车
- **THEN** 界面出现压缩进行中状态，完成后显示含 token 压缩量的结果通知，输入框清空

#### Scenario: 压缩期间排队补发

- **WHEN** compaction 进行中用户输入消息并回车
- **THEN** 消息进入本地队列并可见（不立即发送、不出现红色错误），压缩结束的通知后该消息被自动发出

#### Scenario: 压缩期扩展命令立即执行

- **WHEN** compaction 进行中用户输入已注册的扩展命令 /my-ext 回车
- **THEN** 该行立即作为 prompt 发送（不入本地队列、不出现排队提示），输入框清空

#### Scenario: 压缩失败可见

- **WHEN** compact 返回业务错误（如会话过小）
- **THEN** 红色错误通知显示原因，界面回到空闲
### Requirement: 会话信息面板

`/session` MUST 打开信息浮层，显示 `get_session_stats` 的基本字段（会话名、消息数、token 用量、费用），数据异步到达前显示加载态；esc 关闭；请求失败显示错误通知。

#### Scenario: 查看会话信息

- **WHEN** 用户执行 /session 回车
- **THEN** 出现会话信息浮层显示名称/消息数/token/费用，esc 关闭返回主界面

### Requirement: 切换后界面反映新会话内容

任何改变当前会话的操作（switch_session / fork / clone / new_session）成功后，界面 MUST 反映目标会话：消息区替换为目标会话的对话（经 `get_messages` 重建），进行中流式与旧队列状态被重置，失败（cancelled）时界面保持不变并提示。

#### Scenario: 切换后消息区重建

- **WHEN** 成功切换到含多轮对话的会话
- **THEN** 消息区显示该会话的用户与 assistant 消息（含工具调用块），phase 回到空闲

#### Scenario: 取消操作不改状态

- **WHEN** switch_session 或 fork 返回 cancelled=true
- **THEN** 消息区与状态保持原样，并显示取消提示

### Requirement: 会话导出

`/export [路径]` MUST 支持三种形式：无参数时把当前会话导出为默认路径下的 `.jsonl` 文件；参数以 `.jsonl` 结尾时复制当前会话文件到该路径；参数以 `.html` 结尾时经 `export_html` RPC 生成 HTML。成功后状态提示输出路径，失败（含路径不可写、RPC 错误）以红条显示原因且不崩溃；路径缺省与默认文件名 MUST 记录在 `Docs/slash-commands.md`。

#### Scenario: 默认与指定 jsonl 导出

- **WHEN** 用户输入 /export 回车，或 /export /tmp/x.jsonl 回车
- **THEN** 当前会话被复制为对应文件，状态提示输出路径；会话内容与状态不变

#### Scenario: HTML 导出与失败

- **WHEN** 输入 /export /tmp/x.html，或导出路径不可写
- **THEN** 前者经 export_html 生成 HTML 并提示路径；后者红条显示失败原因

### Requirement: 会话导入

`/import <路径.jsonl>` MUST 无参数时显示用法提示；带参数时 MUST 先弹出确认（显示目标路径），确认后把该 JSONL 导入当前项目会话目录并 `switch_session` 切换过去（切换成功后经 `get_messages` 重建消息区），取消确认或导入失败时会话与界面 MUST 保持原样并给出提示。

#### Scenario: 导入并切换

- **WHEN** 用户输入 /import /tmp/old.jsonl，确认后执行
- **THEN** 文件进入当前项目会话目录，界面切换到该会话并重建其历史，出现成功提示

#### Scenario: 取消与失败

- **WHEN** 确认弹层中取消，或源文件不存在/非法
- **THEN** 不发生任何文件写入与切换，消息区保持原样并显示对应提示
