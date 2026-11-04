//! Reusable `pi --mode rpc` connection layer.

pub mod client;
pub mod jsonl;
pub mod process;

pub use client::RpcClient;
pub use jsonl::{Direction, OutgoingCommand, WireMessage};
pub use process::{resolve_binary, StderrLine, Transport, TransportEvent};
