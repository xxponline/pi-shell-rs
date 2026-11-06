//! 会话树的展平、折叠与过滤（纯函数，对照 pi 的 tree-selector applyFilter）。

use std::collections::HashSet;

use serde_json::Value;

/// 树过滤模式（与 pi 的 FilterMode 同序循环）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    Default,
    NoTools,
    UserOnly,
    LabeledOnly,
    All,
}

impl FilterMode {
    /// 循环到下一模式（ctrl+o）。
    ///
    /// 输入: self — 当前模式
    /// 输出: 下一个模式
    pub fn next(self) -> Self {
        use FilterMode::*;
        match self {
            Default => NoTools,
            NoTools => UserOnly,
            UserOnly => LabeledOnly,
            LabeledOnly => All,
            All => Default,
        }
    }

    /// 模式显示名（标题栏用）。
    pub fn label(&self) -> &'static str {
        match self {
            FilterMode::Default => "default",
            FilterMode::NoTools => "no-tools",
            FilterMode::UserOnly => "user-only",
            FilterMode::LabeledOnly => "labeled-only",
            FilterMode::All => "all",
        }
    }
}

/// 一行可见树节点。
#[derive(Debug, Clone, PartialEq)]
pub struct TreeRow {
    /// 条目 id（fork 的 entryId）。
    pub id: String,
    /// 缩进层级（0 起）。
    pub depth: usize,
    /// 显示文本（角色/类型 + 预览）。
    pub text: String,
    /// 是否带标签（labeled-only 过滤用）。
    pub labeled: bool,
    /// 是否当前 leaf。
    pub is_leaf: bool,
    /// 是否有子节点（可折叠）。
    pub has_children: bool,
}

/// 把 get_tree 的节点数组展平为可见行（应用过滤、搜索与折叠）。
///
/// 输入: tree — get_tree 的 tree 数组（弱类型）；leaf_id — 当前 leaf；
///       folded — 已折叠节点 id 集；filter — 过滤模式；
///       search — 搜索查询（空白分词；空串 = 不过滤）
/// 输出: 按可见顺序的行序列（被过滤/搜索/折叠裁剪的节点不出现）
pub fn flatten_tree(
    tree: &[Value],
    leaf_id: Option<&str>,
    folded: &HashSet<String>,
    filter: FilterMode,
    search: &str,
) -> Vec<TreeRow> {
    // 查询小写化+分词只做一次（walk 内逐节点复用）
    let tokens: Vec<String> = search
        .to_lowercase()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let mut rows = Vec::new();
    walk(tree, 0, leaf_id, folded, filter, &tokens, &mut rows);
    rows
}

/// 查询/过滤变化后的选择迁移：原选中行仍可见则保持（按 id 定位），
/// 否则钳制到最近有效索引；空结果安全归 0。
///
/// 输入: rows — 变化后的可见行；cursor — 当前选择索引（可变）；
///       prev_id — 变化前选中行的 id（None 时仅做钳制）
/// 输出: cursor 就地更新为有效索引（0..rows.len()，空表为 0）
pub fn keep_cursor(rows: &[TreeRow], cursor: &mut usize, prev_id: Option<&str>) {
    if let Some(id) = prev_id
        && let Some(idx) = rows.iter().position(|r| r.id == id)
    {
        *cursor = idx;
        return;
    }
    *cursor = (*cursor).min(rows.len().saturating_sub(1));
}

/// 深度优先遍历单层节点列表。
///
/// 顺序：过滤模式 → 搜索命中 → 折叠裁剪（逐节点过滤：祖先不命中时
/// 命中的后代仍保留，对齐 pi 的 flat 过滤）。
fn walk(
    nodes: &[Value],
    depth: usize,
    leaf_id: Option<&str>,
    folded: &HashSet<String>,
    filter: FilterMode,
    tokens: &[String],
    out: &mut Vec<TreeRow>,
) {
    for node in nodes {
        let Some(entry) = node.get("entry") else { continue };
        let id = entry
            .get("id")
            .and_then(|i| i.as_str())
            .unwrap_or("")
            .to_string();
        let children = node
            .get("children")
            .and_then(|c| c.as_array())
            .cloned()
            .unwrap_or_default();
        let node_label = node.get("label").and_then(|l| l.as_str());
        let labeled = node_label.is_some();

        if passes_filter(entry, &id, leaf_id, labeled, filter)
            && matches_search(entry, node_label, tokens)
        {
            out.push(TreeRow {
                id: id.clone(),
                depth,
                text: entry_preview(entry),
                labeled,
                is_leaf: leaf_id == Some(id.as_str()),
                has_children: !children.is_empty(),
            });
        }

        // 折叠：有子且被折叠 → 只显示自身，跳过子树（对齐 pi skipSet）
        if !children.is_empty() && folded.contains(&id) {
            continue;
        }
        walk(&children, depth + 1, leaf_id, folded, filter, tokens, out);
    }
}

/// 搜索命中判定：所有 token（已小写）必须同时出现在可搜索文本中。
///
/// 输入: entry — 条目 JSON；label — 节点级标签；tokens — 小写化查询词
/// 输出: true = 命中（tokens 为空视为命中）
fn matches_search(entry: &Value, label: Option<&str>, tokens: &[String]) -> bool {
    if tokens.is_empty() {
        return true;
    }
    let hay = searchable_text(entry, label).to_lowercase();
    tokens.iter().all(|t| hay.contains(t.as_str()))
}

/// 单条过滤判定（对照 pi applyFilter：usage 恒隐、无文本纯工具 assistant
/// 隐（leaf 例外）、按模式过滤记账条目）。
fn passes_filter(
    entry: &Value,
    id: &str,
    leaf_id: Option<&str>,
    labeled: bool,
    filter: FilterMode,
) -> bool {
    let entry_type = entry.get("type").and_then(|t| t.as_str()).unwrap_or("");
    // pi: usage 恒隐（所有模式）
    if entry_type == "usage" {
        return false;
    }
    let is_current_leaf = leaf_id == Some(id);

    // 无文本且非错误/中止的 assistant 消息隐藏（当前 leaf 例外）
    if entry_type == "message"
        && entry.pointer("/message/role").and_then(|r| r.as_str()) == Some("assistant")
        && !is_current_leaf
    {
        let has_text = entry
            .pointer("/message/content")
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter().any(|b| {
                    b.get("type").and_then(|t| t.as_str()) == Some("text")
                        && b.get("text").and_then(|t| t.as_str()).is_some_and(|t| !t.trim().is_empty())
                })
            })
            .unwrap_or(false);
        let stop = entry.pointer("/message/stopReason").and_then(|s| s.as_str());
        let is_error_or_aborted =
            stop.is_some_and(|s| s != "stop" && s != "toolUse");
        if !has_text && !is_error_or_aborted {
            return false;
        }
    }

    // 记账类条目（default/no-tools 隐）
    let is_settings = matches!(
        entry_type,
        "label" | "context_edit" | "custom" | "model_change" | "thinking_level_change" | "session_info"
    );

    match filter {
        FilterMode::UserOnly => {
            entry_type == "message"
                && entry.pointer("/message/role").and_then(|r| r.as_str()) == Some("user")
        }
        FilterMode::NoTools => {
            !is_settings
                && !(entry_type == "message"
                    && entry.pointer("/message/role").and_then(|r| r.as_str())
                        == Some("toolResult"))
        }
        FilterMode::LabeledOnly => labeled,
        FilterMode::All => true,
        FilterMode::Default => !is_settings,
    }
}

/// 可搜索文本：对照 pi tree-selector 的 getSearchableText，按条目类型拼接
/// 可匹配字段（含节点级标签）。
///
/// 输入: entry — 会话条目 JSON；label — get_tree 节点上的标签（可空）
/// 输出: 空白拼接的可搜索串（调用方自行小写化做子串匹配）
pub fn searchable_text(entry: &Value, label: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(l) = label.filter(|l| !l.is_empty()) {
        parts.push(l.to_string());
    }
    let entry_type = entry.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match entry_type {
        "message" => {
            let role = entry
                .pointer("/message/role")
                .and_then(|r| r.as_str())
                .unwrap_or("");
            parts.push(role.to_string());
            parts.push(message_text(entry.pointer("/message/content")));
            // 工具结果常无 content、bash 带 command：这些字段是唯一可搜内容
            if (role == "toolResult" || role == "bashExecution")
                && let Some(msg) = entry.get("message")
            {
                parts.push(as_str_or(msg, if role == "toolResult" { "toolName" } else { "command" }));
            }
        }
        "custom_message" => {
            parts.push(as_str_or(entry, "customType"));
            match entry.get("content") {
                Some(Value::String(s)) => parts.push(s.clone()),
                Some(other) => parts.push(message_text(Some(other))),
                None => {}
            }
        }
        "compaction" => {
            parts.push("compaction".to_string());
            parts.push(as_str_or(entry, "summary"));
        }
        "branch_summary" => {
            parts.push("branch summary".to_string());
            parts.push(as_str_or(entry, "summary"));
        }
        "session_info" => {
            parts.push("title".to_string());
            parts.push(as_str_or(entry, "name"));
        }
        "model_change" => {
            parts.push("model".to_string());
            parts.push(as_str_or(entry, "provider"));
            parts.push(as_str_or(entry, "modelId"));
        }
        "thinking_level_change" => {
            parts.push("thinking".to_string());
            parts.push(as_str_or(entry, "thinkingLevel"));
        }
        "custom" => {
            parts.push("custom".to_string());
            parts.push(as_str_or(entry, "customType"));
        }
        "context_edit" => {
            parts.push("context edit".to_string());
            parts.push(if entry.get("replacement").is_none_or(|r| r.is_null()) {
                "omit".to_string()
            } else {
                "replace".to_string()
            });
            parts.push(as_str_or(entry, "targetId"));
        }
        "label" => {
            parts.push("label".to_string());
            parts.push(as_str_or(entry, "label"));
        }
        // 未知类型退化为类型名本身（与预览的 other 分支一致）
        other => parts.push(other.to_string()),
    }
    parts.join(" ")
}

/// 条目显示文本：角色/类型 + 内容预览。
///
/// 输入: entry — 会话条目（弱类型）
/// 输出: 单行预览（截断 40 字符）
pub fn entry_preview(entry: &Value) -> String {
    let entry_type = entry.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match entry_type {
        "message" => {
            let role = entry
                .pointer("/message/role")
                .and_then(|r| r.as_str())
                .unwrap_or("?");
            let preview = match role {
                "toolResult" => {
                    let tool = entry
                        .pointer("/message/toolName")
                        .and_then(|t| t.as_str())
                        .unwrap_or("tool");
                    return format!("▶ {tool} 结果");
                }
                "assistant" => message_text(entry.pointer("/message/content")),
                "user" => message_text(entry.pointer("/message/content")),
                _ => String::new(),
            };
            let tag = match role {
                "user" => "你",
                "assistant" => "AI",
                other => other,
            };
            format!("[{tag}] {}", truncate(&preview, 40))
        }
        "compaction" => format!("压缩: {}", truncate(&as_str_or(entry, "summary"), 40)),
        "label" => format!("标签: {}", truncate(&as_str_or(entry, "label"), 40)),
        "branch_summary" => format!("分支摘要: {}", truncate(&as_str_or(entry, "summary"), 40)),
        "model_change" => format!(
            "模型切换: {}/{}",
            as_str_or(entry, "provider"),
            as_str_or(entry, "modelId")
        ),
        "thinking_level_change" => {
            format!("思考级别 → {}", as_str_or(entry, "thinkingLevel"))
        }
        other => other.to_string(),
    }
}

/// 从 content 数组提取文本（text 块拼接；图片记占位）。
fn message_text(content: Option<&Value>) -> String {
    let Some(arr) = content.and_then(|c| c.as_array()) else {
        // content 可能是纯字符串
        return content
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
    };
    let parts: Vec<String> = arr
        .iter()
        .map(|b| match b.get("type").and_then(|t| t.as_str()) {
            Some("text") => b.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string(),
            Some("thinking") => String::new(), // 思考不进树预览
            Some("toolCall") => format!(
                "🔧{}",
                b.get("name").and_then(|n| n.as_str()).unwrap_or("tool")
            ),
            Some("image") => "[图片]".to_string(),
            _ => String::new(),
        })
        .filter(|s| !s.is_empty())
        .collect();
    parts.join(" ")
}

/// 取字符串字段（缺失返回空串）。
fn as_str_or(v: &Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

/// 字符安全截断。
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let taken: String = text.chars().take(max.saturating_sub(1)).collect();
        format!("{taken}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 造一棵两分支的小树：root(user+assistant) → fork1(user) / fork2(toolResult)。
    fn sample_tree() -> Vec<Value> {
        vec![json!({
            "entry": {"type":"message","id":"root","parentId":null,
                "message":{"role":"user","content":[{"type":"text","text":"最初的提问很长很长"}]}},
            "children": [
                {
                    "entry": {"type":"message","id":"a1","parentId":"root",
                        "message":{"role":"assistant","content":[{"type":"text","text":"第一个回答"}]}},
                    "children": [
                        {"entry":{"type":"message","id":"leaf","parentId":"a1",
                            "message":{"role":"user","content":[{"type":"text","text":"追问"}]}},
                         "children": []}
                    ]
                },
                {
                    "entry": {"type":"message","id":"b1","parentId":"root",
                        "message":{"role":"toolResult","toolName":"read"}},
                    "children": []
                }
            ]
        })]
    }

    #[test]
    fn searchable_text_covers_entry_types() {
        // message：role + 全文内容
        let m = json!({"type":"message","id":"m","message":{
            "role":"user","content":[{"type":"text","text":"login failed"}]}});
        let t = searchable_text(&m, None);
        assert!(t.contains("user") && t.contains("login failed"), "{t}");
        // bashExecution：额外带 command
        let b = json!({"type":"message","id":"b","message":{
            "role":"bashExecution","command":"cargo test"}});
        assert!(searchable_text(&b, None).contains("cargo test"));
        // session_info：title + name
        let si = json!({"type":"session_info","id":"s","name":"修复登录"});
        assert!(searchable_text(&si, None).contains("修复登录"));
        // model/thinking 变更
        let mc = json!({"type":"model_change","id":"m1","provider":"openai","modelId":"gpt-5"});
        let t = searchable_text(&mc, None);
        assert!(t.contains("model") && t.contains("gpt-5"), "{t}");
        let tc = json!({"type":"thinking_level_change","id":"t1","thinkingLevel":"high"});
        assert!(searchable_text(&tc, None).contains("high"));
        // compaction / branch_summary
        let cp = json!({"type":"compaction","id":"c","summary":"历史已压缩"});
        assert!(searchable_text(&cp, None).contains("历史已压缩"));
        let bs = json!({"type":"branch_summary","id":"b1","summary":"分支摘要内容"});
        assert!(searchable_text(&bs, None).contains("分支摘要内容"));
        // custom_message / custom / context_edit / label
        let cm = json!({"type":"custom_message","id":"x","customType":"ext.note","content":"备注文本"});
        let t = searchable_text(&cm, None);
        assert!(t.contains("ext.note") && t.contains("备注文本"), "{t}");
        let cu = json!({"type":"custom","id":"x2","customType":"foo"});
        assert!(searchable_text(&cu, None).contains("foo"));
        let ce = json!({"type":"context_edit","id":"e","replacement":null,"targetId":"t9"});
        let t = searchable_text(&ce, None);
        assert!(t.contains("omit") && t.contains("t9"), "{t}");
        let lb = json!({"type":"label","id":"l","targetId":"t","label":"重点"});
        assert!(searchable_text(&lb, None).contains("重点"));
        // 节点级标签参与搜索
        assert!(searchable_text(&m, Some("我的标签")).contains("我的标签"));
        // 缺字段不 panic、未知类型退化为类型名
        assert_eq!(searchable_text(&json!({"type":"weird","id":"w"}), None), "weird");
        let _ = searchable_text(&json!({"type":"message","id":"m2"}), None);
    }

    #[test]
    fn flatten_indent_children_and_leaf() {
        let rows = flatten_tree(&sample_tree(), Some("leaf"), &HashSet::new(), FilterMode::Default, "");
        // usage 无；root/a1/leaf 可见；b1 是 toolResult → default 可见（非 settings）
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["root", "a1", "leaf", "b1"], "{ids:?}");
        assert_eq!(rows[0].depth, 0);
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[2].depth, 2);
        assert_eq!(rows[3].depth, 1);
        assert!(rows[2].is_leaf, "leaf 标记");
        assert!(!rows[0].is_leaf);
        assert!(rows[0].has_children && !rows[3].has_children);
        // 预览
        assert!(rows[0].text.contains("最初的提问"), "{}", rows[0].text);
        assert!(rows[3].text.contains("▶ read"), "{}", rows[3].text);
    }

    #[test]
    fn fold_hides_subtree() {
        let mut folded = HashSet::new();
        folded.insert("a1".to_string());
        let rows = flatten_tree(&sample_tree(), Some("leaf"), &folded, FilterMode::Default, "");
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["root", "a1", "b1"], "a1 的子树被折叠: {ids:?}");
        // 展开恢复
        folded.clear();
        let rows = flatten_tree(&sample_tree(), Some("leaf"), &folded, FilterMode::Default, "");
        assert_eq!(rows.len(), 4);
    }

    #[test]
    fn filter_modes_match_pi_semantics() {
        let tree = sample_tree();
        let none = HashSet::new();
        // default：settings 类条目隐 → 造一个 session_info 条目验证
        let mut with_settings = tree.clone();
        with_settings[0]["children"]
            .as_array_mut()
            .unwrap()
            .push(json!({"entry":{"type":"session_info","id":"si","name":"n"},"children":[]}));
        let ids = |f: FilterMode| -> Vec<String> {
            flatten_tree(&with_settings, Some("leaf"), &none, f, "")
                .iter()
                .map(|r| r.id.clone())
                .collect()
        };
        assert!(!ids(FilterMode::Default).contains(&"si".to_string()), "default 隐记账");
        assert!(!ids(FilterMode::NoTools).contains(&"si".to_string()), "no-tools 隐记账");
        assert!(ids(FilterMode::All).contains(&"si".to_string()), "all 显示记账");
        assert!(!ids(FilterMode::UserOnly).contains(&"si".to_string()));

        // user-only：只剩 user 消息
        let users = ids(FilterMode::UserOnly);
        assert!(users.contains(&"root".to_string()) && users.contains(&"leaf".to_string()));
        assert!(!users.contains(&"a1".to_string()), "assistant 被过滤");

        // no-tools：隐 toolResult
        let nt = ids(FilterMode::NoTools);
        assert!(!nt.contains(&"b1".to_string()), "no-tools 隐 toolResult");
        assert!(nt.contains(&"a1".to_string()));
        // all：全显（除 usage）—— root + 3 children + 1 grandchild(leaf) = 5
        let all_ids = ids(FilterMode::All);
        assert_eq!(all_ids.len(), 5, "{all_ids:?}");
        assert!(all_ids.contains(&"leaf".to_string()));

        // labeled-only：无标签 → 空
        assert!(ids(FilterMode::LabeledOnly).is_empty());
    }

    #[test]
    fn filter_mode_cycle_order() {
        use FilterMode::*;
        assert_eq!(Default.next(), NoTools);
        assert_eq!(NoTools.next(), UserOnly);
        assert_eq!(UserOnly.next(), LabeledOnly);
        assert_eq!(LabeledOnly.next(), All);
        assert_eq!(All.next(), Default);
        assert_eq!(Default.label(), "default");
        assert_eq!(All.label(), "all");
    }

    #[test]
    fn search_tokens_and_case_insensitive() {
        let tree = sample_tree();
        let none = HashSet::new();
        let ids = |q: &str| -> Vec<String> {
            flatten_tree(&tree, Some("leaf"), &none, FilterMode::Default, q)
                .iter()
                .map(|r| r.id.clone())
                .collect()
        };
        // 单 token 命中
        assert_eq!(ids("追问"), vec!["leaf"], "叶子独有文本");
        // 多 token AND：同一节点须同时命中
        assert_eq!(ids("提问 最初"), vec!["root"]);
        // token 分散在不同节点 → 空
        assert!(ids("追问 回答").is_empty(), "AND 语义");
        // 大小写不敏感（b1 预览含工具名 read）
        assert!(ids("READ").contains(&"b1".to_string()));
        // 零命中
        assert!(ids("绝不存在的词").is_empty());
        // 空白查询 = 不过滤（与基线一致）
        assert_eq!(ids(""), ids("   \t "));
        assert_eq!(ids("").len(), 4);
    }

    #[test]
    fn search_keeps_matching_descendants_of_missed_ancestors() {
        // “追问”只在 leaf：root/a1 不命中但其后代仍保留（flat 过滤，缩进不变）
        let rows = flatten_tree(&sample_tree(), Some("leaf"), &HashSet::new(), FilterMode::Default, "追问");
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["leaf"], "{ids:?}");
        assert_eq!(rows[0].depth, 2, "缩进层级保持");
    }

    #[test]
    fn search_orders_after_filter_and_before_fold() {
        let tree = sample_tree();
        // 过滤在前：user-only 下搜“回答”（assistant 文本）→ 空
        assert!(
            flatten_tree(&tree, Some("leaf"), &HashSet::new(), FilterMode::UserOnly, "回答").is_empty(),
            "先过滤后搜索"
        );
        // user-only + 搜“追问” → leaf 通过两层
        let rows = flatten_tree(&tree, Some("leaf"), &HashSet::new(), FilterMode::UserOnly, "追问");
        assert_eq!(rows.len(), 1);
        // 折叠在后：折叠 a1 后即便 leaf 命中也被裁剪
        let mut folded = HashSet::new();
        folded.insert("a1".to_string());
        assert!(
            flatten_tree(&tree, Some("leaf"), &folded, FilterMode::Default, "追问").is_empty(),
            "折叠裁剪最后生效"
        );
        // 清空查询恢复完整可见集
        assert_eq!(
            flatten_tree(&tree, Some("leaf"), &folded, FilterMode::Default, "").len(),
            3
        );
    }

    #[test]
    fn keep_cursor_three_migrations() {
        let tree = sample_tree();
        let none = HashSet::new();
        let base = flatten_tree(&tree, Some("leaf"), &none, FilterMode::Default, "");
        let pos = |rows: &[TreeRow], id: &str| rows.iter().position(|r| r.id == id).unwrap();
        // 缩小：选中 leaf → 搜索“追问”后仍命中 → 按 id 保持
        let mut cursor = pos(&base, "leaf");
        let narrowed = flatten_tree(&tree, Some("leaf"), &none, FilterMode::Default, "追问");
        keep_cursor(&narrowed, &mut cursor, Some("leaf"));
        assert_eq!(narrowed[cursor].id, "leaf", "命中保持");
        // 扩大：选中 leaf → 清空查询 → 回到原位置
        let mut cursor = 2usize; // 基线中 leaf 的位置
        let widened = flatten_tree(&tree, Some("leaf"), &none, FilterMode::Default, "");
        keep_cursor(&widened, &mut cursor, Some("leaf"));
        assert_eq!(widened[cursor].id, "leaf");
        // 原选择消失：选中 a1 → 搜“追问”（a1 不命中）→ 钳制到有效索引
        let mut cursor = pos(&base, "a1");
        let narrowed = flatten_tree(&tree, Some("leaf"), &none, FilterMode::Default, "追问");
        keep_cursor(&narrowed, &mut cursor, Some("a1"));
        assert_eq!(narrowed[cursor].id, "leaf", "钳到最近有效: {cursor}");
        // 空结果安全
        let mut cursor = 3usize;
        let empty = flatten_tree(&tree, Some("leaf"), &none, FilterMode::Default, "绝无此词");
        keep_cursor(&empty, &mut cursor, Some("b1"));
        assert_eq!(cursor, 0, "空表归 0");
        assert!(empty.is_empty());
    }

    #[test]
    fn usage_always_hidden_and_error_assistant_shown() {
        let tree = vec![json!({
            "entry":{"type":"usage","id":"u1","kind":"cache_warm"},"children":[]
        }), {
            json!({
                "entry":{"type":"message","id":"err","parentId":null,
                    "message":{"role":"assistant","content":[],"stopReason":"error"}},
                "children":[]
            })
        }];
        let rows = flatten_tree(&tree, None, &HashSet::new(), FilterMode::All, "");
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert!(!ids.contains(&"u1"), "usage 恒隐（即便 all）");
        assert!(ids.contains(&"err"), "错误 assistant 显示");
        // 无文本非错误 assistant 隐
        let tree2 = vec![json!({
            "entry":{"type":"message","id":"toolonly","parentId":null,
                "message":{"role":"assistant","content":[{"type":"toolCall","id":"c","name":"read"}]}},
            "children":[]
        })];
        let rows = flatten_tree(&tree2, None, &HashSet::new(), FilterMode::Default, "");
        assert!(rows.is_empty(), "无文本纯工具 assistant 隐: {rows:?}");
        // 同一条作为 leaf 时显示
        let rows = flatten_tree(&tree2, Some("toolonly"), &HashSet::new(), FilterMode::Default, "");
        assert_eq!(rows.len(), 1, "leaf 例外");
    }
}
