# 斜杠命令与补全（本 TUI 实现状态）

对照 pi 内置的 24 个斜杠命令（`core/slash-commands.ts` BUILTIN_SLASH_COMMANDS）。
状态三类：✅ 实现 · ⚠️ 简化（行为弱于 pi，原因注明）· 🚫 提示桩（拦截 + 黄条原因，不发模型）。

## 斜杠补全菜单

输入行首 `/` 即弹出候选（全部 24 个命令，说明列 = `参数提示 — 说明`）：

| 键 | 行为 |
|---|---|
| 继续输入 | 模糊过滤（多词元，复刻 pi 打分），无匹配自动收起 |
| ↑↓ / PageUp/PageDown | 菜单导航（最多同时 5 条，超出 `(i/n)` 计数滚动） |
| Tab | 应用选中补全（`/thi` → `/thinking `，光标在空格后），不提交 |
| Enter | **先补全再立即提交该行**（对齐 pi） |
| Esc | 只关菜单，不清输入 |
| （自动） | 预选最佳匹配（精确 > 首个前缀）；`/model `、`/thinking ` 空格后出参数候选 |

参数候选数据惰性拉取一次（首次键入 `/model ` 时发 `get_available_models`，缓存后复用；
`/reload` 可使其失效重拉）。`@文件` 与路径补全**未实现**（pi 的 fd 集成，另一套机制）。

## 24 个命令

| # | 命令 | 状态 | 行为与差异 |
|---|---|---|---|
| 1 | `/settings` | ⚠️ 简化 | autoCompact / autoRetry 两开关（`set_auto_compaction` / `set_auto_retry`）+ `doubleEsc` 动作行（`tree→fork→none` 循环，写回 `~/.pi/agent/settings.json` 的 `doubleEscapeAction`，保留其它键、损坏拒绝写入，立即生效）+ 当前模型只读行。pi 完整设置（默认模型编辑、图片尺寸、skills 开关等）未做——RPC 无全量 settings 读写方法；**只读全局 settings.json**（pi 的项目级 `.pi/settings.json` 覆盖未实现，与 sessionDir 读取一致） |
| 2 | `/model [搜索]` | ✅ | 无参开选择器（同 ctrl+l）；精确匹配 `provider/id` 或 `id`（大小写不敏感）直接 `set_model` + 绿条；未命中回退选择器（对齐 pi）。参数补全 = 可用模型列表 |
| 3 | `/tree` | ✅ | 会话树：导航 / 折叠 / 5 种过滤（ctrl+o）/ Enter 分支（4b） |
| 4 | `/thinking [级别]` | ✅ | 无参开级别列表（同 alt+t）；精确匹配（大小写不敏感）直接应用；未知级别红条列出全部可用项。参数补全 = 可用级别 |
| 5 | `/scoped-models` | ⚠️ 简化 | 只读展示当前会话模型与全部可用模型。pi 可勾选写入 enabledModels（RPC 无 scoped models 方法，无法写入） |
| 6 | `/export [路径]` | ✅ | **无参默认 `<cwd>/session-<id>.jsonl`（差异：pi 无参默认导出 HTML）**；`.jsonl` 本地复制当前会话文件；`.html`（或其它非 jsonl 扩展名）走 `export_html` RPC；失败红条 |
| 7 | `/import <路径.jsonl>` | ✅ | 校验（存在 / .jsonl / 首行 session 头）→ 二次确认 → 复制进当前项目会话目录 → `switch_session` 切换并 `get_messages` 重建。差异：pi 经 `importFromJsonl` 保 ID/cwd 原位加载，我们按"复制 + 按路径打开"等价（已真机验证）；session header 的 `cwd` 必须存在（pi `assertSessionCwdExists`），失效时 pi 侧报错经红条呈现 |
| 8 | `/share` | 🚫 桩 | 需要网络上报（Radius artifact / GitHub gist），暂不支持 |
| 9 | `/bug` | 🚫 桩 | 需要网络上报到 Pi 开发者，暂不支持 |
| 10 | `/copy` | ✅ | `get_last_assistant_text` → OSC52 转义序列写终端剪贴板 + 绿条字符数；无回复红条。差异：pi 用系统剪贴板库；部分终端禁用 OSC52 时无法复制（无法回读验证） |
| 11 | `/name <名称>` | ✅ | `set_session_name`，状态行更新；无参用法提示（4b） |
| 12 | `/session` | ✅ | 会话信息面板（名称/消息数/tokens/费用）（4b） |
| 13 | `/changelog` | ✅ | 读 pi 包内 CHANGELOG.md **最新 `## ` 小节**逐行渲染进消息区（Notice 样式行）。差异：pi 用 markdown 组件渲染完整 changelog；路径按 `PI_BIN` 上溯 → 仓库内候选定位，缺失黄条提示；可用 `PI_RUST_TUI_CHANGELOG` 覆盖（测试用） |
| 14 | `/hotkeys` | ✅ | 键位表浮层（分组、↑↓ 滚动、Esc/回车关闭），数据即当前真实绑定 |
| 15 | `/fork` | ✅ | 与 `/tree` 同入口（树内 Enter 即分支；pi 的"选择历史用户消息"消息选择器简化为树）（4b） |
| 16 | `/clone` | ✅ | 克隆当前会话（4b） |
| 17 | `/trust` | ✅ | 读写 `~/.pi/agent/trust.json`（格式对齐 pi：`{ <规范cwd>: true }`，原子写），浮层显示当前状态 + 信任/清除两项。差异：pi 信任选择器还有询问策略选项，我们只做记录开关 |
| 18 | `/login [provider]` | 🚫 桩 | 项目约束**不做交互式登录**——依赖 `~/.pi/agent/auth.json` 预登录（用原生 pi 登录一次即可） |
| 19 | `/logout` | 🚫 桩 | 同上；移除凭据请直接编辑 `auth.json` |
| 20 | `/new` | ✅ | 新会话，消息区清空（4b） |
| 21 | `/compact [指令]` | ✅ | 手动压缩；压缩期间输入本地排队、结束自动补发（4b） |
| 22 | `/resume` | ✅ | 会话选择器（当前项目会话、两段式删除、Enter 切换）（4b） |
| 23 | `/reload` | ⚠️ 简化 | 空闲时重读本地配置（会话目录解析）+ 补全缓存失效 + 绿条提示；流式/压缩中黄条拒绝。pi 重载 keybindings/extensions/skills/prompts/themes——本 TUI 无这些运行时，RPC 无 reload 方法 |
| 24 | `/quit` | ✅ | 退出（4b） |

## 未命中与边界

- 未命中命令表的 `/xxx` 作为普通消息照常发给模型（对齐 pi）
- 单独 `/` 或全空白输入不触发分发
- 消息中部出现的 `/` 不触发补全菜单
- 彩蛋命令（`/arminsayshi`、`/dementedelves`）与 `/debug` 未实现（pi 内部调试项）
- **`!`/`!!` 不是斜杠命令**：`!cmd` 走 bash 执行（`!!` 不进上下文），见
  `tui-usage.md` 的 bash 模式一节；普通消息以 `!` 开头会被执行（pi 既有语义）
- **双击 Esc**（空输入、500ms 内）按 `doubleEscapeAction` 开树——默认 `tree`；
  `fork` 也开树（沿用 `/fork` = 树的简化，pi 的 fork 是"选择历史用户消息"）；
  设置在 `/settings` 的 doubleEsc 行修改

## 相关文档

- 键位全表：`Docs/tui-usage.md`
- 与 pi 的整体差异清单：`Docs/pi-tui-interaction.md`（差异 #7 已完成）
