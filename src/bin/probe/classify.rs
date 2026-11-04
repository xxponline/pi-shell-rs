//! Weak structural classification of wire messages.
//!
//! Only discriminator fields are inspected; the raw JSON stays untouched in the
//! transcript. Counts feed `summary.json`.

use serde_json::Value;

/// A two-level classification label: category, optional sub-label.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Label {
    pub category: String,
    pub sub: Option<String>,
}

impl Label {
    pub fn new(category: impl Into<String>) -> Self {
        Self {
            category: category.into(),
            sub: None,
        }
    }

    pub fn with_sub(category: impl Into<String>, sub: impl Into<String>) -> Self {
        Self {
            category: category.into(),
            sub: Some(sub.into()),
        }
    }

    pub fn dotted(&self) -> String {
        match &self.sub {
            Some(s) => format!("{}.{}", self.category, s),
            None => category_only(self),
        }
    }
}

fn category_only(l: &Label) -> String {
    l.category.clone()
}

/// Classify one parsed wire value into a counting label.
///
/// Categories:
/// - `response.<command>` (with `.error` suffix when success=false)
/// - `message_update.<assistantMessageEvent.type>`
/// - `tool_execution_start|update|end`
/// - `extension_ui_request.<method>`
/// - any other top-level `type` as its own category
/// - `parse_error` for non-JSON lines
pub fn classify(value: Option<&Value>, parse_error: bool) -> Label {
    if parse_error {
        return Label::new("parse_error");
    }
    let v = match value {
        Some(v) => v,
        None => return Label::new("unknown"),
    };
    let top = v.get("type").and_then(|t| t.as_str()).unwrap_or("unknown");

    match top {
        "response" => {
            let cmd = v
                .get("command")
                .and_then(|c| c.as_str())
                .unwrap_or("unknown")
                .to_string();
            let ok = v.get("success").and_then(|s| s.as_bool()).unwrap_or(true);
            if ok {
                Label::with_sub("response", cmd)
            } else {
                Label::with_sub("response", format!("{cmd}.error"))
            }
        }
        "message_update" => {
            let sub = v
                .get("assistantMessageEvent")
                .and_then(|e| e.get("type"))
                .and_then(|t| t.as_str())
                .unwrap_or("unknown")
                .to_string();
            Label::with_sub("message_update", sub)
        }
        "extension_ui_request" => {
            let method = v
                .get("method")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown")
                .to_string();
            Label::with_sub("extension_ui_request", method)
        }
        other => Label::new(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    fn count(value: &Value) -> Label {
        classify(Some(value), false)
    }

    #[test]
    fn classifies_nested_streaming_and_tool_events() {
        let cases = vec![
            (json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta"}}),
             "message_update.text_delta"),
            (json!({"type":"message_update","assistantMessageEvent":{"type":"toolcall_start"}}),
             "message_update.toolcall_start"),
            (json!({"type":"tool_execution_start","toolCallId":"a"}),
             "tool_execution_start"),
            (json!({"type":"response","command":"get_state","success":true}),
             "response.get_state"),
            (json!({"type":"response","command":"prompt","success":false,"error":"x"}),
             "response.prompt.error"),
            (json!({"type":"extension_ui_request","method":"select"}),
             "extension_ui_request.select"),
            (json!({"type":"agent_start"}), "agent_start"),
        ];
        let mut seen: HashMap<String, u32> = HashMap::new();
        for (v, expected) in cases {
            let label = count(&v);
            assert_eq!(label.dotted(), expected, "input: {v}");
            *seen.entry(expected.to_string()).or_default() += 1;
        }
        // Mixed-batch counting sanity check.
        assert_eq!(seen["message_update.text_delta"], 1);
    }

    #[test]
    fn classifies_parse_errors_and_unknown() {
        assert_eq!(classify(None, true).dotted(), "parse_error");
        assert_eq!(count(&json!({})).dotted(), "unknown");
    }
}

#[cfg(test)]
mod snapshot_replay {
    use super::*;
    use std::path::Path;

    /// Offline replay: every committed snapshot transcript must parse line by
    /// line, carry seq/raw metadata, and classify without surfacing parse_error
    /// for valid runs. No network / real pi required.
    fn replay_dir(dir: &Path) -> (usize, usize, Vec<String>) {
        let path = dir.join("transcript.jsonl");
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let mut records = 0usize;
        let mut parse_errors = 0usize;
        let mut bad = Vec::new();
        for (i, line) in content.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let wrapper: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(e) => {
                    bad.push(format!("{}:{} invalid wrapper json: {e}", path.display(), i + 1));
                    continue;
                }
            };
            if wrapper.get("seq").is_none() || wrapper.get("raw").is_none() {
                bad.push(format!("{}:{} missing seq/raw", path.display(), i + 1));
            }
            // Classify the inbound raw payload.
            if wrapper.get("dir").and_then(|d| d.as_str()) == Some("in")
                && let Some(raw) = wrapper.get("raw")
            {
                records += 1;
                let label = classify(Some(raw), false);
                if label.dotted() == "parse_error" {
                    parse_errors += 1;
                }
            }
        }
        (records, parse_errors, bad)
    }

    #[test]
    fn committed_snapshots_replay_offline() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/probe");
        let scenarios = ["text_only", "tool_call", "failed_tool", "extension_ui"];
        let mut total = 0usize;
        for name in scenarios {
            let dir = root.join(name).join("snapshot");
            assert!(dir.exists(), "missing snapshot for {name} (run with --snapshot)");
            let (n, errs, bad) = replay_dir(&dir);
            assert!(bad.is_empty(), "snapshot {name}: {bad:?}");
            assert!(n > 0, "snapshot {name} had no inbound records");
            assert_eq!(errs, 0, "snapshot {name} contained parse_error classifications");
            total += n;
        }
        assert!(total >= 10, "expected many inbound records, got {total}");
    }
}
