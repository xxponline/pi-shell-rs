//! Assistant content blocks accumulated from streaming deltas.

/// One assistant content block, indexed by the wire `contentIndex`.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Text { text: String },
    Thinking { text: String },
    ToolCall {
        id: String,
        name: String,
        /// Streaming JSON arguments accumulated from toolcall_delta.
        args_json: String,
        tool: ToolExecution,
    },
}

/// Lifecycle/result of a tool call, normalized from tool_execution_* events.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolExecution {
    pub status: ToolStatus,
    pub is_error: bool,
    pub result: Option<serde_json::Value>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ToolStatus {
    #[default]
    Pending,
    Running,
    Ended,
}
