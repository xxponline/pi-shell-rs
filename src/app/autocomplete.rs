//! 斜杠命令补全的纯逻辑：触发判定、模糊过滤（复刻 pi 打分）、最佳匹配与
//! 补全文本应用。
//!
//! 无 TTY 依赖——渲染与键位在 `view.rs`/`mod.rs`，数据装配在 App 层。
//! 对照 pi `packages/tui/src/fuzzy.ts` 与 `autocomplete.ts`。

use std::path::{Path, PathBuf};

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
    /// `@文件` 补全（替换含 `@` 的 token；目录不补空格可续写）。
    AtFile,
    /// 纯路径补全（Tab 触发；替换路径 token，无 `@`、不加尾随空格、
    /// 目录保 `/` 可续写）。
    Path,
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
    /// 是否 force 态（Tab 打开；退格到空白不关、零候选才关——design D1）。
    pub force: bool,
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

// ── `@文件` 补全（对照 pi `packages/tui/src/autocomplete.ts`） ──

/// `@` 文件补全 token（光标前的 `@path` 或 `@"quoted path"`）。
#[derive(Debug, Clone, PartialEq)]
pub struct AtToken {
    /// 完整待替换前缀（含 `@` 与未闭合引号）。
    pub prefix: String,
    /// 查询部分（不含 `@` 与引号）。
    pub query: String,
    /// 是否 `@"` 引号形态。
    pub quoted: bool,
}

/// 从光标前文本提取 `@` 文件 token（对照 pi `extractAtPrefix`/
/// `extractQuotedPrefix`）。
///
/// 输入: text_before — 光标前的当前行文本
/// 输出: Some(token) = 处于 `@` 补全上下文；None = 不触发（斜杠 token、
///       普通词、引号已闭合等）
pub fn extract_at_token(text_before: &str) -> Option<AtToken> {
    // ① `@"quoted` 未闭合形态：最后一个 `@"` 之后引号数为偶数（未闭合）
    if let Some(at_pos) = text_before.rfind("@\"") {
        let after = &text_before[at_pos + 2..];
        if after.matches('"').count().is_multiple_of(2) {
            let before = &text_before[..at_pos];
            if at_pos == 0 || before.chars().last().is_some_and(is_token_boundary) {
                return Some(AtToken {
                    prefix: text_before[at_pos..].to_string(),
                    query: after.to_string(),
                    quoted: true,
                });
            }
        }
    }
    // ② 裸 `@token`：token 以 `@` 起始，`@` 前为行首或分隔符
    //（pi PATH_DELIMITERS：空白/`"`/`'`/`=`）
    let start = text_before
        .rfind(is_token_boundary)
        .map(|i| i + 1)
        .unwrap_or(0);
    let token = &text_before[start..];
    let query = token.strip_prefix('@')?;
    Some(AtToken { prefix: token.to_string(), query: query.to_string(), quoted: false })
}

/// token 起始分隔符判定（pi PATH_DELIMITERS 的字符集）。
fn is_token_boundary(c: char) -> bool {
    c.is_whitespace() || c == '"' || c == '\'' || c == '='
}

/// 一条 `@` 文件候选。
#[derive(Debug, Clone, PartialEq)]
pub struct AtCandidate {
    /// 展示/应用路径（保留 `~/`、`./`、绝对路径写法）。
    pub path: String,
    /// 是否目录（应用时不补空格、label 带 `/`）。
    pub is_dir: bool,
    /// 相关度分数（排序用；空查询全体为 1）。
    pub score: u32,
}

/// 列出 `@` 文件候选：基础目录前缀匹配 + `fd` 递归模糊（注入输出），
/// 打分排序后截取前 20（对照 pi `getFuzzyFileSuggestions`）。
///
/// 输入: query — token 查询；base_dir — 基准目录（cwd）；fd_output — fd
///       原始输出（None = 无 fd/超时，仅基础候选）
/// 输出: 已去重、打分、排序、截断的候选（目录不存在/不可读 → 空，静默）
pub fn list_at_candidates(query: &str, base_dir: &Path, fd_output: Option<&str>) -> Vec<AtCandidate> {
    let raw = query;
    let is_root = raw.is_empty() || raw == ".." || raw == "~" || raw == "/";
    // ① 基础目录候选（大小写不敏感前缀、目录优先在排序阶段体现）
    let mut cands = list_dir_candidates(query, base_dir);
    // ② fd 递归候选（绝对路径行 → 展示路径；与基础候选去重）
    if let Some(raw_fd) = fd_output {
        for line in parse_fd_lines(raw_fd) {
            let display = display_from_abs(&line, base_dir, raw);
            if cands.iter().any(|c| c.path == display) {
                continue;
            }
            let is_dir = std::fs::metadata(&line).map(|m| m.is_dir()).unwrap_or(false);
            cands.push(AtCandidate { path: display, is_dir, score: 0 });
        }
    }
    // 打分：空查询全体 1（pi `fdQuery ? score : 1`），否则 scoreEntry 对等
    let scoring_query = if is_root { "" } else { query };
    for c in &mut cands {
        c.score = if scoring_query.is_empty() {
            1
        } else {
            score_entry(&c.path, scoring_query, c.is_dir)
        };
    }
    cands.retain(|c| c.score > 0);
    // 排序：分数降序 → 路径深度 → 长度 → 字典序（pi 同序）
    cands.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| path_depth(&a.path).cmp(&path_depth(&b.path)))
            .then_with(|| a.path.len().cmp(&b.path.len()))
            .then_with(|| a.path.cmp(&b.path))
    });
    cands.truncate(20);
    cands
}

/// 基础目录候选段（read_dir + 大小写不敏感前缀匹配，无 fd、无打分、
/// 无截断）——`@` 候选与纯路径补全共用（对照 pi `getFileSuggestions` 的
/// 单目录列举；pi 纯路径同样 readdir-only）。
///
/// 输入: query — token 查询（支持 `~`、`./`、绝对路径、目录部分定位）；
///       base_dir — 基准目录（cwd）
/// 输出: 目录项候选（score 恒 0，排序由调用方决定；目录不存在/不可读 → 空，静默）
pub fn list_dir_candidates(query: &str, base_dir: &Path) -> Vec<AtCandidate> {
    let raw = query;
    let exp = expand_home(query);
    // 定位搜索目录与文件前缀（root 前缀列目录本身）
    let is_root = raw.is_empty() || raw == ".." || raw == "~" || raw == "/";
    let (search_dir, file_prefix, display_dir) = if is_root {
        let d = if raw == "~" {
            PathBuf::from(exp.clone())
        } else if raw == "/" {
            PathBuf::from("/")
        } else if raw == ".." {
            base_dir.join("..")
        } else {
            base_dir.to_path_buf()
        };
        let dd = if raw == "~" { "~/".to_string() } else if raw == "/" { "/".to_string() } else { String::new() };
        (d, String::new(), dd)
    } else if exp.ends_with('/') {
        (join_base(base_dir, &exp), String::new(), raw.to_string())
    } else if let Some(idx) = exp.rfind('/') {
        // 目录部分：绝对路径单段（idx==0，如 `/var`）时目录为根 `/`（对照 pi
        // `dirname("/var")=="/"`；否则截到最后一段之前）
        let dir_part = if idx == 0 { "/" } else { &exp[..idx] };
        let prefix = &exp[idx + 1..];
        let raw_dir = &raw[..raw.rfind('/').map(|i| i + 1).unwrap_or(0)];
        (join_base(base_dir, dir_part), prefix.to_string(), raw_dir.to_string())
    } else {
        (base_dir.to_path_buf(), exp.clone(), String::new())
    };

    let mut cands: Vec<AtCandidate> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&search_dir) {
        let pfx = file_prefix.to_lowercase();
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.to_lowercase().starts_with(&pfx) {
                continue;
            }
            let is_dir = entry_is_dir(&entry);
            cands.push(AtCandidate { path: format!("{display_dir}{name}"), is_dir, score: 0 });
        }
    }
    // 目录优先 → 字母序（pi `getFileSuggestions` 同序；纯路径补全直接消费，
    // `@` 侧随后按分数全序重排不受影响）
    cands.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.path.cmp(&b.path)));
    cands
}

/// 复刻 pi `scoreEntry`：文件名精确 100 / 前缀 80 / 子串 50 / 路径子串 30，
/// 目录命中 +10；不匹配为 0。
fn score_entry(file_path: &str, query: &str, is_dir: bool) -> u32 {
    let file_name = file_name_of(file_path);
    let lf = file_name.to_lowercase();
    let lq = query.to_lowercase();
    let lp = file_path.to_lowercase();
    let mut score = if lf == lq {
        100
    } else if lf.starts_with(&lq) {
        80
    } else if lf.contains(&lq) {
        50
    } else if lp.contains(&lq) {
        30
    } else {
        0
    };
    if is_dir && score > 0 {
        score += 10;
    }
    score
}

/// 取路径最后一段（文件名/目录名）。
fn file_name_of(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// 路径深度（按 `/` 分段数，用于排序次级键）。
fn path_depth(path: &str) -> usize {
    path.split('/').filter(|s| !s.is_empty()).count()
}

/// `~` 展开（仅用于搜索定位，展示保持 `~` 写法）。
fn expand_home(p: &str) -> String {
    if p == "~" {
        return std::env::var("HOME").unwrap_or_else(|_| "~".to_string());
    }
    if let Some(rest) = p.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_default();
        if home.is_empty() {
            return p.to_string();
        }
        return format!("{home}/{rest}");
    }
    p.to_string()
}

/// 拼接基准目录与（可能为空/相对/绝对）子路径。
fn join_base(base: &Path, part: &str) -> PathBuf {
    if part.is_empty() {
        return base.to_path_buf();
    }
    let p = Path::new(part);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

/// 目录项是否目录（符号链接跟随 stat；失败按文件，对齐 pi）。
fn entry_is_dir(entry: &std::fs::DirEntry) -> bool {
    match entry.file_type() {
        Ok(t) if t.is_dir() => true,
        Ok(t) if t.is_symlink() => std::fs::metadata(entry.path()).map(|m| m.is_dir()).unwrap_or(false),
        _ => false,
    }
}

/// fd 绝对路径行 → 展示路径（`~/` 查询保 tilde、base 下转相对、其余绝对）。
fn display_from_abs(line: &str, base: &Path, raw_query: &str) -> String {
    if raw_query.starts_with('~') {
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() && line.starts_with(&home) {
            return format!("~/{}", line[home.len()..].trim_start_matches('/'));
        }
    }
    let base_str = base.to_string_lossy().trim_end_matches('/').to_string();
    if !base_str.is_empty() && line.starts_with(&base_str) {
        let rest = &line[base_str.len()..];
        return rest.trim_start_matches('/').to_string();
    }
    line.to_string()
}

/// 解析 fd 输出为绝对路径行（跳过空行与坏行）。
///
/// 输入: raw — fd stdout 原始文本
/// 输出: 有效路径行（每行一条）
pub fn parse_fd_lines(raw: &str) -> Vec<String> {
    raw.lines()
        .map(|l| l.trim().trim_end_matches('\r'))
        .filter(|l| !l.is_empty() && !l.contains('\0'))
        .map(str::to_string)
        .collect()
}

/// 组装 fd 参数（`-F` 固定串匹配、目录+文件、上限 100、绝对路径）。
///
/// 输入: query — 查询（空 = 不带查询列全部）；base_dir — 搜索根
/// 输出: 参数向量
pub fn fd_args(query: &str, base_dir: &Path) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = ["--absolute-path", "--type", "f", "--type", "d"]
        .iter()
        .map(std::ffi::OsString::from)
        .collect();
    args.push("--max-results".into());
    args.push("100".into());
    if !query.is_empty() {
        args.push("-F".into());
        args.push(query.into());
    }
    args.push(base_dir.as_os_str().to_os_string());
    args
}

/// 执行外部命令并限时等待：超时 kill 后返回 None（降级路径）。
///
/// 输入: cmd — 可执行文件；args — 参数；timeout — 时限
/// 输出: Some(stdout) = 正常结束且有输出；None = 启动失败/超时/读取失败
pub fn run_cmd_with_timeout(cmd: &str, args: &[std::ffi::OsString], timeout: std::time::Duration) -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut child = Command::new(cmd)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    // 独立读线程防管道写满阻塞
    let reader = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stdout.read_to_string(&mut buf);
        buf
    });
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None; // 超时 → 调用方回落基础候选
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(_) => {
                let _ = child.kill();
                return None;
            }
        }
    }
    reader.join().ok()
}

/// 生产 `fd` 调用（300ms 时限；无 fd/超时/失败 → None）。
///
/// 输入: query — token 查询；base_dir — 搜索根
/// 输出: Some(fd stdout) 或 None（回落仅基础候选）
pub fn run_fd(query: &str, base_dir: &Path) -> Option<String> {
    run_cmd_with_timeout("fd", &fd_args(query, base_dir), std::time::Duration::from_millis(300))
}

/// 是否需要自动加引号（空白或 CJK 标点，对照 pi autocompleteSeparatorRegex；
/// 已带引号的 token 恒需要）。
fn needs_quotes(quoted_prefix: bool, path: &str) -> bool {
    if quoted_prefix {
        return true;
    }
    path.chars().any(|c| {
        c.is_whitespace() || "，．：；！？（）［］｛｝“”‘’…—".contains(c)
    })
}

/// 构造应用值：保 `@` 前缀与路径写法，需引号时 `@"…"`，目录带尾 `/`。
///
/// 输入: token — 当前 token；cand — 选中候选
/// 输出: 替换用的完整值（如 `@src/main.rs`、`@"my dir/x"`、`@src/`）
pub fn build_at_value(token: &AtToken, cand: &AtCandidate) -> String {
    let mut path = cand.path.clone();
    if cand.is_dir && !path.ends_with('/') {
        path.push('/');
    }
    if needs_quotes(token.quoted, &path) {
        format!("@\"{path}\"")
    } else {
        format!("@{path}")
    }
}

/// 构造纯路径应用值（对照 pi `buildCompletionValue` 非 `@` 分支）：
/// 无 `@` 前缀、目录保尾 `/`、需引号时包 `"…"`、不加尾随空格。
///
/// 输入: token — 当前路径 token；cand — 选中候选
/// 输出: 替换用的完整值（如 `src/main.rs`、`src/`、`"my dir/foo.txt"`）
pub fn build_path_value(token: &PathToken, cand: &AtCandidate) -> String {
    let mut path = cand.path.clone();
    if cand.is_dir && !path.ends_with('/') {
        path.push('/');
    }
    if needs_quotes(token.quoted, &path) {
        format!("\"{path}\"")
    } else {
        path
    }
}

/// `@` 补全入口：提取 token → fd+基础候选 → 打包菜单数据。
///
/// 输入: text_before — 光标前文本；base_dir — cwd
/// 输出: Some((待替换前缀, 候选)) = 应开菜单；None = 不触发/无候选（静默）
pub fn at_suggestions(text_before: &str, base_dir: &Path) -> Option<(String, Vec<CompletionItem>)> {
    let token = extract_at_token(text_before)?;
    let fd_raw = run_fd(&token.query, base_dir);
    let cands = list_at_candidates(&token.query, base_dir, fd_raw.as_deref());
    if cands.is_empty() {
        return None; // pi: suggestions.length === 0 → 不弹
    }
    let items = cands
        .iter()
        .map(|c| CompletionItem {
            value: build_at_value(&token, c),
            label: if c.is_dir {
                format!("{}/", file_name_of(&c.path))
            } else {
                file_name_of(&c.path).to_string()
            },
            description: Some(c.path.clone()),
        })
        .collect();
    Some((token.prefix, items))
}

/// 纯路径补全入口：提取路径 token → readdir 候选 → 打包菜单数据
/// （对照 pi `extractPathPrefix(force)` → `getFileSuggestions`）。
///
/// 输入: text_before — 光标前文本；base_dir — 基准目录（cwd）；
///       force — force 语义（空 token = cwd 列表；非 force 空 token 不触发）
/// 输出: Some((待替换前缀, 候选)) = 应开菜单；None = 无 token/无候选（静默）
pub fn path_suggestions(
    text_before: &str,
    base_dir: &Path,
    force: bool,
) -> Option<(String, Vec<CompletionItem>)> {
    let token = extract_path_token(text_before, force)?;
    let cands = list_dir_candidates(&token.query, base_dir);
    if cands.is_empty() {
        return None; // pi: suggestions.length === 0 → 不弹/关
    }
    let items = cands
        .iter()
        .map(|c| CompletionItem {
            value: build_path_value(&token, c),
            label: if c.is_dir {
                format!("{}/", file_name_of(&c.path))
            } else {
                file_name_of(&c.path).to_string()
            },
            description: None, // spec: Path 候选无描述列
        })
        .collect();
    Some((token.prefix, items))
}

// ── 纯路径补全（Tab 触发，对照 pi `extractPathPrefix`） ──

/// 纯路径补全 token（光标前的路径片段，无 `@` 前缀）。
#[derive(Debug, Clone, PartialEq)]
pub struct PathToken {
    /// 待替换前缀（与光标前文本尾部逐字对应；未闭合引号形态含开引号、
    /// 前导包裹已剥离——包裹符留在替换区之前，应用时自然保留）。
    pub prefix: String,
    /// 候选查询（不含开引号，供 readdir 定位目录与前缀匹配）。
    pub query: String,
    /// 是否未闭引号形态（应用值恒加引号，对齐 pi `isQuotedPrefix`）。
    pub quoted: bool,
}

impl PathToken {
    /// 造空 token（force 下空输入/空白尾随的 cwd 列表语义）。
    fn empty() -> Self {
        PathToken { prefix: String::new(), query: String::new(), quoted: false }
    }
}

/// 前导包裹符映射（pi `PATH_WRAPPERS`：开括号 → 闭括号）。
const PATH_WRAPPERS: [(char, char); 5] =
    [('(', ')'), ('[', ']'), ('{', '}'), ('<', '>'), ('`', '`')];

/// 剥离 token 前导未闭合包裹符（pi `stripLeadingWrappers`）：仅当闭符
/// 不在余串中才剥——`(x)` 成对整体保留，`(src/x` 剥为 `src/x`。
///
/// 输入: token — 待剥离片段
/// 输出: 剥离后的子串（多数情形原样返回；不分配）
fn strip_leading_wrappers(token: &str) -> &str {
    let mut result = token;
    while let Some(first) = result.chars().next() {
        let Some((_, closer)) = PATH_WRAPPERS.iter().find(|(open, _)| *open == first) else {
            break;
        };
        // 余串已含闭符 → 成对包裹不剥（开符皆 ASCII，`first.len_utf8()` 安全）
        if result[first.len_utf8()..].contains(*closer) {
            break;
        }
        result = &result[first.len_utf8()..];
    }
    result
}

/// 定位未闭合 `"` 的起始字节下标（pi `findUnclosedQuoteStart`）：逐字符
/// 切换引号态，仅在进入引号时记录位置，末态仍在引号内才返回。
///
/// 输入: text — 光标前文本
/// 输出: Some(开引号字节下标) = 存在未闭合引号；None = 引号全闭合
fn find_unclosed_quote_start(text: &str) -> Option<usize> {
    let mut in_quotes = false;
    let mut quote_start = 0usize;
    for (i, c) in text.char_indices() {
        if c == '"' {
            in_quotes = !in_quotes;
            if in_quotes {
                quote_start = i;
            }
        }
    }
    in_quotes.then_some(quote_start)
}

/// 判定开引号是否处于 token 起始（pi `isTokenStart`，边界集为本项目
/// `is_token_boundary`；CJK 标点边界挂账）：向前跳过前导包裹开符后，
/// 前一字符为分隔符、或位置即行首。
///
/// 输入: text — 光标前文本；quote_pos — 开引号字节下标（来自 char_indices）
/// 输出: true = 引号形态成立，可作路径 token；false = 回落分隔符切片
fn is_quote_token_start(text: &str, quote_pos: usize) -> bool {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut idx = chars.iter().position(|(i, _)| *i == quote_pos).unwrap_or(0);
    while idx > 0 && PATH_WRAPPERS.iter().any(|(open, _)| *open == chars[idx - 1].1) {
        idx -= 1;
    }
    idx == 0 || is_token_boundary(chars[idx - 1].1)
}

/// 从光标前文本提取纯路径 token（对照 pi `extractPathPrefix`）。
///
/// 输入: text_before — 光标前的当前行文本；force — Tab 强制触发语义
/// 输出: Some(token) = 处于路径补全上下文；None = 不触发。空 token 仅
///       force 下返回 Some("")（列 cwd 顶层），非 force 返回 None；
///       未闭合引号形态 prefix 含开引号、query 不含。边界为空白/`"`/`'`/`=`
///       （CJK 标点边界挂账），支持 `([{<\`` 前导包裹剥离。
pub fn extract_path_token(text_before: &str, force: bool) -> Option<PathToken> {
    // ① 未闭合引号形态（pi extractQuotedPrefix）：开引号处于 token 起始
    if let Some(qpos) = find_unclosed_quote_start(text_before)
        .filter(|qpos| is_quote_token_start(text_before, *qpos))
    {
        return Some(PathToken {
            prefix: text_before[qpos..].to_string(),
            query: text_before[qpos + 1..].to_string(), // 跳过开引号
            quoted: true,
        });
    }
    // ② 分隔符切片 + 前导包裹剥离（pi findLastDelimiter → stripLeadingWrappers）
    let start = text_before
        .char_indices()
        .rev()
        .find(|(_, c)| is_token_boundary(*c))
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    let token = strip_leading_wrappers(&text_before[start..]);
    if token.is_empty() {
        // 空 token：force → Some("")（列 cwd 顶层）；非 force → 不触发
        return force.then(PathToken::empty);
    }
    Some(PathToken { prefix: token.to_string(), query: token.to_string(), quoted: false })
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
        CompletionKind::AtFile => {
            // `@token` → value：文件补空格、目录不补（续写子路径）；
            // 引号内目录光标落在闭引号前（可继续在引号内输入）
            let is_dir = value.ends_with('/') || value.ends_with("/\"");
            let suffix = if is_dir { "" } else { " " };
            let new_line = format!("{before}{value}{suffix}{after}");
            let mut cursor_in_value = value.chars().count();
            if is_dir && value.ends_with('"') {
                cursor_in_value -= 1; // 闭引号前
            }
            let new_cursor = start_char + cursor_in_value + suffix.len();
            (new_line, new_cursor)
        }
        CompletionKind::Path => {
            // 纯路径 token → value：无 `@`、无尾随空格（spec：光标落插入
            // 文本之后）；引号内目录光标落闭引号前（可续写，对齐 pi
            // applyCompletion 与 AtFile 规则）
            let new_line = format!("{before}{value}{after}");
            let mut cursor_in_value = value.chars().count();
            if value.ends_with("/\"") {
                cursor_in_value -= 1; // 闭引号前
            }
            let new_cursor = start_char + cursor_in_value;
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

    // ── `@文件` 补全（extract / list / apply / fd） ──

    /// 造临时基准目录：main.rs、README.md、src/{login.rs,lib/}、login_dir/、
    /// 20 个 fn*.txt；返回 (根目录, 清理句柄语义=测试末尾自删)。
    fn temp_base(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pi-atfile-{}-{tag}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("src/lib")).unwrap();
        std::fs::create_dir_all(dir.join("login_dir")).unwrap();
        std::fs::write(dir.join("main.rs"), "fn main(){}").unwrap();
        std::fs::write(dir.join("README.md"), "# hi").unwrap();
        std::fs::write(dir.join("src/login.rs"), "fn login(){}").unwrap();
        std::fs::write(dir.join("src/lib/util.rs"), "").unwrap();
        for i in 0..20 {
            std::fs::write(dir.join(format!("fn{i}.txt")), "").unwrap();
        }
        dir
    }

    #[test]
    fn extract_at_token_forms_and_boundaries() {
        // 裸 `@` 与路径
        assert_eq!(
            extract_at_token("@"),
            Some(AtToken { prefix: "@".into(), query: "".into(), quoted: false })
        );
        assert_eq!(
            extract_at_token("hello @src/ma"),
            Some(AtToken { prefix: "@src/ma".into(), query: "src/ma".into(), quoted: false })
        );
        // 分隔符边界：`=` 与 `'` 也算 token 起始
        assert_eq!(extract_at_token("=@foo").map(|t| t.query), Some("foo".into()));
        assert_eq!(extract_at_token("'@foo").map(|t| t.query), Some("foo".into()));
        // `@"quoted` 未闭合容忍
        assert_eq!(
            extract_at_token(r#"he said \""my fi"#),
            None,
            "无 @ 前缀不触发"
        );
        let t = extract_at_token(r#"@"my fi"#).expect("未闭合引号 token");
        assert!(t.quoted && t.query == "my fi" && t.prefix == r#"@"my fi"#, "{t:?}");
        // 引号已闭合 → 不触发（光标在闭引号后）
        assert_eq!(extract_at_token(r#"@"my file" "#), None);
        assert_eq!(extract_at_token(r#"@"my file""#), None, "光标在闭引号处");
        // 词中 @ 不触发；斜杠 token 不误触发
        assert_eq!(extract_at_token("x@foo"), None);
        assert_eq!(extract_at_token("/thi"), None);
        assert_eq!(extract_at_token("see src/app"), None);
        assert_eq!(extract_at_token(""), None);
    }

    #[test]
    fn extract_path_token_forms_and_force_semantics() {
        // 空输入：force → Some("")（列 cwd），非 force → None
        assert_eq!(extract_path_token("", true), Some(PathToken::empty()), "空输入 force 空 token");
        assert_eq!(extract_path_token("", false), None, "非 force 空 token 不触发");
        // 尾随空白：token 为空（光标前一字符是空白的主场景）
        assert_eq!(extract_path_token("look ", true), Some(PathToken::empty()), "空白尾随 → 空 token");
        assert_eq!(extract_path_token("look ", false), None);
        // 普通路径 token（含前置文本时从最后分隔符切）
        assert_eq!(
            extract_path_token("src/mai", true).map(|t| (t.prefix, t.query, t.quoted)),
            Some(("src/mai".into(), "src/mai".into(), false))
        );
        assert_eq!(extract_path_token("find src/mai", true).map(|t| t.query), Some("src/mai".into()));
        // `~/` 写法保留
        assert_eq!(extract_path_token("~/Do", true).map(|t| t.query), Some("~/Do".into()));
        // `=` 是分隔符（pi PATH_DELIMITERS）：`a=b` → token `b`
        assert_eq!(extract_path_token("a=b", true).map(|t| t.query), Some("b".into()), "`=` 为边界");
        // 未闭合引号：prefix 含开引号、query 不含、quoted=true；空格在引号内不切 token
        let t = extract_path_token(r#""my di"#, true).expect("未闭合引号 token");
        assert_eq!((t.prefix.as_str(), t.query.as_str(), t.quoted), (r#""my di"#, "my di", true));
        let t = extract_path_token(r#"he said "my di"#, true).expect("前有文本的未闭合引号");
        assert_eq!((t.prefix.as_str(), t.query.as_str()), (r#""my di"#, "my di"));
        // 前导包裹剥离：`(src/x` → `src/x`（包裹符留在替换区之前）
        assert_eq!(extract_path_token("(src/x", true).map(|t| t.query), Some("src/x".into()), "解包裹");
        // 成对包裹不剥：`(src/x)` 整体保留
        assert_eq!(extract_path_token("(src/x)", true).map(|t| t.query), Some("(src/x)".into()), "成对不剥");
        // 引号已闭合（字面 `"(src/x"`）→ 回落分隔符切片，尾部 `"` 是边界 → 空 token
        assert_eq!(extract_path_token(r#""(src/x""#, true), Some(PathToken::empty()), "闭合引号 → 尾部边界切为空");
        // 非 force 非空 token 照常提取（force 只影响空 token 分支）
        assert_eq!(extract_path_token("src/mai", false).map(|t| t.query), Some("src/mai".into()));
    }

    #[test]
    fn list_at_candidates_prefix_sort_and_cap() {
        let base = temp_base("list");
        // 裸前缀：大小写不敏感、目录优先、字母序
        let c = list_at_candidates("m", &base, None);
        assert!(c.iter().any(|x| x.path == "main.rs"), "{c:?}");
        let c = list_at_candidates("MAIN", &base, None);
        assert_eq!(c.len(), 1, "大小写不敏感: {c:?}");
        // 目录列内容 + 展示路径保留 `src/`
        let c = list_at_candidates("src/", &base, None);
        let paths: Vec<&str> = c.iter().map(|x| x.path.as_str()).collect();
        assert!(paths.contains(&"src/login.rs") && paths.contains(&"src/lib"), "{paths:?}");
        assert!(c.iter().find(|x| x.path == "src/lib").unwrap().is_dir);
        // `./` 写法保留
        let c = list_at_candidates("./", &base, None);
        assert!(c.iter().any(|x| x.path == "./main.rs"), "{c:?}");
        // 目录在同分段优先（dir +10）
        let c = list_at_candidates("login", &base, None);
        assert_eq!(c[0].path, "login_dir", "目录加分优先: {c:?}");
        assert!(c[0].is_dir);
        // top20 截断
        let c = list_at_candidates("fn", &base, None);
        assert_eq!(c.len(), 20, "上限 20");
        // 不存在的目录 → 静默空
        assert!(list_at_candidates("nope-dir/", &base, None).is_empty());
        // 排序：分数 > 深度 > 长度 > 字典序（fd 注入：深层弱匹配排后）
        let deep = base.join("src/lib/util.rs").to_string_lossy().to_string();
        let c = list_at_candidates("util", &base, Some(&format!("{deep}\n")));
        assert_eq!(c.len(), 1, "fd 候选并入: {c:?}");
        assert!(c[0].path.ends_with("util.rs"));
        // 去重：fd 行与基础候选同路径只留一条
        let top = base.join("main.rs").to_string_lossy().to_string();
        let c = list_at_candidates("main", &base, Some(&format!("{top}\n")));
        assert_eq!(c.iter().filter(|x| x.path == "main.rs").count(), 1, "{c:?}");
        // 空查询：全体保留（score=1），基础+fd 合并
        let c = list_at_candidates("", &base, Some(deep.as_str()));
        assert!(c.len() >= 6, "空查询列全: {}", c.len());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn list_dir_candidates_sort_without_cap() {
        let base = temp_base("dircand");
        // 空查询：全量列出，不截断 20（顶层 24 项：2 目录 + 22 文件）
        let c = list_dir_candidates("", &base);
        assert!(c.len() > 20, "无 20 条上限，实得 {}", c.len());
        // 目录优先且目录间字母序：前两条即全部目录
        let paths: Vec<&str> = c.iter().map(|x| x.path.as_str()).collect();
        assert_eq!(&paths[..2], &["login_dir", "src"], "目录优先+字母序: {paths:?}");
        assert!(c[2..].iter().all(|x| !x.is_dir), "第 3 条起全是文件: {paths:?}");
        // 文件间字母序
        let files = &paths[2..];
        let mut want = files.to_vec();
        want.sort_unstable();
        assert_eq!(files, want, "文件间字母序: {files:?}");
        // 前缀匹配：大小写不敏感
        assert!(list_dir_candidates("MAIN", &base).iter().any(|x| x.path == "main.rs"));
        // 目录部分定位 + 展示路径保留 `src/`
        let c = list_dir_candidates("src/", &base);
        assert!(c.iter().any(|x| x.path == "src/login.rs"), "{c:?}");
        // 不存在的目录 → 静默空
        assert!(list_dir_candidates("nope-dir/", &base).is_empty());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn score_entry_matches_pi_table() {
        assert_eq!(score_entry("main.rs", "main.rs", false), 100, "精确");
        assert_eq!(score_entry("main.rs", "mai", false), 80, "前缀");
        assert_eq!(score_entry("main.rs", "ain", false), 50, "文件名子串");
        assert_eq!(score_entry("src/main.rs", "src/ma", false), 30, "路径子串");
        assert_eq!(score_entry("main.rs", "zzz", false), 0, "不匹配");
        assert_eq!(score_entry("src", "src", true), 110, "目录 +10");
        // 解析容错
        assert_eq!(parse_fd_lines("a/b\n\n  \nc\r\n"), vec!["a/b", "c"]);
        assert!(parse_fd_lines("").is_empty());
    }

    #[test]
    fn apply_at_candidate_file_dir_and_quoted() {
        let file = AtCandidate { path: "src/main.rs".into(), is_dir: false, score: 80 };
        let dir = AtCandidate { path: "src".into(), is_dir: true, score: 90 };
        let tok = |q: &str, quoted: bool| AtToken {
            prefix: if quoted { format!("@\"{q}") } else { format!("@{q}") },
            query: q.into(),
            quoted,
        };
        // 文件：补空格、光标在空格后
        let (l, c) = apply_completion("@ma", 3, "@ma", &build_at_value(&tok("ma", false), &file), &CompletionKind::AtFile);
        assert_eq!(l, "@src/main.rs ");
        assert_eq!(c, 13, "光标在空格后（12+1）");
        // 目录：不补空格、光标在 `/` 后可续写
        let (l, c) = apply_completion("@s", 2, "@s", &build_at_value(&tok("s", false), &dir), &CompletionKind::AtFile);
        assert_eq!(l, "@src/");
        assert_eq!(c, 5, "光标在 / 后");
        // 保留光标后文本
        let (l, c) = apply_completion("@ma tail", 3, "@ma", "@src/main.rs", &CompletionKind::AtFile);
        assert_eq!(l, "@src/main.rs  tail", "后缀保留: {l}");
        assert_eq!(c, 13);
        // 引号形态：文件带闭引号+空格；目录光标在闭引号前
        let quoted_tok = tok("my fi", true);
        let qfile = AtCandidate { path: "my file.txt".into(), is_dir: false, score: 50 };
        let v = build_at_value(&quoted_tok, &qfile);
        assert_eq!(v, r#"@"my file.txt""#, "引号恒加");
        let (l, c) = apply_completion(r#"@"my fi"#, 7, r#"@"my fi"#, &v, &CompletionKind::AtFile);
        assert_eq!(l, r#"@"my file.txt" "#);
        assert_eq!(c, 15, "光标在空格后");
        let qdir = AtCandidate { path: "my dir".into(), is_dir: true, score: 90 };
        let v = build_at_value(&quoted_tok, &qdir);
        assert_eq!(v, r#"@"my dir/""#);
        let (l, c) = apply_completion(r#"@"my fi"#, 7, r#"@"my fi"#, &v, &CompletionKind::AtFile);
        assert_eq!(l, r#"@"my dir/""#, "目录不加空格");
        assert_eq!(c, 9, "闭引号前（可继续引号内输入）: {c}");
        // 无引号需要时不加（普通路径）
        let plain = AtToken { prefix: "@a".into(), query: "a".into(), quoted: false };
        assert_eq!(build_at_value(&plain, &file), "@src/main.rs");
        // 需要引号：路径含空白
        let spaced = AtCandidate { path: "my dir/x".into(), is_dir: false, score: 50 };
        assert_eq!(build_at_value(&plain, &spaced), r#"@"my dir/x""#, "空白触发引号");
    }

    #[test]
    fn apply_completion_path_rules() {
        let p = |q: &str, quoted: bool| PathToken {
            prefix: if quoted { format!("\"{q}") } else { q.to_string() },
            query: q.to_string(),
            quoted,
        };
        let file = AtCandidate { path: "src/main.rs".into(), is_dir: false, score: 0 };
        let dir = AtCandidate { path: "src/lib".into(), is_dir: true, score: 0 };
        // 值构造：无 `@`、无尾随空格、目录保 `/`、空白触发引号
        assert_eq!(build_path_value(&p("mai", false), &file), "src/main.rs");
        assert_eq!(build_path_value(&p("li", false), &dir), "src/lib/");
        let spaced = AtCandidate { path: "my dir/foo.txt".into(), is_dir: false, score: 0 };
        assert_eq!(build_path_value(&p("fo", false), &spaced), r#""my dir/foo.txt""#, "空白触发引号");
        // 引号 token 恒加引号（即使值无空白）
        assert_eq!(build_path_value(&p("mai", true), &file), r#""src/main.rs""#, "quoted token 恒引号");
        // 文件：替换 token、无尾随空格、光标在插入文本后
        let (l, c) = apply_completion("src/mai", 7, "src/mai", "src/main.rs", &CompletionKind::Path);
        assert_eq!(l, "src/main.rs");
        assert_eq!(c, 11, "光标在插入文本后");
        // 目录：保 `/`、光标在 `/` 后可续写
        let (l, c) = apply_completion("src/li", 6, "src/li", "src/lib/", &CompletionKind::Path);
        assert_eq!(l, "src/lib/");
        assert_eq!(c, 8, "光标在 / 后");
        // 保留光标后文本（after 自带原空格，插入未追加新空格）
        let (l, c) = apply_completion("src/mai tail", 7, "src/mai", "src/main.rs", &CompletionKind::Path);
        assert_eq!(l, "src/main.rs tail", "后缀保留: {l}");
        assert_eq!(c, 11);
        // 带空白引号：prefix 含开引号被整体替换，值带闭引号（prefix 6 字符）
        let (l, c) = apply_completion(r#""my di"#, 6, r#""my di"#, r#""my dir/foo.txt""#, &CompletionKind::Path);
        assert_eq!(l, r#""my dir/foo.txt""#);
        assert_eq!(c, 16, "光标在闭引号后");
        // 引号目录：光标落闭引号前（可续写，对齐 pi/AtFile）
        let (l, c) = apply_completion(r#""my di"#, 6, r#""my di"#, r#""my dir/lib/""#, &CompletionKind::Path);
        assert_eq!(l, r#""my dir/lib/""#);
        assert_eq!(c, 12, "闭引号前: {c}");
        // `~` 前缀保留
        let tilde = AtCandidate { path: "~/Documents".into(), is_dir: false, score: 0 };
        let v = build_path_value(&p("~/Do", false), &tilde);
        assert_eq!(v, "~/Documents");
        let (l, c) = apply_completion("~/Do", 4, "~/Do", &v, &CompletionKind::Path);
        assert_eq!(l, "~/Documents");
        assert_eq!(c, 11, "~/Documents = 11 字符");
    }

    #[test]
    fn fd_args_and_timeout_degradation() {
        let base = std::path::Path::new("/tmp");
        let args = fd_args("login", base);
        let strs: Vec<String> = args.iter().map(|a| a.to_string_lossy().to_string()).collect();
        assert!(strs.contains(&"-F".to_string()) && strs.contains(&"login".to_string()), "{strs:?}");
        assert!(strs.contains(&"100".to_string()) && strs.contains(&"f".to_string()));
        assert_eq!(strs.last().unwrap(), "/tmp");
        let empty_args = fd_args("", base);
        assert!(!empty_args.iter().any(|a| a == "-F"), "空查询不带查询词");
        // 成功路径
        let out = run_cmd_with_timeout("echo", &["hi".into()], std::time::Duration::from_millis(800));
        assert_eq!(out.as_deref(), Some("hi\n"));
        // 超时 → None（降级路径），且不悬挂
        let t0 = std::time::Instant::now();
        let out = run_cmd_with_timeout("sleep", &["5".into()], std::time::Duration::from_millis(100));
        assert!(out.is_none(), "超时返回 None");
        assert!(t0.elapsed() < std::time::Duration::from_secs(2), "应快速返回");
        // 启动失败（无此命令）→ None
        assert!(run_cmd_with_timeout("fd-no-such-cmd-xyz", &[], std::time::Duration::from_millis(100)).is_none());
    }

    #[test]
    fn at_suggestions_end_to_end() {
        let base = temp_base("e2e");
        // `@` → 顶层候选、prefix 为 `@`
        let (prefix, items) = at_suggestions("look @", &base).expect("空 @ 应触发");
        assert_eq!(prefix, "@");
        assert!(items.iter().any(|i| i.value == "@main.rs"), "{items:?}");
        assert!(items.iter().any(|i| i.value == "@src/"), "目录带 /: {items:?}");
        // 续输重过滤
        let (_, items) = at_suggestions("look @src/", &base).expect("src/ 应触发");
        assert!(items.iter().any(|i| i.value == "@src/login.rs"), "{items:?}");
        // 无 @ → 不触发；斜杠行不干扰
        assert!(at_suggestions("plain text", &base).is_none());
        assert!(at_suggestions("/thi", &base).is_none());
        // 无候选（不存在目录）→ 静默 None
        assert!(at_suggestions("@no-such-dir/", &base).is_none());
        std::fs::remove_dir_all(&base).ok();
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
