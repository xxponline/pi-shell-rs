# Tasks — 树内搜索

## 1. 纯逻辑（tree.rs）

- [x] 1.1 `searchable_text(entry)`：对照 pi `getSearchableText` 分支清单（role/content、bash 命令、session_info 名、model/thinking 变更、custom、label 等，按节点实际字段）拼接；单测覆盖各 entry 类型与缺字段容错
- [x] 1.2 `walk()` 增加 `search` 参数：空白分词 → 全 token AND、大小写不敏感子串；顺序 = 过滤模式 → 搜索 → 折叠裁剪；逐节点过滤（祖先不命中保留命中的后代行）；单测：分词 AND、大小写、后代保留、叠加顺序、空查询等价现状

## 2. 浮层状态与键序

- [x] 2.1 `Overlay::Tree` 增加 `search` 字段（打开清空）；`overlay_key` Tree 臂：无修饰可打印字符追加、`\x7f` 删、Esc 先清后关；导航/过滤/折叠/回车键语义不变；单测：键序三态（输入/清空/关闭）、修饰键不入查询、Esc 优先级
- [x] 2.2 查询变化后选择保持（可见则原索引、否则钳制、空结果安全）；单测：命中集缩小/扩大/清空三种迁移

## 3. 视图与状态行

- [x] 3.1 view.rs 树浮层：标题区展示 `过滤模式 · search:"…" (n 命中)`；零命中黄字提示；单测：状态行格式、零命中提示

## 4. 真机验收与文档

- [x] 4.1 假 pi PTY：打开树 → 输入查询断言行集变化与状态行 → Backspace 恢复 → Esc 清查询再关面板 → ctrl+o 叠加过滤；文档：`Docs/tui-usage.md` 树键位段 + `Docs/pi-tui-interaction.md` 后续批次更新
- [x] 4.2 门禁：`cargo test` 全绿、`clippy --all-targets` 零告警、`openspec validate add-tree-search --strict`
