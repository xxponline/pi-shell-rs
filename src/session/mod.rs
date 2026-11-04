//! Headless session state machine (terminal- and network-independent).

mod blocks;
mod replay;
mod transcript;

pub use blocks::{Block, ToolExecution, ToolStatus};
pub use replay::{replay_file, replay_jsonl};
pub use transcript::{Entry, Phase, SessionState, Usage};
