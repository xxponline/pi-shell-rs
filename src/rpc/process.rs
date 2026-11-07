//! Child-process transport for `pi --mode rpc` (or `PI_BIN` override).
//!
//! Three OS threads:
//!   - writer thread:  sends JSONL lines to child stdin
//!   - stdout reader:  parses each line into [`WireMessage`]
//!   - stderr reader:  forwards raw timestamped lines on a separate channel
//!
//! Generic over the spawned program so tests can point it at a fake "pi"
//! shell script that replays fixture lines.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::rpc::jsonl::{OutgoingCommand, WireMessage};

/// Something observed on the child's stdout.
#[derive(Debug, Clone)]
pub enum TransportEvent {
    /// A decoded stdout line (JSON or an unparseable verbatim line).
    Message(WireMessage),
    /// The stdout reader thread ended (child closed stdout / exited).
    StdoutClosed,
}

/// A raw stderr line with a timestamp relative to transport start.
#[derive(Debug, Clone)]
pub struct StderrLine {
    pub at: Duration,
    pub text: String,
}

/// Handle used to send commands and receive events.
pub struct Transport {
    pub events: Receiver<TransportEvent>,
    pub stderr: Receiver<StderrLine>,
    write_tx: Sender<String>,
    child: Child,
}

/// Resolve the pi binary: `PI_BIN` env override, else `pi` on PATH.
pub fn resolve_binary() -> String {
    std::env::var("PI_BIN").unwrap_or_else(|_| "pi".to_string())
}

impl Transport {
    /// Spawn `<bin> --mode rpc` with piped stdio (child inherits this
    /// process's working directory).
    pub fn spawn_rpc(binary: &str, extra_args: &[String]) -> std::io::Result<Self> {
        Self::spawn_rpc_in(binary, extra_args, None)
    }

    /// Same as [`spawn_rpc`] with an explicit working directory for the
    /// child (embed mode: the session's workspace path).
    pub fn spawn_rpc_in(
        binary: &str,
        extra_args: &[String],
        cwd: Option<&std::path::Path>,
    ) -> std::io::Result<Self> {
        let mut cmd = Command::new(binary);
        cmd.arg("--mode")
            .arg("rpc")
            .args(extra_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = cwd {
            cmd.current_dir(cwd);
        }

        let mut child = cmd.spawn().map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!("failed to spawn `{binary} --mode rpc`: {e}"),
            )
        })?;

        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");

        let (event_tx, event_rx) = mpsc::channel::<TransportEvent>();
        let (err_tx, err_rx) = mpsc::channel::<StderrLine>();
        let (write_tx, write_rx) = mpsc::channel::<String>();

        let started = Instant::now();

        // Writer thread.
        thread::spawn(move || {
            let mut stdin = stdin;
            while let Ok(line) = write_rx.recv() {
                if stdin.write_all(line.as_bytes()).is_err()
                    || stdin.write_all(b"\n").is_err()
                    || stdin.flush().is_err()
                {
                    break;
                }
            }
        });

        // Stdout reader thread.
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(text) => {
                        let msg = WireMessage::parse_line(&text);
                        if event_tx.send(TransportEvent::Message(msg)).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = event_tx.send(TransportEvent::StdoutClosed);
        });

        // Stderr reader thread (separate channel; never mixed with stdout).
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                if err_tx
                    .send(StderrLine {
                        at: started.elapsed(),
                        text: line,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });

        Ok(Transport {
            events: event_rx,
            stderr: err_rx,
            write_tx,
            child,
        })
    }

    /// Send a command envelope (id inserted by [`OutgoingCommand`]).
    pub fn send(&self, cmd: &OutgoingCommand) -> std::io::Result<()> {
        self.send_raw(&cmd.to_line())
    }

    /// Send one pre-serialized JSON object line (no id injection).
    pub fn send_raw(&self, line: &str) -> std::io::Result<()> {
        self.write_tx
            .send(line.to_string())
            .map_err(|_| std::io::Error::other("writer thread gone"))
    }

    /// Kill the child and best-effort reap it.
    pub fn shutdown(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drain events until timeout/closed, collecting every stdout line.
    fn drain(transport: &Transport) -> Vec<WireMessage> {
        let mut out = Vec::new();
        while let Ok(TransportEvent::Message(m)) =
            transport.events.recv_timeout(Duration::from_secs(5))
        {
            out.push(m);
        }
        out
    }

    fn write_fake(script: &str, name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("probe-lib-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fake-pi.sh");
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    #[test]
    fn fake_pi_replays_lines_over_transport() {
        let script = "#!/bin/sh\nprintf '%s\n' '{\"type\":\"response\",\"id\":\"1\"}' 'not json at all'\n";
        let path = write_fake(script, "replay");
        let transport = Transport::spawn_rpc(path.to_str().unwrap(), &[]).unwrap();
        let msgs = drain(&transport);
        assert_eq!(msgs.len(), 2, "expected two lines, got {msgs:?}");
        match &msgs[0] {
            WireMessage::Json(v) => {
                assert_eq!(v.get("type").and_then(|t| t.as_str()), Some("response"));
                assert_eq!(v.get("id").and_then(|t| t.as_str()), Some("1"));
            }
            other => panic!("expected json, got {other:?}"),
        }
        match &msgs[1] {
            WireMessage::ParseError { raw } => assert_eq!(raw, "not json at all"),
            other => panic!("expected parse_error, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn missing_binary_errors_clearly() {
        let err = match Transport::spawn_rpc("/nonexistent/pi-binary-xyz", &[]) {
            Ok(_) => panic!("expected spawn to fail"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("failed to spawn"));
    }

    #[test]
    fn stderr_is_separate_from_stdout() {
        let script = "#!/bin/sh\nprintf '%s\n' 'stdout-json-line-placeholder'\necho 'a stderr warning' >&2\nprintf '%s\n' '{\"type\":\"x\"}'\n";
        let path = write_fake(script, "stderr");
        let transport = Transport::spawn_rpc(path.to_str().unwrap(), &[]).unwrap();

        let mut stderr_text = String::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if let Ok(line) = transport.stderr.recv_timeout(Duration::from_millis(200)) {
                stderr_text.push_str(&line.text);
                stderr_text.push('\n');
                if stderr_text.contains("a stderr warning") {
                    break;
                }
            }
        }
        assert!(stderr_text.contains("a stderr warning"), "stderr: {stderr_text:?}");

        let stdout_msgs = drain(&transport);
        let stdout_dump: String = stdout_msgs
            .iter()
            .map(|m| match m {
                WireMessage::Json(v) => v.to_string(),
                WireMessage::ParseError { raw } => raw.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(stdout_dump.contains("stdout-json-line-placeholder"));
        assert!(!stdout_dump.contains("a stderr warning"));

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
