//! Session-style RPC client over [`Transport`].
//!
//! Responsibilities beyond the raw transport:
//! - assign/forward a unique command `id`
//! - resolve a `response` to the caller waiting on that id
//! - fan every id-less top-level message out on the events receiver
//!
//! A single background pump thread owns the transport and multiplexes queued
//! commands, the stdout event stream, and per-command deadlines.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::rpc::jsonl::OutgoingCommand;
use crate::rpc::process::Transport;
use crate::rpc::{TransportEvent, WireMessage};

/// One inbound unsolicited message.
#[derive(Debug, Clone)]
pub enum ClientEvent {
    /// Any id-less (or unmatched) JSON message: session events,
    /// extension_ui_request, extension_error, ...
    Event(Value),
    /// A stdout line that was not valid JSON, preserved verbatim.
    ParseError { raw: String },
    /// Underlying stdout closed (pi exited).
    Closed,
}

/// Error returned by command calls.
#[derive(Debug)]
pub enum ClientError {
    /// No response before the timeout.
    Timeout,
    /// The response carried `success: false`.
    Response(Value),
    /// Writing to the child failed / channel or stdout torn down.
    Transport(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Timeout => write!(f, "timeout waiting for response"),
            ClientError::Response(v) => write!(
                f,
                "command error: {}",
                v.get("error").and_then(|e| e.as_str()).unwrap_or("?")
            ),
            ClientError::Transport(s) => write!(f, "transport: {s}"),
        }
    }
}

impl std::error::Error for ClientError {}

/// Whether a command call requires a successful response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponsePolicy {
    /// Return the response `data` only when `success == true`.
    RequireSuccess,
    /// Return the whole response object regardless of `success`.
    AcceptError,
}

enum PumpMessage {
    /// Send a command object and resolve on its matched response.
    Command(Waiter),
    /// Send an already-built JSON line and do not expect id routing.
    Raw(String),
    /// Kill the child process and stop the pump (session shutdown).
    Shutdown,
}

struct Waiter {
    value: Value,
    deadline: Instant,
    policy: ResponsePolicy,
    reply: Sender<Result<Value, ClientError>>,
}

pub struct RpcClient {
    events: Option<Receiver<ClientEvent>>,
    to_pump: Sender<PumpMessage>,
}

/// Cheap, `Send` + `Clone` handle for issuing commands without borrowing the
/// whole client (safe to move into a detached thread).
#[derive(Clone)]
pub struct CommandSender {
    to_pump: Sender<PumpMessage>,
}

impl RpcClient {
    /// Spawn pi and return a client. `extra_args` is forwarded to the transport
    /// (e.g. `-e <extension>` pairs).
    pub fn spawn(binary: &str, extra_args: &[String]) -> std::io::Result<Self> {
        Self::spawn_in(binary, extra_args, None)
    }

    /// Spawn with an explicit working directory for the pi child (embed mode:
    /// the session's workspace path, spec: Sidebar session creation).
    pub fn spawn_in(
        binary: &str,
        extra_args: &[String],
        cwd: Option<&std::path::Path>,
    ) -> std::io::Result<Self> {
        let transport = Transport::spawn_rpc_in(binary, extra_args, cwd)?;
        Ok(Self::from_transport(transport))
    }

    /// Kill the pi child process and stop the pump thread (session close,
    /// spec: Manual session close). Subsequent commands fail and the events
    /// channel closes.
    pub fn shutdown(&self) {
        let _ = self.to_pump.send(PumpMessage::Shutdown);
    }

    /// Wrap an existing transport (used by tests with fake pi processes).
    pub fn from_transport(transport: Transport) -> Self {
        let (event_tx, event_rx) = mpsc::channel::<ClientEvent>();
        let (to_pump, from_caller) = mpsc::channel::<PumpMessage>();
        thread::spawn(move || pump(transport, event_tx, from_caller));
        RpcClient {
            events: Some(event_rx),
            to_pump,
        }
    }

    /// A clonable, `Send` command handle usable from other threads.
    pub fn command_sender(&self) -> CommandSender {
        CommandSender {
            to_pump: self.to_pump.clone(),
        }
    }

    /// Receiver for all unsolicited events.
    pub fn events(&self) -> &Receiver<ClientEvent> {
        self.events.as_ref().expect("events receiver taken")
    }

    /// Take ownership of the event receiver (e.g. move into a reader thread).
    /// Panics if called more than once.
    pub fn take_events(&mut self) -> Receiver<ClientEvent> {
        self.events.take().expect("events receiver already taken")
    }

    /// Send a command object (no `id` needed) and await its response.
    pub fn send_command(
        &self,
        value: Value,
        timeout: Duration,
        policy: ResponsePolicy,
    ) -> Result<Value, ClientError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        let waiter = Waiter {
            value,
            deadline: Instant::now() + timeout,
            policy,
            reply: reply_tx,
        };
        self.to_pump
            .send(PumpMessage::Command(waiter))
            .map_err(|_| ClientError::Transport("client pump gone".into()))?;
        reply_rx
            .recv()
            .unwrap_or(Err(ClientError::Transport("no reply".into())))
    }

    /// Send an already-built JSON object line without id routing (e.g. an
    /// `extension_ui_response` carrying its own request id).
    pub fn send_raw(&self, value: &Value) -> Result<(), ClientError> {
        let line = serde_json::to_string(value)
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        self.to_pump
            .send(PumpMessage::Raw(line))
            .map_err(|_| ClientError::Transport("client pump gone".into()))
    }
}

impl CommandSender {
    /// 发送一条不经 id 路由的原始 JSON 行（如 extension_ui_response）。
    ///
    /// 输入: value — 单个 JSON 对象（须自带 id 字段）
    /// 输出: Ok(()) 表示已入队；泵已退出则 Err
    pub fn send_raw(&self, value: &Value) -> Result<(), ClientError> {
        let line = serde_json::to_string(value)
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        self.to_pump
            .send(PumpMessage::Raw(line))
            .map_err(|_| ClientError::Transport("client pump gone".into()))
    }

    /// Issue a command and await its response. Mirrors [`RpcClient::send_command`].
    pub fn send_command(
        &self,
        value: Value,
        timeout: Duration,
        policy: ResponsePolicy,
    ) -> Result<Value, ClientError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        let waiter = Waiter {
            value,
            deadline: Instant::now() + timeout,
            policy,
            reply: reply_tx,
        };
        self.to_pump
            .send(PumpMessage::Command(waiter))
            .map_err(|_| ClientError::Transport("client pump gone".into()))?;
        reply_rx
            .recv()
            .unwrap_or(Err(ClientError::Transport("no reply".into())))
    }
}

fn pump(
    mut transport: Transport,
    event_tx: Sender<ClientEvent>,
    from_caller: Receiver<PumpMessage>,
) {
    // Outstanding commands keyed by id.
    let mut waiters: HashMap<String, Waiter> = HashMap::new();

    loop {
        // 1. Send any queued commands/raw lines (non-blocking drain).
        while let Ok(msg) = from_caller.try_recv() {
            match msg {
                PumpMessage::Raw(line) => {
                    if transport.send_raw(&line).is_err() {
                        // transport writer gone; nothing to target, loop will
                        // observe StdoutClosed soon.
                    }
                }
                PumpMessage::Shutdown => {
                    transport.shutdown();
                    return;
                }
                PumpMessage::Command(w) => {
                    let id = w
                        .value
                        .get("id")
                        .and_then(|i| i.as_str())
                        .map(str::to_string)
                        .unwrap_or_else(next_id);
                    let outgoing = OutgoingCommand::new(id.clone(), w.value.clone());
                    match transport.send(&outgoing) {
                        Ok(()) => {
                            waiters.insert(id, w);
                        }
                        Err(_) => {
                            let _ = w.reply.send(Err(ClientError::Transport(
                                "send failed".into(),
                            )));
                        }
                    }
                }
            }
        }

        // 2. Expire waiters whose deadline passed.
        let now = Instant::now();
        waiters.retain(|_, w| {
            if w.deadline <= now {
                let _ = w.reply.send(Err(ClientError::Timeout));
                false
            } else {
                true
            }
        });

        // 3. Read the next stdout event (short bounded wait).
        match transport.events.recv_timeout(Duration::from_millis(20)) {
            Ok(TransportEvent::Message(WireMessage::Json(v))) => {
                let is_response =
                    v.get("type").and_then(|t| t.as_str()) == Some("response");
                let id = v
                    .get("id")
                    .and_then(|i| i.as_str())
                    .map(str::to_string);
                if is_response {
                    if let Some(id) = id
                        && let Some(w) = waiters.remove(&id) {
                            let success = v
                                .get("success")
                                .and_then(|s| s.as_bool())
                                .unwrap_or(true);
                            let result = match (success, w.policy) {
                                (true, _) => {
                                    Ok(v.get("data").cloned().unwrap_or(Value::Null))
                                }
                                (false, ResponsePolicy::AcceptError) => Ok(v.clone()),
                                (false, ResponsePolicy::RequireSuccess) => {
                                    Err(ClientError::Response(v.clone()))
                                }
                            };
                            let _ = w.reply.send(result);
                            continue;
                        }
                    // Unmatched response: surface as an event rather than drop.
                    let _ = event_tx.send(ClientEvent::Event(v));
                } else {
                    let _ = event_tx.send(ClientEvent::Event(v));
                }
            }
            Ok(TransportEvent::Message(WireMessage::ParseError { raw })) => {
                let _ = event_tx.send(ClientEvent::ParseError { raw });
            }
            Ok(TransportEvent::StdoutClosed) => {
                for (_, w) in waiters.drain() {
                    let _ = w
                        .reply
                        .send(Err(ClientError::Transport("stdout closed".into())));
                }
                let _ = event_tx.send(ClientEvent::Closed);
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                for (_, w) in waiters.drain() {
                    let _ = w
                        .reply
                        .send(Err(ClientError::Transport("stdout closed".into())));
                }
                break;
            }
        }
    }
}

use std::sync::atomic::{AtomicU64, Ordering};
static SEQ: AtomicU64 = AtomicU64::new(1);
fn next_id() -> String {
    format!("client-{}", SEQ.fetch_add(1, Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(script: &str, name: &str) -> String {
        let dir =
            std::env::temp_dir().join(format!("rpc-client-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fake-pi.sh");
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn routes_responses_by_id_and_ignores_events() {
        // Reply to each command with its own id; emit an id-less event first.
        let script = r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
  cmd=$(printf '%s' "$line" | sed -n 's/.*"type":"\([^"]*\)".*/\1/p')
  if [ "$cmd" = "first" ]; then
    printf '{"type":"agent_start"}\n'
    printf '{"type":"response","id":"someone-else","success":true}\n'
    printf '{"type":"response","id":"%s","success":true,"data":{"a":1}}\n' "$id"
  fi
  if [ "$cmd" = "second" ]; then
    printf '{"type":"response","id":"%s","success":true,"data":{"b":2}}\n' "$id"
  fi
done
"#;
        let path = fake(script, "route");
        let client =
            RpcClient::spawn(&path, &[]).expect("spawn");
        let r1 = client
            .send_command(
                serde_json::json!({"type": "first"}),
                Duration::from_secs(5),
                ResponsePolicy::RequireSuccess,
            )
            .unwrap();
        assert_eq!(r1, serde_json::json!({"a": 1}));
        let r2 = client
            .send_command(
                serde_json::json!({"type": "second"}),
                Duration::from_secs(5),
                ResponsePolicy::RequireSuccess,
            )
            .unwrap();
        assert_eq!(r2, serde_json::json!({"b": 2}));
        // The id-less agent_start and unmatched response must be on the event stream.
        let mut saw_agent = false;
        let mut saw_unmatched = false;
        while let Ok(ev) = client.events().recv_timeout(Duration::from_millis(300)) {
            if let ClientEvent::Event(v) = ev {
                match v.get("type").and_then(|t| t.as_str()) {
                    Some("agent_start") => saw_agent = true,
                    Some("response") => saw_unmatched = true,
                    _ => {}
                }
            }
        }
        assert!(saw_agent, "agent_start should be an event, not a response");
        assert!(saw_unmatched, "unmatched id response should be an event");
        let _ = std::fs::remove_dir_all(std::path::Path::new(&path).parent().unwrap());
    }

    #[test]
    fn command_timeout_does_not_hang() {
        // Responds to nothing.
        let script = "#!/bin/sh\nwhile IFS= read -r line; do sleep 1; done\n";
        let path = fake(script, "timeout");
        let client = RpcClient::spawn(&path, &[]).unwrap();
        let err = client
            .send_command(
                serde_json::json!({"type": "prompt"}),
                Duration::from_millis(300),
                ResponsePolicy::RequireSuccess,
            )
            .unwrap_err();
        assert!(matches!(err, ClientError::Timeout), "got {err}");
        let _ = std::fs::remove_dir_all(std::path::Path::new(&path).parent().unwrap());
    }

    #[test]
    fn require_surfaces_error_response() {
        let script = r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
  printf '{"type":"response","id":"%s","success":false,"error":"boom"}\n' "$id"
done
"#;
        let path = fake(script, "errresp");
        let client = RpcClient::spawn(&path, &[]).unwrap();
        let err = client
            .send_command(
                serde_json::json!({"type": "compact"}),
                Duration::from_secs(5),
                ResponsePolicy::RequireSuccess,
            )
            .unwrap_err();
        assert!(matches!(err, ClientError::Response(_)), "got {err}");
        // AcceptError returns the body instead.
        let body = client
            .send_command(
                serde_json::json!({"type": "compact"}),
                Duration::from_secs(5),
                ResponsePolicy::AcceptError,
            )
            .unwrap();
        assert_eq!(body.get("success").and_then(|s| s.as_bool()), Some(false));
        let _ = std::fs::remove_dir_all(std::path::Path::new(&path).parent().unwrap());
    }
}
