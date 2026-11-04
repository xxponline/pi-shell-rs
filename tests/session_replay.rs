//! Offline replay tests over committed probe snapshots. No pi process/network.

use pi_rust_tui::session::{replay_file, Block, Entry, Phase};
use std::path::Path;

fn fixture(scenario: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/probe")
        .join(scenario)
        .join("snapshot/transcript.jsonl")
}

fn assistants(state: &pi_rust_tui::session::SessionState) -> Vec<&Vec<Block>> {
    state
        .entries()
        .iter()
        .filter_map(|e| match e {
            Entry::Assistant { blocks, .. } => Some(blocks),
            _ => None,
        })
        .collect()
}

#[test]
fn text_only_rebuilds_user_and_assistant() {
    let state = replay_file(&fixture("text_only")).expect("replay");
    assert_eq!(state.phase, Phase::Idle);

    let users: Vec<&String> = state
        .entries()
        .iter()
        .filter_map(|e| match e {
            Entry::User { text } => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(users.len(), 1, "one user message: {users:?}");
    assert!(users[0].contains("pong"));

    let asst = assistants(&state);
    assert!(!asst.is_empty(), "at least one assistant entry");
    // The final assistant must carry non-empty text.
    let last = asst.last().unwrap();
    let has_text = last
        .iter()
        .any(|b| matches!(b, Block::Text { text } if !text.is_empty()));
    assert!(has_text, "final assistant text empty: {last:?}");
}

#[test]
fn tool_call_rebuilds_successful_read_tool() {
    let state = replay_file(&fixture("tool_call")).expect("replay");

    let mut found_tool = false;
    for blocks in assistants(&state) {
        for b in blocks {
            if let Block::ToolCall {
                name, tool, ..
            } = b
                && name == "read" {
                    found_tool = true;
                    assert!(!tool.is_error);
                    assert!(tool.result.is_some(), "read tool should carry a result");
                }
        }
    }
    assert!(found_tool, "no read ToolCall block reconstructed");

    // User precedes assistant; transcript is non-empty and ordered.
    assert!(state.entries().len() >= 2);
    assert!(matches!(state.entries().first(), Some(Entry::User { .. })));
}

#[test]
fn failed_tool_marks_error_and_keeps_following_text() {
    let state = replay_file(&fixture("failed_tool")).expect("replay");

    let mut saw_error_tool = false;
    for blocks in assistants(&state) {
        for b in blocks {
            if let Block::ToolCall {
                name, tool, ..
            } = b
                && name == "read" && tool.is_error {
                    saw_error_tool = true;
                }
        }
    }
    assert!(saw_error_tool, "failed read tool not marked is_error");

    // After the failed tool the model still produces an explanatory text turn.
    let has_trailing_text = state
        .entries()
        .iter()
        .any(|e| matches!(e, Entry::Assistant { blocks, .. } if blocks.iter().any(|b| matches!(b, Block::Text { text } if !text.is_empty()))));
    assert!(has_trailing_text, "no assistant text after failed tool");
}
