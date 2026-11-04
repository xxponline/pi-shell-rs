//! JSONL wire types shared by the transport, RPC client, and consumers.
//!
//! Weak on purpose: pi's payloads evolve, so only the discriminator fields
//! needed for routing/classification are extracted; raw JSON is preserved.

use serde_json::Value;

/// One decoded stdout line from `pi --mode rpc`.
#[derive(Debug, Clone)]
pub enum WireMessage {
    /// A line that parsed as JSON.
    Json(Value),
    /// A line that was not valid JSON. Kept verbatim so a protocol regression
    /// (stray log line, ANSI fragment) becomes evidence instead of a crash.
    ParseError { raw: String },
}

impl WireMessage {
    pub fn parse_line(line: &str) -> Self {
        match serde_json::from_str::<Value>(line) {
            Ok(v) => WireMessage::Json(v),
            Err(_) => WireMessage::ParseError {
                raw: line.to_string(),
            },
        }
    }

    /// Top-level `type` discriminator for JSON messages; `"parse_error"` for a
    /// non-JSON line; `None` for a JSON object without a `type` field.
    pub fn top_type(&self) -> Option<&str> {
        match self {
            WireMessage::Json(v) => v.get("type").and_then(|t| t.as_str()),
            WireMessage::ParseError { .. } => Some("parse_error"),
        }
    }
}

/// One command envelope sent to the transport. The transport inserts `id`.
#[derive(Debug, Clone)]
pub struct OutgoingCommand {
    /// Command object with `id` inserted.
    pub command: Value,
}

impl OutgoingCommand {
    pub fn new(id: impl Into<String>, mut command: Value) -> Self {
        if let Some(obj) = command.as_object_mut() {
            obj.insert("id".to_string(), Value::String(id.into()));
        }
        Self { command }
    }

    pub fn to_line(&self) -> String {
        // A command object is always an object; serialization cannot fail here.
        serde_json::to_string(&self.command).expect("command must serialize")
    }
}

/// Direction tag for transcript records.
#[derive(Debug, Clone, Copy, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    In,
    Out,
}
