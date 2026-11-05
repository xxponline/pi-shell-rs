//! 斜杠命令补全的纯逻辑：触发判定、模糊过滤（复刻 pi 打分）、最佳匹配与
//! 补全文本应用。
//!
//! 无 TTY 依赖——渲染与键位在 `view.rs`/`mod.rs`，数据装配在 App 层。
//! 对照 pi `packages/tui/src/fuzzy.ts` 与 `autocomplete.ts`。

use crate::app::commands::{self, CommandSpec};

/// 一条动态斜杠命令（扩展/模板/skill，来自 `get_commands` RPC）。
#[derive(Debug, Clone, PartialEq)]
pub struct DynamicCommand {
    /// 命令名（skill 为 `skill:xxx` 形式）。
    pub name: String,
    /// 描述（来源标签由 merge 时拼接）。
    pub description: String,
    /// 来源分类：`extension` / `prompt` / `skill`。
    pub source: String,
}

/// 一条补全候选。
#[derive(Debug, Clone, PartialEq)]
pub struct CompletionItem {
    /// 应用时插入的值（命令名不带 `/`；参数为完整参数值）。
    pub value: String,
    /// 菜单主列显示文本。
    pub label: String,
    /// 说明列（命令为 `参数提示 — 说明`）。
    pub description: Option<String>,
}

/// 补全类别。
#[derive(Debug, Clone, PartialEq)]
pub enum CompletionKind {
    /// 命令名补全（替换含 `/` 的前缀，应用后补 `空格`）。
    Command,
    /// 命令参数补全（只替换参数前缀，不加空格）。
    Argument(String),
}

/// 菜单状态（存于 App）。
#[derive(Debug, Clone, Default)]
pub struct CompletionState {
    /// 菜单是否打开。
    pub open: bool,
    /// 被替换的前缀（命令类含 `/`）。
    pub prefix: String,
    /// 候选（已过滤、已排序）。
    pub items: Vec<CompletionItem>,
    /// 当前选中下标。
    pub selected: usize,
    /// 类别。
    pub kind: Option<CompletionKind>,
}

impl CompletionState {
    /// 关闭菜单并清空内容。
    pub fn close(&mut self) {
        *self = CompletionState::default();
    }
}

/// 从光标前文本解析补全请求。
///
/// 输入: text_before — 光标前的当前行文本
/// 输出: Some((kind, 前缀, 候选)) 表示应开菜单；None 表示关闭。
///       命令类：行首（trim 后）`/` 起始且 `/` 后无空白；参数类：命中
///       model/thinking 且已输入空格。
pub fn suggestions(
    text_before: &str,
    models: &[String],
    levels: &[String],
    dynamic: &[DynamicCommand],
    skill_enabled: bool,
) -> Option<(CompletionKind, String, Vec<CompletionItem>)> {
    let trimmed = text_before.trim_start();
    if !trimmed.starts_with('/') {
        return None; // 非行首斜杠 / 普通文本 → 不触发（spec：消息中部不触发）
    }
    let rest = &trimmed[1..];
    match rest.find(char::is_whitespace) {
        None => {
            // 命令名补全：内置 + 动态命令合并后按 rest 模糊过滤
            let items = filter_commands(rest, dynamic, skill_enabled);
            if items.is_empty() {
                return None; // 无匹配 → 菜单收起（spec）
            }
            let prefix_len = trimmed.len(); // 含 `/`
            let prefix = trimmed[text_before.len() - prefix_len..].to_string();
            Some((CompletionKind::Command, prefix, items))
        }
        Some(i) => {
            let name = &rest[..i];
            let arg_query = rest[i..].trim_start();
            let spec: &CommandSpec = commands::find(name)?;
            let source = match spec.arg_completion {
                commands::ArgCompletion::Models => models,
                commands::ArgCompletion::ThinkingLevels => levels,
                commands::ArgCompletion::None => return None, // 无参数补全 → 不弹（spec）
            };
            let items = filter_values(source, arg_query);
            if items.is_empty() {
                return None;
            }
            // 前缀 = `/name ` 之后的参数部分（只替换参数）
            let line_prefix_end = text_before.len() - arg_query.len();
            let prefix = text_before[line_prefix_end..].to_string();
            Some((CompletionKind::Argument(name.to_string()), prefix, items))
        }
    }
}

/// 解析当前是否处于"需要数据源的参数补全"上下文（即使缓存为空菜单未开，
/// 上层也据此发起惰性拉取）。
///
/// 输入: text_before — 光标前的当前行文本
/// 输出: Some("model"/"thinking") 表示需要对应数据源；None 表示不需要
pub fn arg_need(text_before: &str) -> Option<&'static str> {
    let trimmed = text_before.trim_start();
    let rest = trimmed.strip_prefix('/')?;
    let i = rest.find(char::is_whitespace)?;
    let spec = commands::find(&rest[..i])?;
    match spec.arg_completion {
        commands::ArgCompletion::Models => Some("model"),
        commands::ArgCompletion::ThinkingLevels => Some("thinking"),
        commands::ArgCompletion::None => None,
    }
}

/// 合并内置与动态命令为补全候选（对齐 pi 的合并规则）：
/// 1) 与内置同名的动态项被过滤（pi 冲突规则）；2) `skill:` 项仅在开关开启时保留；
/// 3) 动态描述加 `[{source}]` 标签；4) 内置原序在前、动态按返回序在后。
///
/// 输入: builtin — 内置命令表；dynamic — get_commands 结果；skill_enabled — 开关
/// 输出: 合并后的候选（未过滤 fuzzy）
pub fn merge_candidates(
    builtin: &[commands::CommandSpec],
    dynamic: &[DynamicCommand],
    skill_enabled: bool,
) -> Vec<CompletionItem> {
    let mut out: Vec<CompletionItem> = builtin.iter().map(command_item).collect();
    let builtin_names: Vec<&str> = builtin.iter().map(|c| c.name).collect();
    for cmd in dynamic {
        if builtin_names.contains(&cmd.name.as_str()) {
            continue; // 同名冲突：过滤扩展（pi: filter((cmd) => !builtinCommandNames.has(...))）
        }
        if cmd.name.starts_with("skill:") && !skill_enabled {
            continue; // enableSkillCommands=false
        }
        let desc = if cmd.description.is_empty() {
            format!("[{}]", cmd.source)
        } else {
            format!("[{}] {}", cmd.source, cmd.description)
        };
        out.push(CompletionItem {
            value: cmd.name.clone(),
            label: cmd.name.clone(),
            description: Some(desc),
        });
    }
    out
}

/// 合并 + 模糊过滤：命令候选（说明列 = `参数提示 — 说明` 或 `[source] 描述`）。
///
/// 输入: query — `/` 之后的查询文本；dynamic/skill_enabled — 动态命令与开关
/// 输出: 过滤排序后的候选
fn filter_commands(
    query: &str,
    dynamic: &[DynamicCommand],
    skill_enabled: bool,
) -> Vec<CompletionItem> {
    let items = merge_candidates(commands::COMMANDS, dynamic, skill_enabled);
    fuzzy_filter(&items, query, |item| item.value.clone())
}

/// 单条命令元数据 → 候选（复刻 pi 的 fullDesc 组装）。
fn command_item(spec: &CommandSpec) -> CompletionItem {
    let desc = match spec.argument_hint {
        Some(hint) => format!("{hint} — {}", spec.description),
        None => spec.description.to_string(),
    };
    CompletionItem {
        value: spec.name.to_string(),
        label: spec.name.to_string(),
        description: Some(desc),
    }
}

/// 参数值列表 → 候选（label 即值，模糊过滤）。
fn filter_values(values: &[String], query: &str) -> Vec<CompletionItem> {
    let items: Vec<CompletionItem> = values
        .iter()
        .map(|v| CompletionItem {
            value: v.clone(),
            label: v.clone(),
            description: None,
        })
        .collect();
    fuzzy_filter(&items, query, |item| item.value.clone())
}

/// 复刻 pi `fuzzyFilter`：按空白/斜杠切词元，全部词元模糊匹配才保留，
/// 按总分升序（pi 分数越低越好），空查询保持原序。
///
/// 输入: items — 候选；query — 查询；get_text — 匹配文本取值
/// 输出: 过滤并排序后的候选（借用拷贝为新 Vec，调用方拥有）
pub fn fuzzy_filter<T: Clone, F: Fn(&T) -> String>(items: &[T], query: &str, get_text: F) -> Vec<T> {
    if query.trim().is_empty() {
        return items.to_vec();
    }
    let tokens: Vec<&str> = query
        .trim()
        .split([' ', '/', '\t'])
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.is_empty() {
        return items.to_vec();
    }
    let mut scored: Vec<(usize, f64)> = Vec::new();
    for (idx, item) in items.iter().enumerate() {
        let text = get_text(item);
        let mut total = 0.0;
        let mut all = true;
        for token in &tokens {
            match fuzzy_score(token, &text) {
                Some(s) => total += s,
                None => {
                    all = false;
                    break;
                }
            }
        }
        if all {
            scored.push((idx, total));
        }
    }
    // 分数升序（越低越好）；同分保持原序（稳定排序）
    scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().map(|(i, _)| items[i].clone()).collect()
}

/// 复刻 pi `fuzzyMatch` 的贪心对齐打分（越低越好）。
///
/// 输入: query/text — 均按小写比较
/// 输出: Some(分数) 匹配成功；None 不匹配
pub fn fuzzy_score(query: &str, text: &str) -> Option<f64> {
    let q = query.to_lowercase();
    let t = text.to_lowercase();
    if q.is_empty() {
        return Some(0.0);
    }
    if q.len() > t.len() {
        return None;
    }
    // 贪心对齐：每个查询字符取其上次位置之后的首次出现
    let tb = t.as_bytes();
    let qb = q.as_bytes();
    let mut positions: Vec<usize> = Vec::with_capacity(qb.len());
    let mut last: isize = -1;
    for &ch in qb {
        let mut found = None;
        for i in (last + 1)..tb.len() as isize {
            if tb[i as usize] == ch {
                found = Some(i as usize);
                break;
            }
        }
        let i = found?;
        positions.push(i);
        last = i as isize;
    }
    let mut score = score_positions(&t, &positions);
    // 连续出现的其它对齐取最优（pi 的二次扫描）
    if let Some(first) = t.find(q.as_str()) {
        let mut start = first;
        loop {
            let alt: Vec<usize> = (0..qb.len()).map(|k| start + k).collect();
            score = score.min(score_positions(&t, &alt));
            match t[start + 1..].find(q.as_str()) {
                Some(off) => start += 1 + off,
                None => break,
            }
        }
    }
    if q == t {
        score -= 100.0;
    }
    Some(score)
}

/// 复刻 pi `scorePositions`：连续加分、间隔罚分、词元边界奖励。
fn score_positions(text: &str, positions: &[usize]) -> f64 {
    let tb = text.as_bytes();
    let mut score: f64 = 0.0;
    let mut last: Option<usize> = None;
    let mut consecutive: i32 = 0;
    for &i in positions {
        match last {
            Some(l) if l == i - 1 => {
                consecutive += 1;
                score -= f64::from(consecutive) * 5.0;
            }
            _ => {
                consecutive = 0;
                if let Some(l) = last {
                    score += (i - l - 1) as f64 * 2.0;
                }
            }
        }
        let boundary = i == 0 || matches!(tb.get(i - 1), Some(b' ' | b'-' | b'_' | b'.' | b'/' | b':'));
        if boundary {
            score -= 10.0;
        }
        score += i as f64 * 0.1;
        last = Some(i);
    }
    score
}

/// 最佳匹配预选：精确匹配优先，其次首个前缀匹配，否则 0。
///
/// 输入: items — 已排序候选；query — 查询（命令类不含 `/`）
/// 输出: 选中下标
pub fn best_match_index(items: &[CompletionItem], query: &str) -> usize {
    if items.is_empty() {
        return 0;
    }
    for (i, item) in items.iter().enumerate() {
        if item.value == query {
            return i;
        }
    }
    for (i, item) in items.iter().enumerate() {
        if item.value.starts_with(query) {
            return i;
        }
    }
    0
}

/// 字符下标 → 字节下标（编辑器 col 是字符序号，行内容可能含多字节）。
fn char_to_byte(text: &str, char_idx: usize) -> usize {
    text.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(text.len())
}

/// 应用补全：替换前缀为选中值。
///
/// 输入: line — 整行文本；cursor_char — 光标字符下标（与编辑器 col 一致）；
///       prefix — 待替换前缀（命令类含 `/`）；value — 选中值；kind — 类别
/// 输出: (新行文本, 新光标字符下标)
pub fn apply_completion(
    line: &str,
    cursor_char: usize,
    prefix: &str,
    value: &str,
    kind: &CompletionKind,
) -> (String, usize) {
    let prefix_chars = prefix.chars().count();
    let start_char = cursor_char.saturating_sub(prefix_chars);
    let byte_start = char_to_byte(line, start_char);
    let byte_end = char_to_byte(line, cursor_char);
    let before = &line[..byte_start];
    let after = &line[byte_end..];
    match kind {
        CompletionKind::Command => {
            // `/query` → `/value `（spec：光标置于空格后）
            let new_line = format!("{before}/{value} {after}");
            let new_cursor = start_char + 1 + value.chars().count() + 1;
            (new_line, new_cursor)
        }
        CompletionKind::Argument(_) => {
            // 参数前缀 → value（不加空格）
            let new_line = format!("{before}{value}{after}");
            let new_cursor = start_char + value.chars().count();
            (new_line, new_cursor)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造模型与级别数据源。
    fn data() -> (Vec<String>, Vec<String>) {
        (
            vec!["openai/gpt-5.4".into(), "anthropic/claude-opus".into()],
            vec!["off".into(), "low".into(), "high".into()],
        )
    }

    #[test]
    fn trigger_only_at_line_start_slash_without_space() {
        let (models, levels) = data();
        // 行首 `/` 无空格 → 命令菜单
        assert!(suggestions("/", &models, &levels, &[], true).is_some());
        assert!(suggestions("/th", &models, &levels, &[], true).is_some());
        // 前导空白允许（trim 语义）
        assert!(suggestions("  /th", &models, &levels, &[], true).is_some());
        // 非行首斜杠不触发（spec：消息中部）
        assert!(suggestions("hello /th", &models, &levels, &[], true).is_none());
        assert!(suggestions("see src/app", &models, &levels, &[], true).is_none());
        // 空输入不触发
        assert!(suggestions("", &models, &levels, &[], true).is_none());
    }

    #[test]
    fn no_match_closes_menu() {
        let (models, levels) = data();
        assert!(suggestions("/zzz", &models, &levels, &[], true).is_none(), "无匹配应收起");
    }

    #[test]
    fn command_items_and_description() {
        let (models, levels) = data();
        let (kind, prefix, items) = suggestions("/mo", &models, &levels, &[], true).unwrap();
        assert_eq!(kind, CompletionKind::Command);
        assert_eq!(prefix, "/mo");
        let model = items.iter().find(|i| i.value == "model").expect("model 应命中");
        let desc = model.description.as_deref().unwrap();
        assert!(desc.contains("<provider/model>"), "参数提示应前缀: {desc}");
        assert!(desc.contains("Select model"), "说明应含描述: {desc}");
        // thinking 也应模糊命中（mo… / th… 不同词元）
        let (_, _, th) = suggestions("/thi", &models, &levels, &[], true).unwrap();
        assert_eq!(th[0].value, "thinking", "thi 应排首: {th:?}");
    }

    #[test]
    fn fuzzy_multi_token_and_scoring() {
        // 词元 AND：全部命中才保留
        assert!(fuzzy_score("mod", "model").is_some());
        assert!(fuzzy_score("xyz", "model").is_none(), "非子序列不匹配");
        // 分数越低越好：前缀命中优于中段命中
        let prefix = fuzzy_score("ses", "session").unwrap();
        let mid = fuzzy_score("ess", "session").unwrap();
        assert!(prefix < mid, "前缀应更优: {prefix} vs {mid}");
        // 多词元
        let items = vec![
            CompletionItem { value: "ab-x".into(), label: "ab-x".into(), description: None },
            CompletionItem { value: "abc".into(), label: "abc".into(), description: None },
        ];
        // 词元 AND：第二词元 x 只在 ab-x 中出现
        let hit = fuzzy_filter(&items, "ab x", |i| i.value.clone());
        assert_eq!(hit.len(), 1, "只有 ab-x 同时命中两个词元: {hit:?}");
        assert_eq!(hit[0].value, "ab-x");
        // 空查询保持原序
        let keep = fuzzy_filter(&items, "", |i| i.value.clone());
        assert_eq!(keep, items);
    }

    #[test]
    fn best_match_prefers_exact_then_prefix() {
        let items = |vals: &[&str]| -> Vec<CompletionItem> {
            vals.iter()
                .map(|v| CompletionItem { value: (*v).into(), label: (*v).into(), description: None })
                .collect()
        };
        let two = items(&["model", "session"]);
        assert_eq!(best_match_index(&two, "model"), 0, "精确匹配优先");
        let reordered = items(&["settings", "model"]);
        assert_eq!(best_match_index(&reordered, "model"), 1, "首个前缀匹配");
        assert_eq!(best_match_index(&two, "zzz"), 0, "无匹配回落 0");
        assert_eq!(best_match_index(&[], "x"), 0, "空表不 panic");
    }

    #[test]
    fn argument_completion_for_model_and_thinking_only() {
        let (models, levels) = data();
        // /model 空格 → 模型候选
        let (kind, prefix, items) = suggestions("/model gpt", &models, &levels, &[], true).unwrap();
        assert_eq!(kind, CompletionKind::Argument("model".into()));
        assert_eq!(prefix, "gpt");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].value, "openai/gpt-5.4");
        // /thinking 空格 → 级别候选
        let (_, _, th) = suggestions("/thinking hi", &models, &levels, &[], true).unwrap();
        assert_eq!(th.len(), 1);
        assert_eq!(th[0].value, "high");
        // 无参数补全的命令空格后不弹（spec）
        assert!(suggestions("/copy x", &models, &levels, &[], true).is_none());
        // 无匹配参数 → 收起
        assert!(suggestions("/model nomatch", &models, &levels, &[], true).is_none());
    }

    #[test]
    fn arg_need_detects_data_source_commands() {
        assert_eq!(arg_need("/model "), Some("model"));
        assert_eq!(arg_need("/model gpt"), Some("model"));
        assert_eq!(arg_need("/thinking hi"), Some("thinking"));
        assert_eq!(arg_need("/copy x"), None, "无参数补全命令");
        assert_eq!(arg_need("/model"), None, "未输入空格不拉取");
        assert_eq!(arg_need("hello"), None);
    }

    #[test]
    fn apply_command_and_argument() {
        // 命令类：替换含 / 的前缀，补空格，光标在空格后
        let (line, cur) = apply_completion("/thi", 4, "/thi", "thinking", &CompletionKind::Command);
        assert_eq!(line, "/thinking ");
        assert_eq!(cur, 10);
        // 光标不在行尾时保留后缀
        let (line, cur) = apply_completion("/thi more", 4, "/thi", "thinking", &CompletionKind::Command);
        assert_eq!(line, "/thinking  more");
        assert_eq!(cur, 10);
        // 参数类：只替换参数、不加空格
        let (line, cur) = apply_completion(
            "/model gpt",
            10,
            "gpt",
            "openai/gpt-5.4",
            &CompletionKind::Argument("model".into()),
        );
        assert_eq!(line, "/model openai/gpt-5.4");
        assert_eq!(cur, 21);
        // 多字节字符：光标为字符下标，中文前缀不被截断
        let (line, cur) = apply_completion("中 /thi", 6, "/thi", "thinking", &CompletionKind::Command);
        assert_eq!(line, "中 /thinking ");
        assert_eq!(cur, 12, "字符下标: 中+空格+/+thinking+空格");
    }

    // ── 动态命令合并（spec：动态斜杠命令补全） ──

    /// 造三条动态命令（扩展/模板/skill 各一）。
    fn dyn_cmds() -> Vec<DynamicCommand> {
        vec![
            DynamicCommand { name: "my-ext".into(), description: "Do thing".into(), source: "extension".into() },
            DynamicCommand { name: "deploy".into(), description: "Ship it".into(), source: "prompt".into() },
            DynamicCommand { name: "skill:review".into(), description: "Review code".into(), source: "skill".into() },
        ]
    }

    #[test]
    fn merge_filters_conflicts_and_skill_toggle_and_tags() {
        // 冲突：与内置同名的动态项被过滤
        let mut dyns = dyn_cmds();
        dyns.push(DynamicCommand { name: "tree".into(), description: "shadow builtin".into(), source: "extension".into() });
        let merged = merge_candidates(commands::COMMANDS, &dyns, true);
        assert_eq!(merged.len(), 24 + 3, "同名 tree 被过滤: {}", merged.len());
        let tree = merged.iter().find(|i| i.value == "tree").unwrap();
        assert!(!tree.description.as_deref().unwrap_or("").contains("shadow"), "内置项未被遮蔽");

        // skill 开关
        let merged_off = merge_candidates(commands::COMMANDS, &dyns, false);
        assert_eq!(merged_off.len(), 24 + 2, "skill 候选被过滤");
        assert!(!merged_off.iter().any(|i| i.value.starts_with("skill:")));

        // 源标签
        let ext = merged.iter().find(|i| i.value == "my-ext").unwrap();
        assert_eq!(ext.description.as_deref(), Some("[extension] Do thing"));
        let tpl = merged.iter().find(|i| i.value == "deploy").unwrap();
        assert_eq!(tpl.description.as_deref(), Some("[prompt] Ship it"));
        let sk = merged.iter().find(|i| i.value == "skill:review").unwrap();
        assert_eq!(sk.description.as_deref(), Some("[skill] Review code"));

        // 空描述 → 仅标签
        let merged2 = merge_candidates(
            commands::COMMANDS,
            &[DynamicCommand { name: "x1".into(), description: "".into(), source: "extension".into() }],
            true,
        );
        let x1 = merged2.iter().find(|i| i.value == "x1").unwrap();
        assert_eq!(x1.description.as_deref(), Some("[extension]"));

        // 顺序：内置在前、动态按返回序
        assert_eq!(merged[0].value, "settings", "内置原序在前");
        let tail: Vec<&str> = merged[24..].iter().map(|i| i.value.as_str()).collect();
        assert_eq!(tail, vec!["my-ext", "deploy", "skill:review"]);
    }

    #[test]
    fn suggestions_includes_dynamic_commands() {
        let (models, levels) = data();
        // 键入 /my → 出现扩展候选
        let (_, _, items) = suggestions("/my", &models, &levels, &dyn_cmds(), true).unwrap();
        assert!(items.iter().any(|i| i.value == "my-ext"), "{items:?}");
        // 空缓存 → 仅内置（降级场景）
        match suggestions("/my", &models, &levels, &[], true) {
            None => {} // /my 无内置匹配 → 收起（即纯内置语义）
            Some((_, _, items)) => assert!(!items.iter().any(|i| i.value == "my-ext")),
        }
        // skill 开关关闭 → skill 候选不可见
        if let Some((_, _, items)) = suggestions("/ski", &models, &levels, &dyn_cmds(), false) {
            assert!(!items.iter().any(|i| i.value.starts_with("skill:")), "{items:?}");
        }
        // 动态命令 Tab 应用（与内置一致）
        let (kind, prefix, items) = suggestions("/my", &models, &levels, &dyn_cmds(), true).unwrap();
        let idx = best_match_index(&items, "my");
        let (line, cur) = apply_completion("/my", 3, &prefix, &items[idx].value, &kind);
        assert_eq!(line, "/my-ext ");
        assert_eq!(cur, 8);
    }
}
