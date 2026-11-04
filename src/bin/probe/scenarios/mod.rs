//! Built-in probe scenarios.
//!
//! Each scenario is a deterministic, low-cost script targeting real pi behavior.
//! Prompts stay small and operate on files inside this repo so runs are cheap
//! and roughly reproducible.

use std::time::Duration;

use serde_json::json;

use crate::scenario::{Scenario, Step, UiAnswer};

const T: u64 = 60_000; // default timeout for model-driven steps
const Q: Duration = Duration::from_millis(30_000); // quick local steps
const LONG: Duration = Duration::from_millis(150_000);

fn t(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

/// Build a normal Send step (not expecting an error response).
fn send(command: serde_json::Value, timeout: Duration) -> Step {
    Step::Send {
        command,
        timeout,
        expect_error: false,
    }
}

fn cmd_get_state() -> Step {
    send(json!({"type": "get_state"}), Q)
}

/// Scenario 1: plain text turn.
fn text_only() -> Scenario {
    Scenario {
        name: "text_only",
        description: "Single text-only prompt; captures streaming lifecycle + text deltas",
        steps: vec![
            cmd_get_state(),
            send(
                json!({"type": "prompt", "message": "Reply with exactly the word: pong"}),
                t(T),
            ),
            Step::WaitSettled { timeout: t(T) },
        ],
    }
}

/// Scenario 2: force a read tool call on a known repo file.
fn tool_call() -> Scenario {
    Scenario {
        name: "tool_call",
        description: "Prompt that triggers a read tool call against Cargo.toml",
        steps: vec![
            cmd_get_state(),
            send(
                json!({"type": "prompt", "message": "Use the read tool to read the file Cargo.toml in the current directory, then tell me its package name. Do nothing else."}),
                t(T),
            ),
            Step::WaitEvent {
                event_type: "tool_execution_end".into(),
                sub_predicate: None,
                timeout: t(T),
            },
            Step::WaitSettled { timeout: t(T) },
        ],
    }
}

/// Scenario 3: thinking deltas (set high, run, restore off).
fn thinking() -> Scenario {
    Scenario {
        name: "thinking",
        description: "High thinking level; captures thinking_* deltas",
        steps: vec![
            send(json!({"type": "set_thinking_level", "level": "high"}), Q),
            send(
                json!({"type": "prompt", "message": "In one sentence, what is 17 * 23?"}),
                t(T),
            ),
            Step::WaitSettled { timeout: t(T) },
            send(json!({"type": "set_thinking_level", "level": "off"}), Q),
        ],
    }
}

/// Scenario 4: steer mid-generation.
fn steer() -> Scenario {
    Scenario {
        name: "steer",
        description: "Start a long answer, then steer it while streaming",
        steps: vec![
            send(
                json!({"type": "prompt", "message": "Write a long, detailed essay about the history of computing, at least 500 words."}),
                t(T),
            ),
            Step::WaitEvent {
                event_type: "message_update".into(),
                sub_predicate: Some(|v| {
                    v.pointer("/assistantMessageEvent/type")
                        .and_then(|x| x.as_str())
                        == Some("text_delta")
                }),
                timeout: t(T),
            },
            Step::Sleep(Duration::from_millis(500)),
            send(
                json!({"type": "steer", "message": "Actually stop and just say OK."}),
                t(T),
            ),
            Step::WaitSettled { timeout: t(T) },
        ],
    }
}

/// Scenario 5: queue follow-ups during a run.
fn follow_up() -> Scenario {
    Scenario {
        name: "follow_up",
        description: "Queue two follow-ups while the first prompt runs",
        steps: vec![
            send(
                json!({"type": "prompt", "message": "Count slowly from 1 to 30 out loud."}),
                t(T),
            ),
            send(json!({"type": "follow_up", "message": "Now say alpha."}), Q),
            send(json!({"type": "follow_up", "message": "Now say beta."}), Q),
            Step::WaitEvent {
                event_type: "queue_update".into(),
                sub_predicate: None,
                timeout: t(T),
            },
            Step::WaitSettled { timeout: LONG },
        ],
    }
}

/// Scenario 6: manual compaction.
///
/// A probe-sized session is far below the 20k-token `keepRecentTokens`
/// threshold, so `compact` returns "Nothing to compact". It still emits
/// `compaction_start`/`compaction_end`, which are the signals we capture; the
/// expected business error is tolerated via `expect_error`.
fn compact() -> Scenario {
    Scenario {
        name: "compact",
        description: "Disable auto compaction, trigger manual compact (expect below-threshold error)",
        steps: vec![
            send(json!({"type": "set_auto_compaction", "enabled": false}), Q),
            send(
                json!({"type": "prompt", "message": "Remember the number 42. Reply OK."}),
                t(T),
            ),
            Step::WaitSettled { timeout: t(T) },
            Step::Send {
                command: json!({"type": "compact"}),
                timeout: t(120_000),
                expect_error: true,
            },
        ],
    }
}

/// Scenario 7: a tool call that must fail (read a missing file).
fn failed_tool() -> Scenario {
    Scenario {
        name: "failed_tool",
        description: "Prompt leading to a failing tool call (missing file)",
        steps: vec![
            send(
                json!({"type": "prompt", "message": "Use the read tool to read the file definitely-does-not-exist-xyz.txt and report what happens."}),
                t(T),
            ),
            Step::WaitEvent {
                event_type: "tool_execution_end".into(),
                sub_predicate: None,
                timeout: t(T),
            },
            Step::WaitSettled { timeout: t(T) },
        ],
    }
}

/// Scenario 8: extension UI request (requires a fixture extension via -e).
fn extension_ui() -> Scenario {
    Scenario {
        name: "extension_ui",
        description: "Triggers ctx.ui.select/confirm/notify via the fixture extension",
        steps: vec![
            Step::AnswerNextUi {
                method: "select".into(),
                answer: UiAnswer::Value("first".into()),
            },
            Step::AnswerNextUi {
                method: "confirm".into(),
                answer: UiAnswer::Confirmed(true),
            },
            send(
                json!({"type": "prompt", "message": "/probe-ui"}),
                t(T),
            ),
            // Extension commands may not start an agent run, so we wait for the
            // trailing notify UI request rather than agent_settled.
            Step::WaitEvent {
                event_type: "extension_ui_request".into(),
                sub_predicate: Some(|v| {
                    v.get("method").and_then(|m| m.as_str()) == Some("notify")
                }),
                timeout: t(T),
            },
        ],
    }
}

/// All built-in scenarios in run order.
pub fn all() -> Vec<Scenario> {
    vec![
        text_only(),
        tool_call(),
        thinking(),
        steer(),
        follow_up(),
        compact(),
        failed_tool(),
        extension_ui(),
    ]
}

pub fn by_name(name: &str) -> Option<Scenario> {
    all().into_iter().find(|s| s.name == name)
}
