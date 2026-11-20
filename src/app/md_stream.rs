//! 流式 markdown 尾槽行级 memo（design D1）：块区切分 + 完结行前缀冻结。
//!
//! 定位：易失尾槽（in-flight Text 块）每帧渲染的等价记忆化——输出与
//! [`markdown::parse_to_lines`] 全量路径**逐字节等价**（parity 测试钉死），
//! 成本从 O(尾条) 降为 O(新增行)（代码 fence）/ O(开放块区)（散文）。
//!
//! 机制（design D1 spike 回写）：
//! - **代码 fence**：内容行按完结行推进 `(ParseState, HighlightState)`
//!   缓存（[`CodeLineState`]，与全量路径同一逐行算法）；开放行每帧
//!   Clone 状态后单独重排（不推进）。
//! - **散文块区**：块区 = 以空白行/fence 为界的源区间；纯段落简单行
//!   按完结行冻结，开放行所在后缀每帧单行重排；行内标记（`*_~` 等）
//!   停止冻结，块起始样式行（列表/引用/标题/主题线/setext/缩进代码）
//!   失效回块区首整区重排——setext 回溯、紧表嵌套错位、松紧表 retro
//!   等 pulldown 事件流怪癖由整区重排吸收（失败安全）。
//! - **永久回退**：reference 定义/空列表项/空标题（parse_to_lines 跨块
//!   泄漏怪癖）→ 整槽交回全量路径。
//!
//! 失效矩阵（D4）：宽度变化整体重建；theme/flags 变化由上层 LayoutKey
//! 失效丢弃整个 memo。

use pulldown_cmark::HeadingLevel;
use ratatui::style::Style;
use ratatui::text::Line;

use crate::app::markdown::{self, CodeLineState};
use crate::app::theme::TuiTheme;
use crate::app::wrap::{self, HitKind};

/// 流式 Text 块的行级 memo（易失尾槽 design D1：等价记忆化）。
pub(crate) struct StreamMemo {
    /// 渲染行 = 冻结段 `[..frozen_len]` + 热段 `[frozen_len..]`（热段每帧重建）。
    lines: Vec<Line<'static>>,
    /// 逐行 wrap 计数（宽度键控；宽度变化整体重建）。
    counts: Vec<u32>,
    /// 逐行命中 meta（Text 块恒空组；与 lines 等长供分块借出）。
    meta: Vec<Vec<HitKind>>,
    /// 冻结段合计屏行数（热段另计，免每帧全量求和）。
    frozen_total: usize,
    /// 全部行合计屏行数。
    total: usize,
    /// 冻结行数。
    frozen_len: usize,
    /// 冻结源前缀字节数（`text[..frozen_src]` 对应 `lines[..frozen_len]`）。
    frozen_src: usize,
    /// 已消费完结行的源位置（下一完结行起点）。
    pos: usize,
    /// 当前开放块区（None = 块区间/文本尾）。
    chunk: Option<Chunk>,
    /// 待决断的尾随空白行起点（松紧表分割需 1 行前瞻）。
    pending_blank: Option<usize>,
    /// 本轮可逐行冻结的完结简单行数（帧末从热段前缀冻结）。
    pending_freeze: usize,
    /// pending_freeze 覆盖的源终点（字节）。
    pending_freeze_end: usize,
    /// 最后一条完结简单行的源终点（等下一行判据才能冻结：后继行为空白
    /// 或缺失时留热，段落尾空行归属才不旁落）。
    waiting: Option<usize>,
    /// 冻结点的样式栈顶（标题样式跨块泄漏的承接值，design D1）。
    carry: Style,
    /// 永久全量回退（跨区泄漏构造出现过）。
    fallback: bool,
    /// 计数键控宽度（变化整体重建）。
    width: u16,
    /// 防改写校验快照（源首 8 字节 + 冻结尾 8 字节）。
    guard_head: [u8; 8],
    guard_tail: [u8; 8],
    /// 免重排短路签名：（文本长度，冻结后尾部哈希）。
    sig: (usize, u64),
    /// 签名是否已建立。
    sig_valid: bool,
}

/// 开放块区状态（design D1 块区上下文）。
struct Chunk {
    /// 块区源起点（字节）。
    start: usize,
    /// 块区起点的冻结行数（整区重排失效回退点）。
    frozen_at_start: usize,
    /// 代码 fence 行级状态（散文区为 None）。
    fence: Option<FenceState>,
    /// 区内出现过列表/引用行（fence 分割守卫）。
    saw_container: bool,
    /// 区内出现过列表项行（松紧表/序号续接分割守卫）。
    saw_item: bool,
    /// 逐行冻结可用（纯段落简单行区间）。
    freeze_ok: bool,
    /// 尾块为段落（setext/中断/懒续行判据）。
    trailing_para: bool,
    /// 块区起点的样式栈顶（整区重排的续渲初值）。
    carry_at_start: Style,
    /// 块区内推进的样式栈顶（标题泄漏累计）。
    carry_now: Style,
}

impl std::fmt::Debug for StreamMemo {
    /// 调试摘要（不展开行内容；mem 为近似字节）。
    ///
    /// 输入: f — 格式化器；输出: 摘要字段写入 f
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamMemo")
            .field("frozen_len", &self.frozen_len)
            .field("frozen_src", &self.frozen_src)
            .field("pos", &self.pos)
            .field("total", &self.total)
            .field("fallback", &self.fallback)
            .field("width", &self.width)
            .finish()
    }
}

/// 代码 fence 行级推进状态（design D1：HighlightState 按完结行前缀推进）。
struct FenceState {
    /// fence 标记字符（` 或 ~）。
    ch: char,
    /// 开始行标记长度（闭合行须 ≥ 此长度）。
    run: usize,
    /// 逐行高亮状态。
    code: CodeLineState,
}

/// 源行分类（块区推进判据）。
enum LineClass {
    /// 空白行（仅空格/制表符）。
    Blank,
    /// fence 起始行（``` / ~~~ 3+）。
    FenceOpener { lang: String, ch: char, run: usize },
    /// ATX 标题行；empty = 标题无内容（全量路径渲染 0 行的怪癖）；
    /// level = 标题级别（样式泄漏推进用）。
    AtxHeading { empty: bool, level: HeadingLevel },
    /// 主题分割线（3+ 同符标记，非 setext 形态）。
    Thematic,
    /// 引用行（> 起始）。
    QuoteLine,
    /// 列表项行；interrupt = 能否打断段落（非空且（无序或 start==1））；
    /// empty = 标记后无内容。
    Item { interrupt: bool, empty: bool },
    /// setext 下划线候选（=+ / -+）；also_thematic = 同时匹配主题线；
    /// empty_dash_item = "-" / "- " 形（块区间时为空列表项）；
    /// level = 成标题时的级别（= → H1，- → H2）。
    Underline {
        also_thematic: bool,
        empty_dash_item: bool,
        level: HeadingLevel,
    },
    /// reference 定义候选（`[id]: ...`）。
    Def,
    /// html 构造（`<` + 字母等）：parse_to_lines 的 Html 事件只压 cur 不
    /// 冲刷，会与前后块内容并行成行（跨区泄漏怪癖）→ 全量回退。
    Html,
    /// 普通段落行（无行内标记，可逐行冻结）。
    Simple,
    /// 行内标记行（`*_~` `[]` 反引号/尾随反斜杠）：停止逐行冻结即可——
    /// 后缀整段重排可吸收跨行配对，无需失效。
    InlineMark,
    /// 块样式/不确定构造（缩进代码/html/懒续行块样式）：独立渲染有块级
    /// 歧义，须失效回块区首整区重排；closes_para = 是否同时闭合段落。
    BlockLike { closes_para: bool },
}

impl StreamMemo {
    /// 创建空 memo（宽度键控计数）。
    ///
    /// 输入: width — 视口宽度（`max(1)` 归一；变化触发整体重建）
    /// 输出: 冻结/热段皆空的 memo
    pub(crate) fn new(width: u16) -> Self {
        Self {
            lines: Vec::new(),
            counts: Vec::new(),
            meta: Vec::new(),
            frozen_total: 0,
            total: 0,
            frozen_len: 0,
            frozen_src: 0,
            pos: 0,
            chunk: None,
            pending_blank: None,
            pending_freeze: 0,
            pending_freeze_end: 0,
            waiting: None,
            carry: Style::default(),
            fallback: false,
            width: width.max(1),
            guard_head: [0; 8],
            guard_tail: [0; 8],
            sig: (0, 0),
            sig_valid: false,
        }
    }

    /// 渲染流式 Text 块（含 memo 推进）：冻结段复用 + 热段每帧重排。
    ///
    /// 输入: text — 块累计源文本（协议只增；改写时防御性整体重建）；
    ///       theme — 注入配色（fence 高亮主题）；width — 视口宽度
    ///       （变化整体重建，D4 失效矩阵）
    /// 输出: `(渲染行, 逐行 wrap 计数, 逐行 meta)`——三者等长，与
    ///       `markdown::parse_to_lines(text)` 逐字节等价；错误情形：
    ///       跨区泄漏构造（reference 定义/空列表项/空标题）转永久全量
    ///       回退后仍逐字节等价（失败安全）
    pub(crate) fn render(
        &mut self,
        text: &str,
        theme: &TuiTheme,
        width: u16,
    ) -> (&[Line<'static>], &[u32], &[Vec<HitKind>]) {
        let width = width.max(1);
        if width != self.width {
            let fallback = self.fallback;
            *self = Self::new(width);
            self.fallback = fallback;
        }
        if self.fallback {
            self.fallback_render(text, theme, width);
            return (&self.lines, &self.counts, &self.meta);
        }
        if !self.prefix_ok(text) {
            // 源被改写（协议外行为）：丢弃 memo 全量重建。
            self.reset();
        }
        // 免重排短路：同一帧内 refresh 二次调用（显式 + draw 内部）零成本。
        let tail = &text[self.frozen_src.min(text.len())..];
        let sig = (text.len(), fnv1a(tail));
        if self.sig_valid && self.sig == sig {
            return (&self.lines, &self.counts, &self.meta);
        }
        // 热段重建（先截掉上一帧热段，冻结段只增/失效）。
        self.lines.truncate(self.frozen_len);
        self.counts.truncate(self.frozen_len);
        self.meta.truncate(self.frozen_len);
        self.total = self.frozen_total;
        self.advance(text, theme, width);
        if self.fallback {
            self.fallback_render(text, theme, width);
            return (&self.lines, &self.counts, &self.meta);
        }
        if self.open_line_precheck(text, theme, width) {
            return (&self.lines, &self.counts, &self.meta);
        }
        self.render_hot(text, theme, width);
        self.guard_head = snapshot(text, 0);
        self.guard_tail = snapshot(text, self.frozen_src.saturating_sub(8));
        self.sig = (text.len(), fnv1a(&text[self.frozen_src.min(text.len())..]));
        self.sig_valid = true;
        (&self.lines, &self.counts, &self.meta)
    }

    /// 全部行合计屏行数（分块借出的 `total` 原料）。
    ///
    /// 输入: 无；输出: counts 合计（增量维护）
    pub(crate) fn total(&self) -> usize {
        self.total
    }

    /// 借出三元组（渲染行/逐行计数/逐行 meta，等长）供分块出借。
    ///
    /// 输入: 无（&self 借用）
    /// 输出: `(lines, counts, meta)` 切片三元组
    pub(crate) fn parts(
        &self,
    ) -> (&[Line<'static>], &[u32], &[Vec<HitKind>]) {
        (&self.lines, &self.counts, &self.meta)
    }

    /// 是否已转全量回退（观测钩子；测试核对快路径真实生效）。
    ///
    /// 输入: 无；输出: true = 永久回退中
    #[allow(dead_code)]
    pub(crate) fn is_fallback(&self) -> bool {
        self.fallback
    }

    /// 记忆化内存足迹（结构 + 行内容近似；基准注记用）。
    ///
    /// 输入: 无；输出: 字节近似（含 String 堆内容长度，不含分配器开销）
    #[allow(dead_code)]
    pub(crate) fn memory_bytes(&self) -> usize {
        let mut total = std::mem::size_of_val(self);
        for l in &self.lines {
            total += std::mem::size_of_val(l)
                + l.spans
                    .iter()
                    .map(|s| std::mem::size_of_val(s) + s.content.as_ref().len())
                    .sum::<usize>();
        }
        total += self.counts.capacity() * std::mem::size_of::<u32>();
        total += self.meta.capacity() * std::mem::size_of::<Vec<HitKind>>();
        total
    }

    /// 防改写校验：冻结前缀的首/尾片段与快照一致且长度足够。
    ///
    /// 输入: text — 本次渲染的源文本
    /// 输出: true = 前缀未被改写（in-flight 文本按协议只增）
    fn prefix_ok(&self, text: &str) -> bool {
        if text.len() < self.frozen_src {
            return false;
        }
        if self.frozen_len == 0 {
            return true;
        }
        let head_ok = text.len() < 8 || text.as_bytes()[..8] == self.guard_head;
        let tail_ok = self.frozen_src < 8
            || text.as_bytes()[self.frozen_src - 8..self.frozen_src] == self.guard_tail;
        head_ok && tail_ok
    }

    /// 整体重置（保留宽度与 fallback 标记）。
    ///
    /// 输入: 无；输出: 无返回值；副作用为清空冻结/热段与块区状态
    fn reset(&mut self) {
        let width = self.width;
        let fallback = self.fallback;
        *self = Self::new(width);
        self.fallback = fallback;
    }

    /// 瞬时全量渲染（开放行暂定分类命中怪癖）：只出本帧输出，不改写
    /// 冻结指针/块区状态（开放行下帧可能改判回快路径）。
    ///
    /// 输入: text — 块源文本；theme — 注入配色；width — 视口宽度
    /// 输出: 无返回值；副作用为整体替换 lines/counts/meta/total（冻结
    ///       指针不动，下一帧从冻结段重建热段）
    fn transient_full_render(&mut self, text: &str, theme: &TuiTheme, width: u16) {
        self.lines = markdown::parse_to_lines(text, theme);
        self.counts = self
            .lines
            .iter()
            .map(|l| wrap::count_wrapped_rows(l, width as usize))
            .collect();
        self.meta = vec![Vec::new(); self.lines.len()];
        self.total = self.counts.iter().map(|c| *c as usize).sum();
    }

    /// 永久全量回退渲染（parse_to_lines 直通，不污染 markdown 哈希缓存）。
    ///
    /// 输入: text — 块源文本；theme — 注入配色；width — 视口宽度
    /// 输出: 无返回值；副作用为整体替换 lines/counts/meta（全部视为冻结）
    fn fallback_render(&mut self, text: &str, theme: &TuiTheme, width: u16) {
        self.lines = markdown::parse_to_lines(text, theme);
        self.counts = self
            .lines
            .iter()
            .map(|l| wrap::count_wrapped_rows(l, width as usize))
            .collect();
        self.meta = vec![Vec::new(); self.lines.len()];
        self.total = self.counts.iter().map(|c| *c as usize).sum();
        self.frozen_len = self.lines.len();
        self.frozen_total = self.total;
        self.frozen_src = text.len();
        self.pos = text.len();
        self.chunk = None;
    }

    /// 逐完结行推进块区状态（design D1 行推进状态机）。
    ///
    /// 输入: text — 块源文本；theme — 注入配色；width — 视口宽度
    /// 输出: 无返回值；副作用为推进 self.pos、冻结 fence 行/闭合块区，
    ///       并累计散文区待冻结简单行（pending_freeze）；命中永久回退
    ///       构造时置 fallback 并中止
    fn advance(&mut self, text: &str, theme: &TuiTheme, width: u16) {
        loop {
            let rest = &text[self.pos..];
            let Some(nl) = rest.find('\n') else {
                break;
            };
            let line_start = self.pos;
            let line_end = self.pos + nl + 1;
            let line = &text[line_start..line_end];
            let content = &line[..line_end - line_start - 1];
            self.pos = line_end;

            // fence 块区优先：区内空白/样式行都是代码内容。
            let fence_act = self
                .chunk
                .as_ref()
                .and_then(|c| c.fence.as_ref())
                .map(|f| !fence_closes(content, f.ch, f.run));
            if let Some(is_content) = fence_act {
                if is_content {
                    let rendered = self
                        .chunk
                        .as_mut()
                        .and_then(|c| c.fence.as_mut())
                        .expect("fence 在场")
                        .code
                        .push_line(line);
                    self.push_frozen_line(rendered, width);
                    self.frozen_src = line_end;
                } else {
                    // 闭合行：底栏 + 空行冻结，块区闭合。
                    self.push_frozen_line(markdown::code_close_bar(), width);
                    self.push_frozen_line(Line::default(), width);
                    self.frozen_src = line_end;
                    self.chunk = None;
                }
                continue;
            }

            let class = classify(content);

            // 空白行：分割判定延迟到下一非空行（松紧表前瞻）；后继行为空白
            // → 前一完结简单行不冻结（段落尾空行归属留给块区闭合/热段）。
            if matches!(class, LineClass::Blank) {
                self.waiting = None;
                self.pending_blank.get_or_insert(line_start);
                continue;
            }

            // 非空行：先决断尾随空白的块区分割（块间语境先于怪癖判据）。
            if let Some(blank_start) = self.pending_blank.take() {
                let absorb = self.chunk.as_ref().is_some_and(|c| c.saw_item)
                    && matches!(class, LineClass::Item { .. } | LineClass::QuoteLine);
                if !absorb {
                    self.close_chunk_at(text, blank_start, theme, width);
                    self.start_prose_chunk(blank_start);
                }
            }

            // 永久回退构造（跨区泄漏：reference 定义/空列表项/空标题）；
            // 块间语境（!trailing_para）才是定义/空项，段落懒续行是文本。
            let trailing_para = self
                .chunk
                .as_ref()
                .is_some_and(|c| c.trailing_para);
            let fatal = matches!(
                (&class, trailing_para),
                (LineClass::AtxHeading { empty: true, .. }, _)
                    | (LineClass::Def, false)
                    | (LineClass::Html, _)
                    | (LineClass::Item { empty: true, .. }, false)
                    | (LineClass::Underline { empty_dash_item: true, .. }, false)
            );
            if fatal {
                self.fallback = true;
                return;
            }

            // fence 起始分割（守卫：区内出现过列表/引用行 → 吸收保等价）。
            if let LineClass::FenceOpener { lang, ch, run } = &class {
                let allow = self
                    .chunk
                    .as_ref()
                    .map(|c| !c.saw_container)
                    .unwrap_or(true);
                if allow {
                    self.close_chunk_at(text, line_start, theme, width);
                    let (lang, ch, run) = (lang.clone(), *ch, *run);
                    self.chunk = Some(Chunk {
                        start: line_start,
                        frozen_at_start: self.frozen_len,
                        fence: Some(FenceState {
                            ch,
                            run,
                            code: CodeLineState::new(&lang, theme),
                        }),
                        saw_container: false,
                        saw_item: false,
                        freeze_ok: false,
                        trailing_para: false,
                        carry_at_start: self.carry,
                        carry_now: self.carry,
                    });
                    // opener 行本身：顶栏行冻结。
                    self.push_frozen_line(markdown::code_open_bar(&lang), width);
                    self.frozen_src = line_end;
                    continue;
                }
            }

            // 归入当前散文块区。
            if self.chunk.is_none() {
                self.start_prose_chunk(line_start);
            }
            self.apply_prose_line(class, line_end);
        }
    }

    /// 开一个散文块区（起点 = 当前行行首）。
    ///
    /// 输入: start — 块区源起点（字节）
    /// 输出: 无返回值；副作用为 self.chunk 置为散文区
    fn start_prose_chunk(&mut self, start: usize) {
        self.chunk = Some(Chunk {
            start,
            frozen_at_start: self.frozen_len,
            fence: None,
            saw_container: false,
            saw_item: false,
            freeze_ok: true,
            trailing_para: false,
            carry_at_start: self.carry,
            carry_now: self.carry,
        });
    }

    /// 把一行归入散文块区（更新守卫状态与逐行冻结判据）。
    ///
    /// 输入: class — 行分类；line_end — 该行行尾字节（含换行）
    /// 输出: 无返回值；副作用为块区状态推进、必要时失效回块区首
    fn apply_prose_line(&mut self, class: LineClass, line_end: usize) {
        let chunk = self.chunk.as_mut().expect("散文区在场");
        chunk.saw_container |= matches!(class, LineClass::Item { .. } | LineClass::QuoteLine);
        chunk.saw_item |= matches!(class, LineClass::Item { .. });
        let trailing = chunk.trailing_para;
        let freeze_ok = chunk.freeze_ok;
        // 后继行非空白已到场 → 前一条待判简单行可冻结（1:1 后缀续渲成立）。
        if let Some(end) = self.waiting.take() {
            self.pending_freeze += 1;
            self.pending_freeze_end = end;
        }
        match class {
            LineClass::Simple => {
                if freeze_ok {
                    self.waiting = Some(line_end);
                }
                chunk.trailing_para = true;
            }
            LineClass::InlineMark => {
                // 行内标记：后缀整段重排可吸收跨行配对，仅停止逐行冻结。
                chunk.freeze_ok = false;
                chunk.trailing_para = true;
            }
            LineClass::BlockLike { closes_para } => {
                self.invalidate_chunk();
                let chunk = self.chunk.as_mut().expect("散文区在场");
                chunk.freeze_ok = false;
                chunk.trailing_para = !closes_para;
            }
            LineClass::FenceOpener { .. }
            | LineClass::AtxHeading { .. }
            | LineClass::Thematic
            | LineClass::QuoteLine => {
                self.invalidate_chunk();
                let chunk = self.chunk.as_mut().expect("散文区在场");
                chunk.freeze_ok = false;
                chunk.trailing_para = false;
                if let LineClass::AtxHeading { level, .. } = &class {
                    chunk.carry_now = chunk
                        .carry_now
                        .add_modifier(ratatui::style::Modifier::BOLD)
                        .fg(markdown::heading_color(*level));
                }
            }
            LineClass::Item { interrupt, .. } => {
                self.invalidate_chunk();
                let chunk = self.chunk.as_mut().expect("散文区在场");
                chunk.freeze_ok = false;
                // 能打断 = 新块（闭段）；不能打断 = 懒续行（段落延续）。
                chunk.trailing_para = !interrupt;
            }
            LineClass::Underline {
                also_thematic,
                empty_dash_item: _,
                level,
            } => {
                if trailing || also_thematic {
                    // setext 下划线（回溯样式）或主题分割线 → 整区重排闭段。
                    self.invalidate_chunk();
                    let chunk = self.chunk.as_mut().expect("散文区在场");
                    chunk.freeze_ok = false;
                    chunk.trailing_para = false;
                    if trailing {
                        chunk.carry_now = chunk
                            .carry_now
                            .add_modifier(ratatui::style::Modifier::BOLD)
                            .fg(markdown::heading_color(level));
                    }
                } else if freeze_ok {
                    // "=" / "--" 等普通段落文本。
                    self.waiting = Some(line_end);
                    chunk.trailing_para = true;
                } else {
                    chunk.trailing_para = true;
                }
            }
            LineClass::Def => {
                // 懒续行文本（定义不能打断段落）→ 整区重排保字面渲染。
                self.invalidate_chunk();
                let chunk = self.chunk.as_mut().expect("散文区在场");
                chunk.freeze_ok = false;
                chunk.trailing_para = true;
            }
            LineClass::Blank | LineClass::Html => unreachable!("先行分流"),
        }
    }

    /// 整区重排失效：丢弃本块区已冻结行，回退到块区首（design D1 失败安全）。
    ///
    /// 输入: 无
    /// 输出: 无返回值；副作用为 lines/counts/meta 截断到块区起点、
    ///       frozen_src 回到块区源起点、逐行冻结停用
    fn invalidate_chunk(&mut self) {
        let Some(chunk) = self.chunk.as_ref() else {
            return;
        };
        let keep = chunk.frozen_at_start;
        let start = chunk.start;
        self.lines.truncate(keep);
        self.counts.truncate(keep);
        self.meta.truncate(keep);
        self.frozen_len = keep;
        self.frozen_total = self.counts.iter().map(|c| *c as usize).sum();
        self.frozen_src = start;
        self.pending_freeze = 0;
        self.pending_freeze_end = 0;
        self.waiting = None;
        self.carry = self.chunk.as_ref().expect("散文区在场").carry_at_start;
    }

    /// 闭合当前散文块区（终点 end）：冻结 [frozen_src..end) 的整区渲染。
    ///
    /// 输入: text — 块源文本；end — 块区源终点（字节）；theme — 注入
    ///       配色；width — 视口宽度
    /// 输出: 无返回值；副作用为冻结渲染行并清空块区/待决断状态
    fn close_chunk_at(&mut self, text: &str, end: usize, theme: &TuiTheme, width: u16) {
        self.pending_freeze = 0;
        self.pending_freeze_end = 0;
        self.waiting = None;
        self.pending_blank = None;
        let Some(chunk) = self.chunk.take() else {
            return;
        };
        debug_assert!(chunk.fence.is_none(), "fence 块区只由闭合行收尾");
        if chunk.fence.is_some() {
            return;
        }
        let render =
            markdown::parse_to_lines_with(&text[self.frozen_src..end], theme, self.carry);
        for line in render {
            self.push_frozen_line(line, width);
        }
        self.frozen_src = end;
        self.carry = chunk.carry_now;
    }

    /// 开放行暂定分类的回溯处理（design D1 失败安全）：开放行未完结也
    /// 可能已回溯影响冻结段（setext 回溯样式、块起始补空行、定义改写
    /// 引用行等），与完结行同判据先行失效/瞬时全量。
    ///
    /// 输入: text — 块源文本；theme — 注入配色；width — 视口宽度
    /// 输出: true = 本帧已转瞬时全量渲染（不置永久回退标志——开放行
    ///       下一帧可能归类为普通内容）；false = 继续快路径
    fn open_line_precheck(&mut self, text: &str, theme: &TuiTheme, width: u16) -> bool {
        let open = &text[self.pos.min(text.len())..];
        if open.is_empty() || self.chunk.as_ref().is_some_and(|c| c.fence.is_some()) {
            return false;
        }
        let class = classify(open);
        let trailing_para = self.chunk.as_ref().is_some_and(|c| c.trailing_para);
        let transient_fatal = matches!(
            (&class, trailing_para),
            (LineClass::AtxHeading { empty: true, .. }, _)
                | (LineClass::Def, false)
                | (LineClass::Html, _)
                | (LineClass::Item { empty: true, .. }, false)
                | (LineClass::Underline { empty_dash_item: true, .. }, false)
        );
        if transient_fatal {
            self.transient_full_render(text, theme, width);
            return true;
        }
        let invalidate = matches!(
            class,
            LineClass::FenceOpener { .. }
                | LineClass::AtxHeading { .. }
                | LineClass::Thematic
                | LineClass::QuoteLine
                | LineClass::Item { .. }
                | LineClass::Underline { .. }
                | LineClass::Def
                | LineClass::BlockLike { .. }
        );
        if invalidate {
            self.invalidate_chunk();
            let chunk = self.chunk.as_mut();
            if let Some(chunk) = chunk {
                chunk.freeze_ok = false;
            }
        }
        false
    }

    /// 热段重建：开放块区每帧重排 + 逐行冻结前缀。
    ///
    /// 输入: text — 块源文本；theme — 注入配色；width — 视口宽度
    /// 输出: 无返回值；副作用为热段行/计数重建并推进冻结点
    fn render_hot(&mut self, text: &str, theme: &TuiTheme, width: u16) {
        let suffix = &text[self.frozen_src.min(text.len())..];
        let hot: Vec<Line<'static>> = match self.chunk.as_ref() {
            // 块间但仍有未消费尾部（开放行/尾随空白）：按临时散文区渲染，
            // 完结后由行推进归类（与全量路径 EOF 口径一致）。
            None => {
                if suffix.is_empty() {
                    Vec::new()
                } else {
                    markdown::parse_to_lines_with(suffix, theme, self.carry)
                }
            }
            Some(chunk) => match &chunk.fence {
                Some(f) => {
                    let mut v = Vec::new();
                    // 开放行：闭合行候选则直接收尾；纯空白且缩进 <4 列的
                    // 开放行按 pulldown 口径丢弃（EOF 尾空白规则，probe 实测）。
                    if !suffix.is_empty()
                        && !fence_closes(suffix, f.ch, f.run)
                        && !(suffix.chars().all(|c| c == ' ') && suffix.len() < 4)
                    {
                        v.push(f.code.peek_line(suffix));
                    }
                    v.push(markdown::code_close_bar());
                    v.push(Line::default());
                    v
                }
                None => markdown::parse_to_lines_with(suffix, theme, self.carry),
            },
        };
        let hot_counts: Vec<u32> = hot
            .iter()
            .map(|l| wrap::count_wrapped_rows(l, width as usize))
            .collect();
        let hot_total: usize = hot_counts.iter().map(|c| *c as usize).sum();
        let mut hot_meta = vec![Vec::new(); hot.len()];

        // 逐行冻结：热段前缀的完结简单行（1:1 映射，开放行/尾随空行不冻）。
        // 末条待判行：后继行（开放行）存在且非空白才冻结，否则留热——
        // 段落尾空行归属不旁落（design D1 失败安全）。
        if let Some(end) = self.waiting.take() {
            let open = &text[self.pos.min(text.len())..];
            let open_blank =
                open.is_empty() || open.bytes().all(|b| b == b' ' || b == b'\t');
            if !open_blank {
                self.pending_freeze += 1;
                self.pending_freeze_end = end;
            }
        }
        let k = self.pending_freeze.min(hot.len());
        if k > 0 {
            self.frozen_len += k;
            self.frozen_src = self.pending_freeze_end;
            self.frozen_total += hot_counts[..k].iter().map(|c| *c as usize).sum::<usize>();
        }
        self.pending_freeze = 0;
        self.pending_freeze_end = 0;

        self.total = self.frozen_total + hot_total;
        self.lines.extend(hot);
        self.counts.extend(hot_counts);
        self.meta.append(&mut hot_meta);
    }

    /// 追加一条冻结行（fence 行级推进/块区闭合共用）。
    ///
    /// 输入: line — 已渲染行；width — 视口宽度（wrap 计数）
    /// 输出: 无返回值；副作用为 lines/counts/meta 追加并推进冻结合计
    fn push_frozen_line(&mut self, line: Line<'static>, width: u16) {
        let count = wrap::count_wrapped_rows(&line, width as usize);
        self.frozen_total += count as usize;
        self.total += count as usize;
        self.lines.push(line);
        self.counts.push(count);
        self.meta.push(Vec::new());
        self.frozen_len += 1;
    }
}

/// 行分类（块区推进判据）：先块级样式后段落文本，形态判据与 pulldown
/// 对齐（probe 实测）。
///
/// 输入: content — 单行源文本（不含行尾换行）
/// 输出: 行分类；不确定构造一律归 Complex（整区重排吸收）
fn classify(content: &str) -> LineClass {
    if content.bytes().all(|b| b == b' ' || b == b'\t') {
        return LineClass::Blank;
    }
    let indent = content.len() - content.trim_start_matches(' ').len();
    let body = &content[indent..];

    // 缩进代码候选（4+ 空格/制表符起头）：独立渲染有歧义 → 整区重排。
    if indent > 3 || body.starts_with('\t') {
        return LineClass::BlockLike {
            closes_para: false,
        };
    }

    // fence 起始：``` / ~~~ 3+（反引号 fence 信息串不得含反引号）。
    for ch in ['\u{60}', '~'] {
        if body.starts_with(ch) {
            let run = body.chars().take_while(|c| *c == ch).count();
            let info = &body[run..];
            if run >= 3 && (ch != '`' || !info.contains('`')) {
                return LineClass::FenceOpener {
                    lang: info.trim().to_string(),
                    ch,
                    run,
                };
            }
        }
    }

    // ATX 标题：# 1-6 个后接空白/行尾/#。
    if body.starts_with('#') {
        let run = body.chars().take_while(|c| *c == '#').count();
        let rest = &body[run..];
        if run <= 6
            && (rest.is_empty()
                || rest.starts_with(' ')
                || rest.starts_with('\t')
                || rest.starts_with('#'))
        {
            let t = rest.trim();
            let empty = t.is_empty() || t.chars().all(|c| c == '#');
            let level = match run {
                1 => HeadingLevel::H1,
                2 => HeadingLevel::H2,
                3 => HeadingLevel::H3,
                4 => HeadingLevel::H4,
                5 => HeadingLevel::H5,
                _ => HeadingLevel::H6,
            };
            return LineClass::AtxHeading { empty, level };
        }
    }

    // setext 下划线候选（=+ / -+）与主题分割线的重叠形态。
    let trimmed = body.trim_end_matches([' ', '\t']);
    let underline = !trimmed.is_empty()
        && ((trimmed.starts_with('=') && trimmed.chars().all(|c| c == '='))
            || (trimmed.starts_with('-') && trimmed.chars().all(|c| c == '-')));
    if underline {
        let level = if trimmed.starts_with('=') {
            HeadingLevel::H1
        } else {
            HeadingLevel::H2
        };
        return LineClass::Underline {
            also_thematic: is_thematic(body),
            empty_dash_item: trimmed.len() == 1 && trimmed == "-",
            level,
        };
    }
    if is_thematic(body) {
        return LineClass::Thematic;
    }

    // 引用行。
    if body.starts_with('>') {
        return LineClass::QuoteLine;
    }

    // reference 定义候选（[id]: ...）。
    if body.starts_with('[')
        && let Some(close) = body.find("]:")
        && close > 0
    {
        return LineClass::Def;
    }

    // 列表项：标记后接空白/行尾。
    let item = if body.starts_with(['*', '+', '-']) {
        Some((1usize, false, 1u32))
    } else {
        let digits: String = body.chars().take_while(|c| c.is_ascii_digit()).collect();
        let after = &body[digits.len()..];
        if !digits.is_empty()
            && digits.len() <= 9
            && (after.starts_with('.') || after.starts_with(')'))
            && (after.len() == 1 || after[1..].starts_with(' ') || after[1..].starts_with('\t'))
        {
            Some((digits.len() + 1, true, digits.parse().unwrap_or(0)))
        } else {
            None
        }
    };
    if let Some((marker_len, ordered, num)) = item {
        let rest = &body[marker_len..];
        let empty = rest.trim_start_matches([' ', '\t']).is_empty();
        let interrupt = !empty && (!ordered || num == 1);
        return LineClass::Item { interrupt, empty };
    }

    // 段落文本：html/不确定构造 → 整区重排；行内标记 → 停止逐行冻结。
    let html_like = body
        .chars()
        .zip(body.chars().skip(1))
        .any(|(a, b)| a == '<' && (b.is_ascii_alphabetic() || b == '/' || b == '!' || b == '?'));
    if html_like {
        return LineClass::Html;
    }
    if body.contains(['*', '_', '~', '[', ']', '`']) || body.ends_with('\\') {
        return LineClass::InlineMark;
    }
    LineClass::Simple
}

/// 主题分割线判定（3+ 同符标记，标记间仅空白）。
///
/// 输入: body — 去缩进行文本
/// 输出: true = 主题分割线形态
fn is_thematic(body: &str) -> bool {
    for ch in ['*', '-', '_'] {
        if !body.starts_with(ch) {
            continue;
        }
        let mut count = 0usize;
        let mut ok = true;
        for c in body.chars() {
            match c {
                c if c == ch => count += 1,
                ' ' | '\t' => {}
                _ => {
                    ok = false;
                    break;
                }
            }
        }
        if ok && count >= 3 {
            return true;
        }
    }
    false
}

/// fence 闭合行判定（同字符、长度 ≥ 开始行、仅尾随空白）。
///
/// 输入: content — 单行源文本（不含行尾换行）；ch — fence 标记字符；
///       run — 开始行标记长度
/// 输出: true = 闭合行
fn fence_closes(content: &str, ch: char, run: usize) -> bool {
    let body = content.trim_start_matches(' ');
    if content.len() - body.len() > 3 || !body.starts_with(ch) {
        return false;
    }
    let len = body.chars().take_while(|c| *c == ch).count();
    len >= run && body[len..].trim().is_empty()
}

/// 防改写快照：取源指定偏移起的 8 字节（不足补零）。
///
/// 输入: text — 源文本；at — 起点字节
/// 输出: 8 字节快照
fn snapshot(text: &str, at: usize) -> [u8; 8] {
    let mut out = [0u8; 8];
    if let Some(seg) = text.as_bytes().get(at..at + 8) {
        out.copy_from_slice(seg);
    }
    out
}

/// FNV-1a 64 位哈希（免重排短路签名；不需要密码学强度）。
///
/// 输入: text — 任意文本
/// 输出: 64 位哈希值
fn fnv1a(text: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}


