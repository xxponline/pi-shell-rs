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

/// 把 get_tree 的节点数组展平为可见行（应用过滤与折叠）。
///
/// 输入: tree — get_tree 的 tree 数组（弱类型）；leaf_id — 当前 leaf；
///       folded — 已折叠节点 id 集；filter — 过滤模式
/// 输出: 按可见顺序的行序列（被过滤/折叠的子树不出现）
pub fn flatten_tree(
    tree: &[Value],
    leaf_id: Option<&str>,
    folded: &HashSet<String>,
    filter: FilterMode,
) -> Vec<TreeRow> {
    let mut rows = Vec::new();
    walk(tree, 0, leaf_id, folded, filter, &mut rows);
    rows
}

/// 深度优先遍历单层节点列表。
fn walk(
    nodes: &[Value],
    depth: usize,
    leaf_id: Option<&str>,
    folded: &HashSet<String>,
    filter: FilterMode,
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
        let labeled = node.get("label").and_then(|l| l.as_str()).is_some();

        if passes_filter(entry, &id, leaf_id, labeled, filter) {
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
        walk(&children, depth + 1, leaf_id, folded, filter, out);
    }
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
    fn flatten_indent_children_and_leaf() {
        let rows = flatten_tree(&sample_tree(), Some("leaf"), &HashSet::new(), FilterMode::Default);
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
        let rows = flatten_tree(&sample_tree(), Some("leaf"), &folded, FilterMode::Default);
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["root", "a1", "b1"], "a1 的子树被折叠: {ids:?}");
        // 展开恢复
        folded.clear();
        let rows = flatten_tree(&sample_tree(), Some("leaf"), &folded, FilterMode::Default);
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
            flatten_tree(&with_settings, Some("leaf"), &none, f)
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
        let rows = flatten_tree(&tree, None, &HashSet::new(), FilterMode::All);
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert!(!ids.contains(&"u1"), "usage 恒隐（即便 all）");
        assert!(ids.contains(&"err"), "错误 assistant 显示");
        // 无文本非错误 assistant 隐
        let tree2 = vec![json!({
            "entry":{"type":"message","id":"toolonly","parentId":null,
                "message":{"role":"assistant","content":[{"type":"toolCall","id":"c","name":"read"}]}},
            "children":[]
        })];
        let rows = flatten_tree(&tree2, None, &HashSet::new(), FilterMode::Default);
        assert!(rows.is_empty(), "无文本纯工具 assistant 隐: {rows:?}");
        // 同一条作为 leaf 时显示
        let rows = flatten_tree(&tree2, Some("toolonly"), &HashSet::new(), FilterMode::Default);
        assert_eq!(rows.len(), 1, "leaf 例外");
    }
}
