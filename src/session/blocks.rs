//! Assistant content blocks accumulated from streaming deltas.

/// One assistant content block, indexed by the wire `contentIndex`.
///
/// `allow(large_enum_variant)`：`gpui-support` 特性下 gpui-kit 传递统一了
/// `serde_json` 的 `preserve_order`/`raw_value`，`Value` 布局变大把 ToolCall
/// 变体推过 clippy 阈值（默认特性下不告警）。装箱会改动共享 API 构造点，
/// 违反 design D6「共享代码仅加法」，故以属性抑制（行为零变化）。
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
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
    /// 运行中的部分结果（`tool_execution_update` 的 `partialResult`）；
    /// end 到达后由 merge 规则清空，渲染取 `result.or(partial)`。
    pub partial: Option<serde_json::Value>,
    /// 执行起点（start 或 update 首见时打点），渲染 `Elapsed` 用
    /// `started_at.elapsed()`；Ended 后优先 `duration_ms`（design D2）。
    pub started_at: Option<std::time::Instant>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ToolStatus {
    #[default]
    Pending,
    Running,
    Ended,
}
