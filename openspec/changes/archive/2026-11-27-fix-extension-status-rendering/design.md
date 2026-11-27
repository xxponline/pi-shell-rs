# Design

## Context

setStatus 的消费链路：`handle_extension_request`（`src/app/mod.rs`）把 `statusText` 原样存入单槽 `Option<String>`（`statusKey` 被丢弃），`view::header_line`（`src/app/view.rs`）把它整段包进一个洋红 `Span` 交给 ratatui。ratatui 的 `styled_graphemes` 只过滤含控制字符的 grapheme——`\x1b` 字节被丢，但 `[38;2;0;255;136m` 等可打印残留原样上屏。pi 侧对照：pi-tui 是"字符串透传"架构（序列原样进终端由终端着色，`utils.ts` 的 `ansiCodeLength` 等只负责在量宽/折行时跳过序列），而 ratatui 是"结构体"架构（颜色属于 `Style`，ANSI 由后端最后生成）——透传模型在我们这没有落点，缺的恰是 pi 不需要的反向转换：`ANSI → Vec<Span>`。双前端（ratatui `render_header`、GPUI `status_grid_line`）同源于 `view::header_line`，改一处两边受益。动机见 proposal.md。

## Goals / Non-Goals

**Goals:**

- 状态文本零转义残留，扩展设置的颜色（真彩/256 色/属性）保真上屏
- statusKey 分格语义与 pi 对齐（并存、字母序、独立清除）
- 解析为纯函数，无 TTY 可单测；不新增依赖
- 截断以可见文本宽度为准

**Non-Goals:**

- 不复刻 pi-tui 的字符串透传管线（`wrapTextWithAnsi` / `visibleWidth` / `AnsiCodeTracker` ~500 行）——ratatui 已在结构化行上做量宽/折行/截断，移植字符串版等于维护两套平行逻辑
- 不引入通用 ANSI 引擎 crate（`ansi-to-tui` 等）；不覆盖光标移动、清屏等非 SGR 控制序列的语义（只跳过）
- 不改 RPC 收发与 `notify`/`setWidget` 等其它 method 的行为
- 不把扩展状态移到 footer 独立行（布局维持现状：状态行尾段）

## Decisions

### D1：手写最小子集 SGR 解析器，而非引 crate 或剥离

解析目标 `fn parse_ansi_spans(text: &str, base: Style) -> Vec<Span>`。覆盖插件实际产出与 theme 预置 SGR 的子集：

- 颜色：`38/48;2;r;g;b` → `Color::Rgb`；`38/48;5;n` → `Color::Indexed`；`30-37/90-97`、`40-47/100-107` → 8/16 色
- 属性：`1/2/3/4` → BOLD/DIM/ITALIC/UNDERLINED；`22/23/24/39/49` 各自 reset；`0`（或空参数）→ 回到 base Style
- 未知序列：按 pi `ansiCodeLength` 的边界语义跳过（`ESC[` 扫描至 `m/G/K/H/J`，`ESC]`/`ESC_` 扫描至 BEL 或 `ESC\`），不显示、不 panic

备选与否决：**剥离**（~30 行，行为确定）——颜色全丢，TPS 分档色（红/橙/绿/青）与 dim 前缀消失，与 pi 观感差距固化；**引 `ansi-to-tui`**——新增依赖、需适配 ratatui 0.30 API，而所需子集手写仅 ~100-150 行且可精确对齐 pi 的跳过语义。

### D2：`status_text: Option<String>` → `BTreeMap<String, String>`

- 存储：`BTreeMap` 天然给出 pi 要求的 key 字母序（pi 是 `Map` + `localeCompare` 排序后 join）
- 缺省 `statusKey` 的请求落入固定默认格（如 `""`），保旧 fixture/简易客户端兼容
- `statusText: null` → `remove(key)`，只清该格
- 渲染入口：按 key 排序取值 join（以空格分隔）成状态段文本

备选与否决：`HashMap`——无序，显示顺序随迭代抖动；保留单槽 + 优先级——不解决多扩展并存与独立清除。

### D3：解析时机在渲染装配层（`header_line`），存储保持原始字符串

`header_line` 拼 `Vec<Span>` 时对状态段调用 `parse_ansi_spans`。备选与否决：**入库时解析**（存 `Vec<Span>`）——`App` 状态持有 ratatui 类型、双前端 GPUI 路径还要再转换，且截断宽度依赖渲染期的可用宽度，入库解析拿不到。逐次状态更新（插件默认每 token 一次）对几十字符做一次解析，开销可忽略。

### D4：截断按可见宽度

状态段截断改为对"剥离序列后的可见文本"计算宽度（解析器顺带产出可见文本，或截断发生在 spans 序列上按 `Span::width()` 累计），替代现有按 `chars().count()` 的 `truncate`——后者把转义残留计入，导致可见文本被提前截断。零宽字符（U+200B）经 ratatui `cell_width()==0` 自然不占位，解析时直接剔除以免进入宽度计算。

## Risks / Trade-offs

- [theme 预置 SGR 的实际形态随终端色模式变化（可能 `38;5;n`、标准 `3x` 或真彩）] → 解析器按参数分派覆盖全部三类；未知形态走"跳过"兜底，最坏情况是该段无色而非乱码
- [手写解析器边界 bug（截断在序列中间、畸形参数）] → 单测覆盖：截断于序列中点、空参数、超长参数、缺终止符、OSC 8 超链接；解析器保证任意输入不 panic
- [GPUI 与 ratatui 行为分叉] → 解析与装配都收敛在 `view::header_line` 单点，`status_grid_line` 不重复实现
- [statusKey 改 BTreeMap 触及既有测试断言（`app.status_text == None` 等）] → 同步更新 `src/app/mod.rs` 内 setStatus 相关测试；行为变化已在规格场景中声明，非静默破坏
- [仅覆盖 SGR，若扩展发非 SGR 控制序列（清屏/光标）] → 按 D1 只跳过不执行——与"状态栏是文本"的模型一致，pi 的 footer 同样只透传 SGR 着色

## Migration Plan

无数据迁移、无协议变更。单 PR 落地：解析器 + 存储结构 + 渲染装配 + 测试同步；回滚即 revert。`Docs/rpc-protocol-notes.md` 字段表已含 `statusKey`，无需文档变更。

## Open Questions

（无——statusKey 槽位命名、默认格 key 等微小选择不改变规格、方案或任务拆分，实现时按 D2 就近定。）
