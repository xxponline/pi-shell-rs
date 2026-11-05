# Tasks

## 1. 动态命令拉取与缓存

- [x] 1.1 `UiEvent::CommandsFetched` 轻量事件 + 静默拉取线程（`get_commands` 失败静默丢弃、不走 RequireSuccess 红条）；App 缓存 `dynamic_commands: Vec<DynamicCommand{name,description,source}>` 与 `skill_commands_enabled`（同次读 settings.json `enableSkillCommands`，缺省 true）；启动探活成功后触发；**验证**：单测喂 `CommandsFetched` 回填缓存、假 pi 响应解析（三类 source）、失败路径无通知产生

## 2. 补全合并

- [x] 2.1 `autocomplete.rs`：`DynamicCommand` 结构 + 纯函数 `merge_candidates(builtin, dynamic, skill_enabled)`（同名内置过滤动态项、`skill:` 按开关过滤、描述加 `[source]` 标签、内置原序在前）；`suggestions()` 扩展参数透传；**验证**：单测覆盖冲突过滤、开关过滤、标签格式、空缓存=纯内置（降级场景）、顺序
- [x] 2.2 App 接线：`refresh_completion` 传缓存与开关；补全菜单对动态命令的模糊过滤/预选/Tab 应用与内置一致；**验证**：单测键入 `/my` 出现 `[extension]` 候选、Tab 应用为 `/my-ext `、`skill:` 在开关关闭时不可见

## 3. 分发语义

- [x] 3.1 `submit_as` 压缩门控前的扩展例外：行首命令名查缓存 `source=="extension"` → 立即发 prompt（清空输入、不入队）；skill/模板与未知文本维持排队；**验证**：单测三分支（扩展立即发、skill 排队、缓存空时排队）+ 压缩期扩展发送的 prompt 含正确载荷
- [x] 3.2 `/reload` 联动：重拉 `get_commands` + 重读 `enableSkillCommands`；**验证**：单测 reload 后缓存与开关被刷新（假 pi 第二次 get_commands 进日志）

## 4. 文档

- [x] 4.1 `Docs/slash-commands.md` 新增"动态命令（扩展/skill/模板）"一节（来源/压缩期语义/过滤规则/降级）；tui-usage 补充；`pi-tui-interaction.md` 核对 #7 相关表述并更新；**验证**：三文档与实现一致、无过时"无扩展命令"表述

## 5. 验收与门禁

- [x] 5.1 假 pi PTY：`get_commands` 返回扩展/skill 项 → 补全出现带标签候选并可 Tab 应用；压缩期间提交扩展命令立即发送（日志无排队提示、stdin 有 prompt）；skill 候选在 `enableSkillCommands=false` 时不出现；**验证**：脚本断言全 True（ALL OK）
- [x] 5.2 门禁：`cargo test` 全绿、`cargo clippy --all-targets` 零告警、`openspec validate add-dynamic-slash-commands --strict` 通过；**验证**：三命令输出无 error/告警

## Workflow follow-up

- 提交：`git add -A && git commit`
- 归档：`/opsx-archive add-dynamic-slash-commands`（同步 tui-app 与 session-management delta）
