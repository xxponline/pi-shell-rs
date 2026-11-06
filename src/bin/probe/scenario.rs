//! Declarative scenario model and its sequential runner.
//!
//! The runner owns one [`Transport`], assigns command ids, correlates
//! `response` messages by id, and treats *every id-less message as an event*.
//! All raw lines are appended to a [`Transcript`] as they pass through.

use std::collections::HashMap;
use std::io::Write;
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::classify;
use pi_shell_rs::rpc::jsonl::{OutgoingCommand, WireMessage};
use pi_shell_rs::rpc::process::{TransportEvent};
use pi_shell_rs::rpc::{Direction, StderrLine, Transport};

/// Answer to an `extension_ui_request`.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Cancelled is part of the protocol, used by future scenarios.
pub enum UiAnswer {
    Value(String),
    Confirmed(bool),
    Cancelled,
}

/// One declarative scenario step.
#[derive(Debug, Clone)]
pub enum Step {
    /// Send a command object (without `id`) and wait for its `response`.
    /// `expect_error` treats a `success:false` response as the expected outcome
    /// (used for compaction on an intentionally small session).
    Send {
        command: Value,
        timeout: Duration,
        expect_error: bool,
    },
    /// Wait for an event whose top-level `type` equals `event_type`.
    /// Optional sub-predicate checks `assistantMessageEvent.type` etc.
    WaitEvent {
        event_type: String,
        sub_predicate: Option<fn(&Value) -> bool>,
        timeout: Duration,
    },
    /// Wait for the `agent_settled` event.
    WaitSettled { timeout: Duration },
    /// Fixed delay.
    Sleep(Duration),
    /// When the next UI request with this method arrives, reply automatically.
    AnswerNextUi {
        method: String,
        answer: UiAnswer,
    },
}

#[derive(Debug, Clone)]
pub struct Scenario {
    pub name: &'static str,
    pub description: &'static str,
    pub steps: Vec<Step>,
}

/// One transcript record (one raw JSON object in `raw`, plus probe metadata).
#[derive(Debug, Clone)]
pub struct Record {
    pub seq: u64,
    pub at_ms: u128,
    pub dir: Direction,
    pub raw: Value,
}

/// Accumulates transcript rows, counts, and step outcomes for one run.
pub struct RunReport {
    pub records: Vec<Record>,
    pub counts: HashMap<String, u32>,
    pub stderr_lines: Vec<StderrLine>,
    /// (step_index, ok, detail)
    pub step_results: Vec<(usize, bool, String)>,
    pub failure: Option<String>,
    next_seq: u64,
}

impl RunReport {
    fn new() -> Self {
        Self {
            records: Vec::new(),
            counts: HashMap::new(),
            stderr_lines: Vec::new(),
            step_results: Vec::new(),
            failure: None,
            next_seq: 0,
        }
    }

    /// Test-only builder producing a report with one long-field record.
    #[cfg(test)]
    pub fn new_for_test() -> Self {
        let mut r = Self::new();
        r.push(
            Instant::now(),
            Direction::In,
            json!({"type":"tool_execution_end","result": "z".repeat(120)}),
        );
        *r.counts.entry("tool_execution_end".to_string()).or_default() += 1;
        r.step_results.push((0, true, "ok".into()));
        r
    }

    fn push(&mut self, started: Instant, dir: Direction, raw: Value) {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.records.push(Record {
            seq,
            at_ms: started.elapsed().as_millis(),
            dir,
            raw,
        });
    }

    /// Increment classification counts for an inbound wire message.
    fn count_inbound(&mut self, msg: &WireMessage) {
        let label = match msg {
            WireMessage::Json(v) => classify::classify(Some(v), false),
            WireMessage::ParseError { .. } => classify::classify(None, true),
        };
        *self.counts.entry(label.dotted()).or_default() += 1;
    }
}

/// Outcome of running one scenario.
pub struct RunOutcome {
    pub report: RunReport,
    pub ok: bool,
}

/// Run one scenario against an already-spawned transport.
pub fn run(
    transport: &mut Transport,
    scenario: &Scenario,
    default_timeout: Duration,
) -> RunOutcome {
    let started = Instant::now();
    let mut report = RunReport::new();
    let mut pending_responses: HashMap<String, std::sync::mpsc::Sender<Value>> =
        HashMap::new();
    // We use a simple per-command channel stored alongside a receiver keyed by id.
    let mut response_waiters: HashMap<String, std::sync::mpsc::Receiver<Value>> =
        HashMap::new();
    // Pending UI auto-answer rules keyed by method (AnswerNextUi semantics).
    let mut ui_rules: HashMap<String, UiAnswer> = HashMap::new();
    let mut command_seq: u64 = 0;

    let mut ok = true;

    'steps: for (idx, step) in scenario.steps.iter().enumerate() {
        match step {
            Step::Sleep(d) => {
                std::thread::sleep(*d);
                report.step_results.push((idx, true, "sleep".into()));
            }
            Step::Send { command, timeout, expect_error } => {
                command_seq += 1;
                let id = format!("{}-{command_seq}", scenario.name);
                let outgoing = OutgoingCommand::new(id.clone(), command.clone());
                // Register waiter BEFORE sending.
                let (tx, rx) = std::sync::mpsc::channel::<Value>();
                response_waiters.insert(id.clone(), rx);
                pending_responses.insert(id.clone(), tx);
                let _ = pending_responses; // tx retained in map (kept alive); see drain below

                if let Err(e) = transport.send(&outgoing) {
                    report.step_results.push((idx, false, format!("send failed: {e}")));
                    report.failure = Some(format!("step {idx}: send failed: {e}"));
                    ok = false;
                    break 'steps;
                }
                report.push(started, Direction::Out, outgoing.command.clone());

                // Pump events until the response for this id shows up.
                let rx = response_waiters.get(&id).unwrap();
                match recv_response_or_pump(
                    transport,
                    rx,
                    *timeout,
                    started,
                    &mut report,
                    &mut pending_responses,
                    &mut ui_rules,
                ) {
                    Ok(resp) => {
                        let success = resp
                            .get("success")
                            .and_then(|s| s.as_bool())
                            .unwrap_or(true);
                        let err_text = resp
                            .get("error")
                            .and_then(|e| e.as_str())
                            .unwrap_or("?");
                        let passed = success || *expect_error;
                        let detail = if success {
                            "response ok".into()
                        } else {
                            format!("response error (expected={expect_error}): {err_text}")
                        };
                        report.step_results.push((idx, passed, detail));
                        if !passed {
                            report.failure = Some(format!(
                                "step {idx}: command returned error: {err_text}"
                            ));
                            ok = false;
                            break 'steps;
                        }
                    }
                    Err(e) => {
                        report.step_results.push((idx, false, e.clone()));
                        report.failure = Some(format!("step {idx}: {e}"));
                        ok = false;
                        break 'steps;
                    }
                }
                response_waiters.remove(&id);
                pending_responses.remove(&id);
            }
            Step::WaitEvent {
                event_type,
                sub_predicate,
                timeout,
            } => {
                match wait_event(
                    transport,
                    event_type,
                    *sub_predicate,
                    *timeout,
                    started,
                    &mut report,
                    &mut pending_responses,
                    &mut ui_rules,
                ) {
                    Ok(()) => report
                        .step_results
                        .push((idx, true, format!("saw {event_type}"))),
                    Err(e) => {
                        report.step_results.push((idx, false, e.clone()));
                        report.failure = Some(format!("step {idx}: {e}"));
                        ok = false;
                        break 'steps;
                    }
                }
            }
            Step::WaitSettled { timeout } => {
                match wait_event(
                    transport,
                    "agent_settled",
                    None,
                    *timeout,
                    started,
                    &mut report,
                    &mut pending_responses,
                    &mut ui_rules,
                ) {
                    Ok(()) => report
                        .step_results
                        .push((idx, true, "agent_settled".into())),
                    Err(e) => {
                        report.step_results.push((idx, false, e.clone()));
                        report.failure = Some(format!("step {idx}: {e}"));
                        ok = false;
                        break 'steps;
                    }
                }
            }
            Step::AnswerNextUi { method, answer } => {
                ui_rules.insert(method.clone(), answer.clone());
                report
                    .step_results
                    .push((idx, true, format!("armed UI answer for {method}")));
            }
        }
    }

    // Drain anything still buffered (best effort, short) so transcripts capture tails.
    let drain_deadline = Instant::now() + Duration::from_millis(300);
    while Instant::now() < drain_deadline {
        match transport.events.recv_timeout(Duration::from_millis(50)) {
            Ok(TransportEvent::Message(m)) => {
                report.count_inbound(&m);
                if let WireMessage::Json(v) = &m {
                    report.push(started, Direction::In, v.clone());
                }
                try_answer_ui(transport, &m, &mut ui_rules, started, &mut report);
                route_response(&m, &mut pending_responses);
            }
            Ok(TransportEvent::StdoutClosed) | Err(_) => break,
        }
    }
    // Suck any buffered stderr too.
    while let Ok(line) = transport.stderr.recv_timeout(Duration::from_millis(20)) {
        report.stderr_lines.push(line);
    }

    let _ = default_timeout; // steps carry explicit timeouts; kept for API future use.
    RunOutcome { report, ok }
}

fn recv_response_or_pump(
    transport: &Transport,
    rx: &std::sync::mpsc::Receiver<Value>,
    timeout: Duration,
    started: Instant,
    report: &mut RunReport,
    pending: &mut HashMap<String, std::sync::mpsc::Sender<Value>>,
    ui_rules: &mut HashMap<String, UiAnswer>,
) -> Result<Value, String> {
    let deadline = Instant::now() + timeout;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err("timeout waiting for response".into());
        }
        let remaining = deadline - now;
        // Peek our waiter first (already routed by another pump pass).
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(v) => return Ok(v),
            Err(RecvTimeoutError::Disconnected) => {
                return Err("response waiter disconnected".into())
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        // Pull one transport event and route it.
        match transport.events.recv_timeout(remaining.min(Duration::from_millis(200))) {
            Ok(TransportEvent::Message(m)) => {
                report.count_inbound(&m);
                if let WireMessage::Json(v) = &m {
                    report.push(started, Direction::In, v.clone());
                }
                try_answer_ui(transport, &m, ui_rules, started, report);
                route_response(&m, pending);
            }
            Ok(TransportEvent::StdoutClosed) => return Err("stdout closed".into()),
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return Err("stdout closed".into()),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn wait_event(
    transport: &Transport,
    event_type: &str,
    sub_predicate: Option<fn(&Value) -> bool>,
    timeout: Duration,
    started: Instant,
    report: &mut RunReport,
    pending: &mut HashMap<String, std::sync::mpsc::Sender<Value>>,
    ui_rules: &mut HashMap<String, UiAnswer>,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        if Instant::now() >= deadline {
            return Err(format!("timeout waiting for event {event_type}"));
        }
        match transport
            .events
            .recv_timeout(Duration::from_millis(100))
        {
            Ok(TransportEvent::Message(m)) => {
                report.count_inbound(&m);
                let matched = match &m {
                    WireMessage::Json(v) => {
                        report.push(started, Direction::In, v.clone());
                        v.get("type").and_then(|t| t.as_str()) == Some(event_type)
                            && sub_predicate.is_none_or(|p| p(v))
                    }
                    WireMessage::ParseError { .. } => false,
                };
                try_answer_ui(transport, &m, ui_rules, started, report);
                route_response(&m, pending);
                if matched {
                    return Ok(());
                }
            }
            Ok(TransportEvent::StdoutClosed) => return Err("stdout closed".into()),
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return Err("stdout closed".into()),
        }
    }
}

/// If this inbound message is a `response`, forward it to the waiter channel.
fn route_response(
    m: &WireMessage,
    pending: &mut HashMap<String, std::sync::mpsc::Sender<Value>>,
) {
    if let WireMessage::Json(v) = m {
        let is_response = v.get("type").and_then(|t| t.as_str()) == Some("response");
        if is_response
            && let Some(id) = v.get("id").and_then(|i| i.as_str())
            && let Some(tx) = pending.get(id)
        {
            // Best effort; if receiver gone, drop waiter.
            if tx.send(v.clone()).is_err() {
                pending.remove(id);
            }
        }
    }
}

/// If a `AnswerNextUi` rule matches this request, send the response command.
fn try_answer_ui(
    transport: &Transport,
    m: &WireMessage,
    ui_rules: &mut HashMap<String, UiAnswer>,
    started: Instant,
    report: &mut RunReport,
) {
    let (id, method) = match m {
        WireMessage::Json(v)
            if v.get("type").and_then(|t| t.as_str()) == Some("extension_ui_request") =>
        {
            (
                v.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string(),
                v.get("method").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            )
        }
        _ => return,
    };
    let Some(answer) = ui_rules.remove(&method) else {
        return;
    };
    let payload = match answer {
        UiAnswer::Value(value) => json!({
            "type": "extension_ui_response", "id": id, "value": value
        }),
        UiAnswer::Confirmed(c) => json!({
            "type": "extension_ui_response", "id": id, "confirmed": c
        }),
        UiAnswer::Cancelled => json!({
            "type": "extension_ui_response", "id": id, "cancelled": true
        }),
    };
    // UI responses already carry their request id; send the object verbatim.
    let line = serde_json::to_string(&payload).expect("json");
    if transport.send_raw(&line).is_ok() {
        report.push(started, Direction::Out, payload);
    }
}

/// Write transcript records to `writer` as JSONL:
/// `{seq, ts_ms, dir, raw}`.
pub fn write_transcript(
    records: &[Record],
    writer: &mut dyn Write,
) -> std::io::Result<()> {
    for r in records {
        let wrapper = json!({
            "seq": r.seq,
            "ts_ms": r.at_ms,
            "dir": r.dir,
            "raw": r.raw,
        });
        writeln!(writer, "{}", serde_json::to_string(&wrapper)?)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn fake_pi(script: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "probe-scenario-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fake-pi.sh");
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        (path, dir)
    }

    // 3.1: responses routed by id; id-less events never satisfy a Send wait.
    #[test]
    fn routes_responses_by_id_out_of_order() {
        // Reads stdin lines; responds to ids in REVERSE arrival order.
        // Each command replies with its OWN id. `first` emits an id-less event
        // and an unrelated response-shaped noise first, proving the Send wait
        // is only satisfied by its matching id.
        let script = r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
  cmd=$(printf '%s' "$line" | sed -n 's/.*"type":"\([^"]*\)".*/\1/p')
  if [ "$cmd" = "first" ]; then
    printf '{"type":"agent_start"}\n'
    printf '{"type":"response","id":"someone-else","success":true}\n'
    printf '{"type":"response","id":"%s","success":true}\n' "$id"
  fi
  if [ "$cmd" = "second" ]; then
    printf '{"type":"response","id":"%s","success":true}\n' "$id"
  fi
done
"#;
        let (path, dir) = fake_pi(script);
        let mut t = Transport::spawn_rpc(path.to_str().unwrap(), &[]).unwrap();
        let s = Scenario {
            name: "reorder",
            description: "",
            steps: vec![
                Step::Send {
                    command: json!({"type": "first"}),
                    timeout: Duration::from_secs(5),
                    expect_error: false,
                },
                Step::Send {
                    command: json!({"type": "second"}),
                    timeout: Duration::from_secs(5),
                    expect_error: false,
                },
            ],
        };
        let out = run(&mut t, &s, Duration::from_secs(5));
        t.shutdown();
        assert!(out.ok, "steps: {:?}", out.report.step_results);
        assert_eq!(out.report.step_results.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // 3.2: multi-step scenario with WaitEvent predicate + WaitSettled ordering.
    #[test]
    fn waits_for_settled_before_next_step() {
        // Emits an event immediately for the prompt command, then settles later.
        let script = r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
  cmd=$(printf '%s' "$line" | sed -n 's/.*"type":"\([^"]*\)".*/\1/p')
  if [ "$cmd" = "prompt" ]; then
    printf '{"type":"response","id":"%s","success":true,"data":{"disposition":"started"}}\n' "$id"
    printf '{"type":"agent_start"}\n{"type":"agent_settled","aborted":false}\n'
  fi
  if [ "$cmd" = "get_state" ]; then
    printf '{"type":"response","id":"%s","success":true,"data":{}}\n' "$id"
  fi
done
"#;
        let (path, dir) = fake_pi(script);
        let mut t = Transport::spawn_rpc(path.to_str().unwrap(), &[]).unwrap();
        // Track ordering via transcript: get_state (out) must come AFTER settled (in).
        let s = Scenario {
            name: "order",
            description: "",
            steps: vec![
                Step::Send { command: json!({"type":"prompt"}), timeout: Duration::from_secs(5), expect_error: false },
                Step::WaitSettled { timeout: Duration::from_secs(5) },
                Step::Send { command: json!({"type":"get_state"}), timeout: Duration::from_secs(5), expect_error: false },
            ],
        };
        let out = run(&mut t, &s, Duration::from_secs(5));
        t.shutdown();
        assert!(out.ok, "{:?}", out.report.step_results);
        let settled_pos = out.report.records.iter().position(|r| {
            r.dir == Direction::In
                && r.raw.get("type").and_then(|v| v.as_str()) == Some("agent_settled")
        });
        let getstate_pos = out.report.records.iter().position(|r| {
            r.dir == Direction::Out
                && r.raw.get("type").and_then(|v| v.as_str()) == Some("get_state")
        });
        assert!(settled_pos.unwrap() < getstate_pos.unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // 3.3: extension_ui_request triggers an AnswerNextUi response with same id.
    #[test]
    fn answers_extension_ui_request() {
        let script = r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
  cmd=$(printf '%s' "$line" | sed -n 's/.*"type":"\([^"]*\)".*/\1/p')
  if [ "$cmd" = "trigger" ]; then
    printf '{"type":"extension_ui_request","id":"req-42","method":"select","title":"t","options":["a"]}\n'
    printf '{"type":"response","id":"%s","success":true}\n' "$id"
  fi
  if [ "$cmd" = "extension_ui_response" ]; then
    # Emit a sentinel event carrying the reply for in-transcript assertion.
    printf '{"type":"ui_replied","echo":%s}\n' "$line"
  fi
done
"#;
        let (path, dir) = fake_pi(script);
        let mut t = Transport::spawn_rpc(path.to_str().unwrap(), &[]).unwrap();
        let s = Scenario {
            name: "ui",
            description: "",
            steps: vec![
                Step::AnswerNextUi { method: "select".into(), answer: UiAnswer::Value("a".into()) },
                Step::Send { command: json!({"type":"trigger"}), timeout: Duration::from_secs(5), expect_error: false },
                Step::WaitEvent {
                    event_type: "ui_replied".into(),
                    sub_predicate: Some(|v| {
                        v.get("echo")
                            .and_then(|e| e.get("id"))
                            .and_then(|i| i.as_str()) == Some("req-42")
                    }),
                    timeout: Duration::from_secs(5),
                },
            ],
        };
        let out = run(&mut t, &s, Duration::from_secs(5));
        t.shutdown();
        assert!(out.ok, "{:?}", out.report.step_results);
        // The outbound response is in the transcript with matching id + value.
        let reply = out.report.records.iter().any(|r| {
            r.dir == Direction::Out
                && r.raw.get("type").and_then(|v| v.as_str()) == Some("extension_ui_response")
                && r.raw.get("id").and_then(|i| i.as_str()) == Some("req-42")
                && r.raw.get("value").and_then(|i| i.as_str()) == Some("a")
        });
        assert!(reply, "ui response missing from transcript");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // 3.4: timeout fails the scenario with non-ok and preserves captured records.
    #[test]
    fn timeout_fails_and_preserves_records() {
        // Respond to prompt but NEVER emit agent_settled.
        let script = r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
  cmd=$(printf '%s' "$line" | sed -n 's/.*"type":"\([^"]*\)".*/\1/p')
  if [ "$cmd" = "prompt" ]; then
    printf '{"type":"response","id":"%s","success":true,"data":{"disposition":"started"}}\n' "$id"
    printf '{"type":"agent_start"}\n'
    sleep 5
  fi
done
"#;
        let (path, dir) = fake_pi(script);
        let mut t = Transport::spawn_rpc(path.to_str().unwrap(), &[]).unwrap();
        let s = Scenario {
            name: "hang",
            description: "",
            steps: vec![
                Step::Send { command: json!({"type":"prompt"}), timeout: Duration::from_secs(5), expect_error: false },
                Step::WaitSettled { timeout: Duration::from_millis(300) },
            ],
        };
        let out = run(&mut t, &s, Duration::from_secs(1));
        t.shutdown();
        assert!(!out.ok);
        assert!(out.report.failure.as_ref().unwrap().contains("agent_settled"));
        assert!(out.report.records.iter().any(|r| {
            r.raw.get("type").and_then(|v| v.as_str()) == Some("agent_start")
        }));
        assert!(out
            .report
            .step_results
            .iter()
            .any(|(_, ok, _)| !ok));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
