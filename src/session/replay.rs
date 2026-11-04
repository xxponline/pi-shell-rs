//! Replay probe transcript JSONL into a [`SessionState`].
//!
//! Accepted file format is the probe artifact: one JSON object per line shaped
//! `{seq, ts_ms, dir, raw}`. Only inbound (`dir == "in"`) raw events are fed to
//! the state machine, so replaying never needs a pi process or network.

use std::io::{BufRead, BufReader, Read};

use serde_json::Value;

use crate::session::transcript::SessionState;

/// Reconstruct a session state by reading a probe transcript stream.
pub fn replay_jsonl<R: Read>(reader: R) -> std::io::Result<SessionState> {
    let mut state = SessionState::new();
    let reader = BufReader::new(reader);
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(wrapper) = serde_json::from_str::<Value>(&line) else {
            // A wrapper that is not JSON is not a valid probe record; skip.
            continue;
        };
        let is_inbound = wrapper
            .get("dir")
            .and_then(|d| d.as_str())
            .is_some_and(|d| d.eq_ignore_ascii_case("in"));
        if !is_inbound {
            continue;
        }
        if let Some(raw) = wrapper.get("raw") {
            state.handle(raw);
        }
    }
    Ok(state)
}

/// Replay from a file path.
pub fn replay_file(path: &std::path::Path) -> std::io::Result<SessionState> {
    let file = std::fs::File::open(path)?;
    replay_jsonl(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_inline_jsonl_builds_state() {
        let input = r#"
{"seq":0,"dir":"out","raw":{"type":"get_state"}}
{"seq":1,"dir":"in","raw":{"type":"agent_start"}}
{"seq":2,"dir":"in","raw":{"type":"turn_start"}}
{"seq":3,"dir":"in","raw":{"type":"message_start","message":{"role":"user","content":[{"type":"text","text":"hi"}]}}}
{"seq":4,"dir":"in","raw":{"type":"message_end","message":{"role":"user","content":[{"type":"text","text":"hi"}]}}}
{"seq":5,"dir":"in","raw":{"type":"message_start","message":{"role":"assistant","content":[]}}}
{"seq":6,"dir":"in","raw":{"type":"message_update","usage":{"input":1,"output":2,"cacheRead":0,"cacheWrite":0,"totalTokens":3},"assistantMessageEvent":{"type":"text_start","contentIndex":0}}}
{"seq":7,"dir":"in","raw":{"type":"message_update","usage":{"input":1,"output":2,"cacheRead":0,"cacheWrite":0,"totalTokens":3},"assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"po"}}}
{"seq":8,"dir":"in","raw":{"type":"message_update","usage":{"input":1,"output":2,"cacheRead":0,"cacheWrite":0,"totalTokens":3},"assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"ng"}}}
{"seq":9,"dir":"in","raw":{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"pong"}]}}}
{"seq":10,"dir":"in","raw":{"type":"agent_settled","aborted":false}}
"#;
        let state = replay_jsonl(input.as_bytes()).unwrap();
        use crate::session::{Block, Entry, Phase};
        assert_eq!(state.phase, Phase::Idle);
        let user = state.entries().iter().any(|e| matches!(e, Entry::User { text } if text == "hi"));
        assert!(user, "user entry missing");
        let assistant = state
            .entries()
            .iter()
            .find_map(|e| match e {
                Entry::Assistant { blocks, .. } => Some(blocks),
                _ => None,
            })
            .expect("assistant entry");
        assert!(
            assistant.iter().any(|b| matches!(b, Block::Text { text } if text == "pong")),
            "got {assistant:?}"
        );
        assert_eq!(state.usage.as_ref().map(|u| u.total_tokens), Some(3));
    }
}
