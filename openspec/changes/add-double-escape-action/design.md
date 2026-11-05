# Design

## Context

差异表 #5 的 pi 实现位于 `interactive-mode.ts` 的 `defaultEditor.onEscape`：优先级为
流式（回填+中止）→ bash 运行（本 TUI 无 bash 模式，见 #6）→ bash 模式清空（同上）→
**仅输入为空**时做双击判定（`Date.now() - lastEscapeTime < 500`），按
`getDoubleEscapeAction()`（`"tree"|"fork"|"none"`，默认 `"tree"`，settings.json 键
`doubleEscapeAction`，设置选择器取值序 `tree→fork→none`）触发树/分支选择器。
我们侧：空闲 Esc 目前走 `key_intent → Ignore`（对齐单击语义），会话树（4b）与
`/settings` 简化浮层（上一 change）已就绪；`resolve_sessions_dir` 已示范全局
`~/.pi/agent/settings.json` 的读取。动机见 proposal.md - Why。

## Goals / Non-Goals

**Goals:**
- 双击窗口、优先级、三态动作与 pi 逐条对齐（500ms、空输入、仅空闲判定）
- 设置可读可写，写回不破坏 settings.json 其它键，对双击立即生效
- 判定与设置读写均为无 TTY 纯逻辑，可单测

**Non-Goals:**
- 项目级 `.pi/settings.json` 覆盖（与现有 sessionDir 读取一致，只读全局；记入文档差异）
- fork 动作的"消息选择器"形态（沿用 fork=树的既有简化，与 `/fork` 相同）
- bash 模式相关的 Esc 分支（#6 未做，不预留）
- pi 设置选择器的其它键（treeFilterMode 等）

## Decisions

### D1. 判定挂在 handle_key 的"空闲 Esc"出口，不改 key_intent 表
`key_intent(Esc, idle)` 保持 `Ignore`（其断言测试不动）；在 `handle_key` 中
overlay/补全分支**之后**拦截 `Esc + Ignore` 走 `handle_double_escape()`。
**理由**：浮层与补全的 Esc 已在前序分支 return（天然优先且不计入窗口），
流式 Esc 走 `Abort` 不落入此分支——优先级由分发顺序表达，无需新意图枚举。

### D2. 时间戳只在"空闲且输入为空"的 Esc 上更新
对齐 pi：输入非空的 Esc 既不触发也不刷新窗口。`last_escape: Option<Instant>`
字段，触发后清空；用 `Instant`（单调钟）而非 pi 的 `Date.now()`——语义相同、
不受系统时钟调整影响。窗口常量 `500ms` 与 pi 一致。

### D3. 设置以文件为真源，每次判定现读（无缓存）
`read_double_escape_action()` 现读 `~/.pi/agent/settings.json`：键缺失/非字符串/
非法值 → `Tree`；文件不存在 → `Tree`。**理由**：settings 浮层写回后需"立即对双击
生效"（spec），文件即真源可免去缓存失效逻辑；单次 Esc 一次小文件读取成本可忽略。
写入：文件存在但**不可解析**时**拒绝写入并红条**（不覆盖用户可能可修复的文件）；
可解析则取对象、改键、tmp+rename 原子写回（保留其它键）。
路径可注入（`read_..._from(path)` / `write_..._from(path)` 纯函数 + 默认路径包装），
测试用临时文件，不碰真实 HOME（吸取上一 change 的 env 竞态教训）。

### D4. fork 动作 → 打开会话树
pi 的 fork 打开"选择历史用户消息"选择器；我们已有 `/fork` = 树的简化（4b 决定），
双击 fork 沿用同一入口，保持一致。记入 `Docs/slash-commands.md`。

### D5. settings 浮层加第三行 doubleEsc
`Overlay::Settings` 增加 `double_esc: Option<String>` 字段（打开时现读文件填充）；
可切换行 0..2（Down 夹取 2），Enter → `Toggle(2)` → 循环 `tree→fork→none` 并写回；
模型行与提示行顺延只读。写回失败红条且行值回读文件（不显示未生效值）。

### D6. 读取函数归属
设置读写放 `src/app/settings_io.rs`（新小模块：enum + read/write + 默认路径），
与 `sessions.rs` 的 settings 读取并列——**不**塞进 `mod.rs`（已 4800+ 行）。
`resolve_sessions_dir` 不动（不引入共享解析，避免扩散改动）。

## Risks / Trade-offs

- **settings.json 损坏** → 读：降级默认 tree；写：拒绝 + 红条（D3），不覆盖
- **pi 进程同时写 settings.json**（如原生 pi 改设置）→ 我们"读-改-写"非加锁，
  极小窗口可能丢对方新键——pi 自身用 markModified 也非事务；风险可接受，
  原子 rename 缩短窗口
- **双击窗口与浮层关闭竞态**（Esc 关浮层后 500ms 内再 Esc）→ 浮层 Esc 不计入
  窗口（D1 分发顺序保证），行为与 pi（选择器层先消费）一致
- **测试时间敏感**（500ms 窗口）→ 用 50ms 内连按（必触发）与 600ms 间隔
  （必不触发）两端断言，留 4 倍余量，避免慢机器抖动

## Open Questions

（无——设置取值序、窗口时长、优先级均已在 pi 源码确认。）
