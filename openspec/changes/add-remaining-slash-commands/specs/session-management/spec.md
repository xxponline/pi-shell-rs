# Spec Delta

## ADDED Requirements

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
