# Design

## Context

研究结论（见 proposal）：扩展/skill/模板的**执行在 pi 的 `prompt()` 内部**——
`_tryExecuteExtensionCommand`（先于压缩检查，流式/压缩中均可执行）→ 输入拦截 →
`_expandSkillCommand` → `expandPromptTemplate` → 未命中才发模型。我们已把未知 `/xxx`
当 prompt 发送，故执行链天然通。缺口在发现与我方发送语义。
列表经 `get_commands` RPC：`{commands:[{name, description, source:
"extension"|"prompt"|"skill", sourceInfo}]}`（skill 名带 `skill:` 前缀，**不**按
`enableSkillCommands` 过滤——该设置默认 true，键在全局 settings.json）；pi 补全侧规则：
与内置同名的扩展被过滤（`interactive-mode.ts:806`）、描述加源标签
（`prefixAutocompleteDescription`）、skill 列表按设置过滤（`:823`）。
我方现状：`submit_as` 压缩门控对所有文本一律排队（4b 语义）；补全候选仅内置 24。
动机见 proposal.md - Why。

## Goals / Non-Goals

**Goals:**
- 动态命令可发现（补全合并、标签、冲突/开关过滤）与发送语义对齐 pi（压缩期扩展例外）
- 拉取失败/空缓存全程降级可用，不引入错误弹层
- 合并与过滤为纯逻辑，可离线单测

**Non-Goals:**
- 扩展命令的 `getArgumentCompletions` 参数补全（RPC 不返回该能力，pi 独有）
- 模板 `argumentHint`（get_commands 不返回）
- 扩展执行结果的回执渲染（pi 侧扩展自行输出，经既有通知/消息通道呈现）
- 输入拦截（`_runInputHandlers` 改写输入）——那是 pi 进程内行为，发送即交还

## Decisions

### D1. 静默拉取，失败不打扰
启动探活成功后与 `/reload` 时经**自建静默线程**（`let _ = send_command`，不走
`spawn_async` 的 RequireSuccess——避免失败时红条违背"降级无错误"）拉 `get_commands`，
回填 `App.dynamic_commands: Vec<DynamicCommand>` 与 `skill_commands_enabled`
（同次读 settings.json 的 `enableSkillCommands`，缺省 true）。
`AsyncKind` 不新增（静默线程用现成 report 通道回 `UiEvent::Info`？**否**——直接不回报，
成功才回填：线程内拿到数据后经 `report_tx` 发一个新 `UiEvent::CommandsFetched` 轻量事件，
失败静默丢弃）。理由：缓存是发现性数据，迟到/缺失都不影响正确性。

### D2. 补全合并为纯函数
`autocomplete.rs` 增 `DynamicCommand { name, description, source }` 与
`merge_candidates(builtin, dynamic, skill_enabled) -> Vec<CompletionItem>`：
1. 过滤同名内置的动态项（pi 冲突规则）
2. `skill:` 项仅在 `skill_enabled` 时保留
3. 描述 = `[{source}] {description}`（source 字段即标签，等价 pi 的 sourceInfo 派生）
4. 顺序：内置 24 原序在前，动态按 get_commands 返回序在后
`suggestions()` 增参数（dynamic + skill_enabled）透传；App 每次刷新传缓存。
**拉取失败 = dynamic 空** → 合并结果即纯内置，天然满足降级场景。

### D3. 压缩门控的扩展例外
`submit_as` 压缩分支前插：
```rust
if compacting && command=="prompt" && is_dynamic_extension(message) {
    清空输入; 立即发 prompt（steer 标志按 phase 现算）; return;
}
```
`is_dynamic_extension`：取行首命令名（`/` 到首个空白）→ 查缓存 `source=="extension"`。
- skill/模板**不**例外 → 落入原排队（pi 的展开在压缩检查之后，立即发会被 pi 拒绝）
- 缓存为空（拉取未到/失败）→ 走排队（保守降级；pi 侧本可执行，延迟到压缩结束仍正确）
- 流式路径不变：steer prompt 到 pi 后扩展先执行（pi 语义，无需我方分支）

### D4. `/reload` 联动
现有 `cmd_reload`（空闲绿条 + 补全缓存失效）追加：重拉 `get_commands` + 重读
`enableSkillCommands`（与"配置已重新加载"语义一致）。

### D5. 文档落点
`Docs/slash-commands.md` 新增"动态命令（扩展/skill/模板）"一节（来源、压缩期语义、
过滤规则、降级行为）；tui-usage 补一句；`pi-tui-interaction.md` 无需改（#7 已含
"扩展命令随后续变更"字样——核对后更新）。

## Risks / Trade-offs

- **get_commands 与 pi 实际执行面不一致**（如扩展运行时动态注册/注销）→ 缓存只在
  启动/`/reload` 刷新；执行由 pi 按其**当前**注册表判定，列表陈旧只影响补全发现，
  不影响执行正确性（发送的是文本，pi 自己认不认识为准）
- **同名判定时机**：冲突过滤基于拉取时的内置表快照——内置表是编译期常量，无漂移
- **静默线程与退出竞态**：线程持有 report_tx clone，退出后 send 失败被 `let _` 吞掉
  （既有模式一致）
- **`enableSkillCommands` 只读全局**：与 settings 其余读取（sessionDir/doubleEscape）
  一致，项目级 `.pi/settings.json` 覆盖仍未做（文档注明）

## Open Questions

（无——执行链、RPC 形状、过滤规则均已在 pi 源码确认。）
