//! Unified UI event channel fed by two source threads: crossterm keyboard and
//! the RpcClient event stream.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use crossterm::event::{KeyEvent};
use crossterm::event::{self as ct_event};
use pi_rust_tui::rpc::{ClientEvent, RpcClient};
use serde_json::Value;

/// Everything the main loop reacts to.
#[derive(Debug, Clone)]
pub enum UiEvent {
    Key(KeyEvent),
    /// One inbound session event (raw JSON, fed to SessionState).
    Rpc(Value),
    /// A non-JSON stdout line, kept verbatim.
    ParseError(String),
    /// pi closed stdout.
    Closed,
    /// A detached command send reported failure.
    SendFailed(String),
}

/// Spawn the keyboard + RPC reader threads. Returns the receiving end and a
/// sender handle used by detached command threads to report failures.
pub fn spawn_event_sources(
    client: &mut RpcClient,
) -> (Receiver<UiEvent>, Sender<UiEvent>) {
    let (tx, rx) = mpsc::channel();

    // Keyboard source.
    let key_tx = tx.clone();
    thread::spawn(move || loop {
        match ct_event::read() {
            Ok(ct_event::Event::Key(key)) => {
                if key_tx.send(UiEvent::Key(key)).is_err() {
                    break;
                }
            }
            Ok(_) => {} // mouse/resize ignored in stage 2
            Err(_) => break,
        }
    });

    // RPC event source.
    let rpc_tx = tx.clone();
    let events = client.take_events();
    thread::spawn(move || loop {
        match events.recv() {
            Ok(ClientEvent::Event(v)) => {
                if rpc_tx.send(UiEvent::Rpc(v)).is_err() {
                    break;
                }
            }
            Ok(ClientEvent::ParseError { raw }) => {
                if rpc_tx.send(UiEvent::ParseError(raw)).is_err() {
                    break;
                }
            }
            Ok(ClientEvent::Closed) => {
                let _ = rpc_tx.send(UiEvent::Closed);
                break;
            }
            Err(_) => {
                let _ = rpc_tx.send(UiEvent::Closed);
                break;
            }
        }
    });

    // Keep one sender as the public report handle; it is dropped with the app.
    (rx, tx)
}
