//! Structured, terminal-independent conversation transcript and run state.

use serde_json::Value;

use crate::session::blocks::Block;

/// Agent run phase.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    Streaming,
}

/// Token usage/cost snapshot carried on message_update / message_end.
#[derive(Debug, Clone, Default, PartialEq)]
/// 一次请求的 token 用量与费用快照（来自 wire 的 usage 对象）。
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total_tokens: u64,
    /// 各项费用（美元），来自 `cost.*`；缺失时为 0。
    pub cost_input: f64,
    pub cost_output: f64,
    pub cost_cache_read: f64,
    pub cost_cache_write: f64,
    /// 总费用（美元），来自 `cost.total`；缺失时为 0。
    pub cost_total: f64,
}

impl Usage {
    /// 从 wire 的 usage JSON 解析用量与费用。
    ///
    /// 输入: v — usage 对象（可能缺 cost.*）；非对象返回 None
    /// 输出: Some(Usage)，token 计数缺失为 0，费用缺失为 0（不报错）
    pub fn from_json(v: &Value) -> Option<Self> {
        let obj = v.as_object()?;
        let cost = obj.get("cost");
        let cost_f64 = |key: &str| -> f64 {
            cost.and_then(|c| c.get(key))
                .and_then(|x| x.as_f64())
                .unwrap_or(0.0)
        };
        Some(Usage {
            input: obj.get("input").and_then(|x| x.as_u64()).unwrap_or(0),
            output: obj.get("output").and_then(|x| x.as_u64()).unwrap_or(0),
            cache_read: obj.get("cacheRead").and_then(|x| x.as_u64()).unwrap_or(0),
            cache_write: obj.get("cacheWrite").and_then(|x| x.as_u64()).unwrap_or(0),
            total_tokens: obj.get("totalTokens").and_then(|x| x.as_u64()).unwrap_or(0),
            cost_input: cost_f64("input"),
            cost_output: cost_f64("output"),
            cost_cache_read: cost_f64("cacheRead"),
            cost_cache_write: cost_f64("cacheWrite"),
            cost_total: cost_f64("total"),
        })
    }
}

/// One ordered transcript entry.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    User { text: String },
    Assistant {
        /// Monotonic turn index (0-based).
        turn: usize,
        blocks: Vec<Block>,
        usage: Option<Usage>,
    },
    /// Non-message notices (compaction, auto-retry, queue changes, ...).
    Notice { kind: String, text: String },
}

impl Entry {
    pub fn as_assistant_mut(&mut self) -> Option<&mut Vec<Block>> {
        match self {
            Entry::Assistant { blocks, .. } => Some(blocks),
            _ => None,
        }
    }
}

/// 会话权威状态：由 RPC 事件流归约得到，与终端无关。
#[derive(Debug, Clone, Default)]
pub struct SessionState {
    pub phase: Phase,
    pub thinking_level: Option<String>,
    pub session_name: Option<String>,
    /// 当前模型标识（来自 get_state 响应的 model.name/id）。
    pub model: Option<String>,
    pub steering_queue: Vec<String>,
    pub follow_up_queue: Vec<String>,
    pub usage: Option<Usage>,
    pub transcript: Vec<Entry>,

    /// Current turn counter (incremented on turn_start; 0 before first turn).
    current_turn: usize,
    /// In-flight assistant blocks for the streaming assistant message.
    inflight: Vec<Block>,
    /// Tool executions seen before their toolCall block arrives, keyed by id.
    pending_tools: std::collections::HashMap<String, crate::session::blocks::ToolExecution>,
    /// Whether the in-flight assistant belongs to the current turn's appended entry.
    inflight_appended: bool,
}

impl SessionState {
    /// 创建空闲态会话状态。
    ///
    /// 输入: 无
    /// 输出: 全字段默认值的 SessionState
    pub fn new() -> Self {
        Self::default()
    }

    /// 把 `get_state` 响应（RpcSessionState）应用到状态。
    ///
    /// 输入: v — get_state 的 data 对象（model/thinkingLevel/sessionName 等）
    /// 输出: 无返回值；设置 model/thinking_level/session_name，
    ///       字段缺失时保留既有值
    pub fn apply_get_state(&mut self, v: &Value) {
        if let Some(model) = v.get("model") {
            // Model 形状: { id, name, ... }；优先 name，退回 id。
            let name = model
                .get("name")
                .or_else(|| model.get("id"))
                .and_then(|x| x.as_str())
                .map(String::from);
            if name.is_some() {
                self.model = name;
            }
        }
        if let Some(level) = v.get("thinkingLevel").and_then(|x| x.as_str()) {
            self.thinking_level = Some(level.to_string());
        }
        if let Some(name) = v.get("sessionName").and_then(|x| x.as_str()) {
            self.session_name = Some(name.to_string());
        }
    }

    /// Ordered transcript entries.
    pub fn entries(&self) -> &[Entry] {
        &self.transcript
    }

    /// Blocks for the assistant message currently streaming but not yet
    /// finalized by `message_end`. Empty when nothing is in flight. Reading is
    /// non-mutating.
    pub fn in_flight(&self) -> &[Block] {
        &self.inflight
    }

    /// Whether an assistant message is currently accumulating.
    pub fn is_in_flight(&self) -> bool {
        !self.inflight.is_empty()
    }

    /// Apply one top-level wire event (weakly typed JSON). Unknown event types
    /// are safely ignored.
    pub fn handle(&mut self, event: &Value) {
        let Some(kind) = event.get("type").and_then(|t| t.as_str()) else {
            return;
        };
        match kind {
            "agent_start" => self.phase = Phase::Streaming,
            "agent_settled" => {
                self.flush_inflight();
                self.phase = Phase::Idle;
            }
            "turn_start" => {
                self.phase = Phase::Streaming;
                self.current_turn += 1;
            }
            "turn_end" => {
                // turn_end repeats the assistant message already finalized by its
                // message_end, so it must not append a duplicate entry. Use it
                // only to backfill authoritative content/tool results onto the
                // existing transcript entry for the same toolCall ids.
                if let Some(message) = event.get("message")
                    && message.get("role").and_then(|r| r.as_str()) == Some("assistant") {
                        self.correct_existing_from_message(message);
                    }
                if let Some(results) = event.get("toolResults").and_then(|v| v.as_array()) {
                    for tr in results {
                        self.apply_tool_result_message(tr);
                    }
                }
            }
            "message_start" => self.on_message_start(event.get("message")),
            "message_update" => self.on_message_update(event),
            "message_end" => self.on_message_end(event.get("message")),
            "tool_execution_start" => self.on_tool_start(event),
            "tool_execution_update" => self.on_tool_update(event),
            "tool_execution_end" => self.on_tool_end(event),
            "queue_update" => {
                if let Some(q) = event.get("steering").and_then(|v| v.as_array()) {
                    self.steering_queue =
                        q.iter().filter_map(|x| x.as_str().map(String::from)).collect();
                }
                if let Some(q) = event.get("followUp").and_then(|v| v.as_array()) {
                    self.follow_up_queue =
                        q.iter().filter_map(|x| x.as_str().map(String::from)).collect();
                }
            }
            "thinking_level_changed" => {
                if let Some(l) = event.get("level").and_then(|v| v.as_str()) {
                    self.thinking_level = Some(l.to_string());
                }
            }
            "session_info_changed" => {
                self.session_name = event
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(String::from);
            }
            // 自动重试：归约为对话流通知（不影响 phase/消息归约）。
            "auto_retry_start" => {
                let attempt = event.get("attempt").and_then(|x| x.as_u64()).unwrap_or(0);
                let max = event.get("maxAttempts").and_then(|x| x.as_u64()).unwrap_or(0);
                let err = event
                    .get("errorMessage")
                    .and_then(|x| x.as_str())
                    .unwrap_or("?");
                self.transcript.push(Entry::Notice {
                    kind: "retry".to_string(),
                    text: format!("重试 {attempt}/{max}: {}", summarize(err)),
                });
            }
            "auto_retry_end" => {
                let success = event.get("success").and_then(|x| x.as_bool()).unwrap_or(false);
                let text = if success {
                    "重试成功".to_string()
                } else {
                    let final_err = event
                        .get("finalError")
                        .and_then(|x| x.as_str())
                        .unwrap_or("未知错误");
                    format!("重试失败: {}", summarize(final_err))
                };
                self.transcript.push(Entry::Notice {
                    kind: "retry".to_string(),
                    text,
                });
            }
            // Captured but not modeled in depth this phase:
            "compaction_start" | "compaction_end"
            | "summarization_retry_scheduled"
            | "summarization_retry_attempt_start" | "summarization_retry_finished"
            | "entry_appended" | "bash_execution_update" | "agent_end" => {}
            _ => {}
        }
    }

    fn on_message_start(&mut self, message: Option<&Value>) {
        let Some(message) = message else { return };
        match message.get("role").and_then(|r| r.as_str()) {
            Some("assistant") => {
                self.inflight = Vec::new();
                self.inflight_appended = false;
            }
            Some("user") => {
                if let Some(text) = extract_user_text(message) {
                    self.transcript.push(Entry::User { text });
                }
            }
            // system and anything else are ignored for transcript purposes.
            _ => {}
        }
    }

    fn on_message_update(&mut self, event: &Value) {
        if let Some(u) = event.get("usage")
            && let Some(usage) = Usage::from_json(u) {
                self.usage = Some(usage);
            }
        let Some(ev) = event.get("assistantMessageEvent") else { return };
        let etype = ev.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let index = ev.get("contentIndex").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
        match etype {
            "text_start" => self.ensure_block(index, Block::Text { text: String::new() }),
            "text_delta" => {
                if let Some(d) = ev.get("delta").and_then(|d| d.as_str()) {
                    self.append_text(index, d, false);
                }
            }
            "text_end" => {
                if let Some(c) = ev.get("content").and_then(|c| c.as_str()) {
                    self.set_block(index, Block::Text { text: c.to_string() });
                }
            }
            "thinking_start" => {
                self.ensure_block(index, Block::Thinking { text: String::new() })
            }
            "thinking_delta" => {
                if let Some(d) = ev.get("delta").and_then(|d| d.as_str()) {
                    self.append_text(index, d, true);
                }
            }
            "thinking_end" => {
                if let Some(c) = ev.get("content").and_then(|c| c.as_str()) {
                    self.set_block(index, Block::Thinking { text: c.to_string() });
                }
            }
            "toolcall_start" => {
                // Flattened wire shape: id/toolName at the event top level.
                let id = ev
                    .get("id")
                    .and_then(|i| i.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = ev
                    .get("toolName")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string();
                let tool = self.pending_tools.remove(&id).unwrap_or_default();
                self.ensure_block(
                    index,
                    Block::ToolCall {
                        id,
                        name,
                        args_json: String::new(),
                        tool,
                    },
                );
            }
            "toolcall_delta" => {
                if let Some(d) = ev.get("delta").and_then(|d| d.as_str())
                    && let Some(Block::ToolCall { args_json, .. }) =
                        self.inflight.get_mut(index)
                    {
                        args_json.push_str(d);
                    }
            }
            "toolcall_end" => {
                if let Some(tc) = ev.get("toolCall") {
                    let id = tc
                        .get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = tc
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    let args = match tc.get("arguments") {
                        Some(Value::String(s)) => s.clone(),
                        Some(other) => other.to_string(),
                        None => String::new(),
                    };
                    let tool = self
                        .inflight
                        .get(index)
                        .and_then(|b| match b {
                            Block::ToolCall { tool, .. } => Some(tool.clone()),
                            _ => None,
                        })
                        .or_else(|| self.pending_tools.remove(&id))
                        .unwrap_or_default();
                    self.set_block(
                        index,
                        Block::ToolCall {
                            id,
                            name,
                            args_json: args,
                            tool,
                        },
                    );
                }
            }
            _ => {}
        }
    }

    /// 处理 `message_end`：把 assistant 消息权威落盘；若携带错误信息，
    /// 额外追加一条 error 通知，避免运行级错误静默丢失。
    ///
    /// 输入: message — 权威完整消息（System/User/Assistant 等）；None 直接忽略
    /// 输出: 无返回值；副作用为更新 in-flight/transcript，
    ///       且当 assistant 消息含非空 errorMessage 时追加 Notice{kind:"error"}
    fn on_message_end(&mut self, message: Option<&Value>) {
        let Some(message) = message else { return };
        if message.get("role").and_then(|r| r.as_str()) == Some("assistant") {
            self.replace_inflight_from_message(message);
            self.flush_inflight();
            // 运行级错误（如模型 400/订阅失效）：content 可能为空，
            // 但 errorMessage 携带原因，必须呈现给用户。
            if let Some(err) = message.get("errorMessage").and_then(|e| e.as_str())
                && !err.is_empty()
            {
                self.transcript.push(Entry::Notice {
                    kind: "error".to_string(),
                    text: err.to_string(),
                });
            }
        }
    }

    fn on_tool_start(&mut self, event: &Value) {
        let Some(id) = event.get("toolCallId").and_then(|i| i.as_str()) else {
            return;
        };
        let exec = crate::session::blocks::ToolExecution {
            status: crate::session::blocks::ToolStatus::Running,
            ..Default::default()
        };
        self.apply_tool(id.to_string(), exec);
    }

    fn on_tool_update(&mut self, event: &Value) {
        // Updates carry partialResult; we only need to mark running existence.
        if let Some(id) = event.get("toolCallId").and_then(|i| i.as_str())
            && !self.has_tool(id) {
                self.apply_tool(
                    id.to_string(),
                    crate::session::blocks::ToolExecution {
                        status: crate::session::blocks::ToolStatus::Running,
                        ..Default::default()
                    },
                );
            }
    }

    fn on_tool_end(&mut self, event: &Value) {
        let Some(id) = event.get("toolCallId").and_then(|i| i.as_str()) else {
            return;
        };
        let exec = crate::session::blocks::ToolExecution {
            status: crate::session::blocks::ToolStatus::Ended,
            is_error: event.get("isError").and_then(|b| b.as_bool()).unwrap_or(false),
            result: event.get("result").cloned(),
            duration_ms: event.get("durationMs").and_then(|d| d.as_u64()),
        };
        self.apply_tool(id.to_string(), exec);
    }

    // ---- block helpers ----

    fn ensure_block(&mut self, index: usize, block: Block) {
        while self.inflight.len() <= index {
            self.inflight.push(Block::Text {
                text: String::new(),
            });
        }
        // Only fill an empty-text placeholder with a different variant; never
        // overwrite a block that already carries content or a matching variant.
        let slot_is_empty_text_placeholder =
            matches!(self.inflight[index], Block::Text { ref text } if text.is_empty());
        let same_variant = std::mem::discriminant(&self.inflight[index])
            == std::mem::discriminant(&block);
        if slot_is_empty_text_placeholder && !same_variant {
            self.inflight[index] = block;
        }
    }

    fn set_block(&mut self, index: usize, block: Block) {
        while self.inflight.len() <= index {
            self.inflight.push(Block::Text {
                text: String::new(),
            });
        }
        self.inflight[index] = block;
    }

    fn append_text(&mut self, index: usize, delta: &str, thinking: bool) {
        if self.inflight.len() <= index {
            self.inflight.push(if thinking {
                Block::Thinking {
                    text: String::new(),
                }
            } else {
                Block::Text {
                    text: String::new(),
                }
            });
        }
        let slot = &mut self.inflight[index];
        let is_text = matches!(slot, Block::Text { .. });
        let is_think = matches!(slot, Block::Thinking { .. });
        if (thinking && is_think) || (!thinking && is_text) {
            match slot {
                Block::Text { text } | Block::Thinking { text } => text.push_str(delta),
                _ => {}
            }
        }
    }

    fn replace_inflight_from_message(&mut self, message: &Value) {
        let Some(content) = message.get("content").and_then(|c| c.as_array()) else {
            return;
        };
        let mut blocks: Vec<Block> = Vec::new();
        for item in content {
            match item.get("type").and_then(|t| t.as_str()) {
                Some("text") => blocks.push(Block::Text {
                    text: item
                        .get("text")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string(),
                }),
                Some("thinking") => blocks.push(Block::Thinking {
                    text: item
                        .get("thinking")
                        .or_else(|| item.get("text"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string(),
                }),
                Some("toolCall") => {
                    let id = item
                        .get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = item
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    let args = match item.get("arguments") {
                        Some(Value::String(s)) => s.clone(),
                        Some(other) => other.to_string(),
                        None => String::new(),
                    };
                    let tool = self
                        .pending_tools
                        .remove(&id)
                        .or_else(|| {
                            // Preserve execution already attached to inflight.
                            self.inflight.iter().find_map(|b| match b {
                                Block::ToolCall {
                                    id: bid,
                                    tool,
                                    ..
                                } if bid == &id => Some(tool.clone()),
                                _ => None,
                            })
                        })
                        .or_else(|| self.find_tool_in_transcript(&id))
                        .unwrap_or_default();
                    blocks.push(Block::ToolCall {
                        id,
                        name,
                        args_json: args,
                        tool,
                    });
                }
                _ => {}
            }
        }
        self.inflight = blocks;
    }

    /// Attach a tool execution to the in-flight block with this id, or stash it.
    fn apply_tool(&mut self, id: String, exec: crate::session::blocks::ToolExecution) {
        for b in self.inflight.iter_mut() {
            if let Block::ToolCall {
                id: bid, tool, ..
            } = b
                && bid == &id {
                    *tool = merge_tool(tool, exec);
                    return;
                }
        }
        // Also update an already-flushed assistant entry (tool_end may follow
        // message_end ordering).
        for entry in self.transcript.iter_mut().rev() {
            if let Entry::Assistant { blocks, .. } = entry {
                for b in blocks.iter_mut() {
                    if let Block::ToolCall {
                        id: bid, tool, ..
                    } = b
                        && bid == &id {
                            *tool = merge_tool(tool, exec);
                            return;
                        }
                }
            }
        }
        self.pending_tools.insert(id, exec);
    }

    /// Find an already-flushed tool execution by id (authoritative rebuilds
    /// such as turn_end must preserve execution results attached earlier).
    fn find_tool_in_transcript(
        &self,
        id: &str,
    ) -> Option<crate::session::blocks::ToolExecution> {
        for entry in self.transcript.iter().rev() {
            if let Entry::Assistant { blocks, .. } = entry {
                for b in blocks {
                    if let Block::ToolCall {
                        id: bid,
                        tool,
                        ..
                    } = b
                        && bid == id {
                            return Some(tool.clone());
                        }
                }
            }
        }
        None
    }

    fn has_tool(&self, id: &str) -> bool {
        self.inflight.iter().any(|b| {
            matches!(b, Block::ToolCall { id: bid, .. } if bid == id)
        }) || self.pending_tools.contains_key(id)
    }

    /// Apply a turn_end assistant message onto an already-flushed entry:
    /// update text/thinking content and preserve attached tool executions.
    fn correct_existing_from_message(&mut self, message: &Value) {
        let Some(content) = message.get("content").and_then(|c| c.as_array()) else {
            return;
        };
        let Some(last) = self
            .transcript
            .iter_mut()
            .rev()
            .find(|e| matches!(e, Entry::Assistant { .. }))
        else {
            return;
        };
        let Entry::Assistant { blocks, .. } = last else {
            return;
        };
        for (i, item) in content.iter().enumerate() {
            let Some(existing) = blocks.get_mut(i) else {
                // Content grew (e.g. late block); append what we know.
                match item.get("type").and_then(|t| t.as_str()) {
                    Some("text") => blocks.push(Block::Text {
                        text: item.get("text").and_then(|t| t.as_str()).unwrap_or("").into(),
                    }),
                    Some("thinking") => blocks.push(Block::Thinking {
                        text: item
                            .get("thinking")
                            .or_else(|| item.get("text"))
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .into(),
                    }),
                    _ => {}
                }
                continue;
            };
            match (existing, item.get("type").and_then(|t| t.as_str())) {
                (Block::Text { text }, Some("text")) => {
                    if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                        *text = t.to_string();
                    }
                }
                (Block::Thinking { text }, Some("thinking")) => {
                    if let Some(t) = item
                        .get("thinking")
                        .or_else(|| item.get("text"))
                        .and_then(|t| t.as_str())
                    {
                        *text = t.to_string();
                    }
                }
                _ => {}
            }
        }
    }

    /// Backfill a tool result delivered in turn_end.toolResults (keyed by
    /// toolCallId) onto an existing ToolCall block.
    fn apply_tool_result_message(&mut self, tr: &Value) {
        let Some(id) = tr.get("toolCallId").and_then(|i| i.as_str()) else {
            return;
        };
        let exec = crate::session::blocks::ToolExecution {
            status: crate::session::blocks::ToolStatus::Ended,
            is_error: tr.get("isError").and_then(|b| b.as_bool()).unwrap_or(false),
            result: Some({
                // 透传卡片需要的字段：content（必须）+ structuredContent/details（可选）。
                let mut obj = serde_json::Map::new();
                obj.insert("content".into(), tr.get("content").cloned().unwrap_or(Value::Null));
                for key in ["structuredContent", "details"] {
                    if let Some(v) = tr.get(key) {
                        obj.insert(key.to_string(), v.clone());
                    }
                }
                Value::Object(obj)
            }),
            duration_ms: tr.get("durationMs").and_then(|d| d.as_u64()),
        };
        self.apply_tool(id.to_string(), exec);
    }

    /// Ensure the in-flight assistant is represented by a live transcript
    /// entry (created on first content), returning its index.
    fn ensure_assistant_entry(&mut self) -> usize {
        // Reuse the trailing assistant entry if it belongs to the same turn and
        // is the one we are streaming into.
        if self.inflight_appended
            && let Some(pos) = self.transcript.iter().rposition(|e| {
                matches!(e, Entry::Assistant { .. })
            }) {
                return pos;
            }
        let turn = self.current_turn.max(1);
        self.transcript.push(Entry::Assistant {
            turn,
            blocks: Vec::new(),
            usage: self.usage.clone(),
        });
        self.inflight_appended = true;
        self.transcript.len() - 1
    }

    /// Promote the in-flight assistant blocks into the live transcript entry.
    fn flush_inflight(&mut self) {
        if self.inflight.is_empty() {
            self.inflight_appended = false;
            return;
        }
        let blocks = std::mem::take(&mut self.inflight);
        let pos = self.ensure_assistant_entry();
        if let Some(Entry::Assistant {
            blocks: dst,
            usage: u,
            ..
        }) = self.transcript.get_mut(pos)
        {
            *dst = blocks;
            if self.usage.is_some() {
                *u = self.usage.clone();
            }
        }
        self.inflight_appended = false;
    }
}

fn merge_tool(
    base: &crate::session::blocks::ToolExecution,
    new: crate::session::blocks::ToolExecution,
) -> crate::session::blocks::ToolExecution {
    crate::session::blocks::ToolExecution {
        status: if matches!(new.status, crate::session::blocks::ToolStatus::Ended) {
            crate::session::blocks::ToolStatus::Ended
        } else {
            base.status.clone()
        },
        is_error: new.is_error || base.is_error,
        result: merge_result(base.result.as_ref(), new.result.as_ref()),
        duration_ms: new.duration_ms.or(base.duration_ms),
    }
}

/// 合并两次来源的工具结果：都是对象时按键合并（new 覆盖 base 的同名键，
/// 保留 base 独有键如 structuredContent/details），否则取 new 再退回 base。
///
/// 输入: base — 较早的结果（如 tool_execution_end 的完整 result）；
///       new — 较晚的结果（如 turn_end toolResults 的 content-only 对象）
/// 输出: 合并后的结果对象；两者皆 None 则 None
fn merge_result(base: Option<&Value>, new: Option<&Value>) -> Option<Value> {
    match (base, new) {
        (Some(Value::Object(b)), Some(Value::Object(n))) => {
            let mut merged = b.clone();
            for (k, v) in n {
                merged.insert(k.clone(), v.clone());
            }
            Some(Value::Object(merged))
        }
        (_, Some(n)) => Some(n.clone()),
        (Some(b), None) => Some(b.clone()),
        (None, None) => None,
    }
}

/// 把错误文本压缩为单行摘要（取首行、限长），用于通知行。
///
/// 输入: text — 原始错误文本（可能多行/很长）
/// 输出: 单行摘要，超 80 字符截断加 …
fn summarize(text: &str) -> String {
    let first_line = text.lines().next().unwrap_or(text).trim();
    if first_line.chars().count() <= 80 {
        first_line.to_string()
    } else {
        let taken: String = first_line.chars().take(79).collect();
        format!("{taken}…")
    }
}

/// 从用户消息提取可显示文本（字符串或 text/image 内容块）。
///
/// 输入: message — 用户消息对象
/// 输出: Some(拼接文本)；image 记为 [image]；无可提取内容返回 None
fn extract_user_text(message: &Value) -> Option<String> {
    if let Some(s) = message.get("content").and_then(|c| c.as_str()) {
        return Some(s.to_string());
    }
    let arr = message.get("content").and_then(|c| c.as_array())?;
    let mut parts = Vec::new();
    for item in arr {
        match item.get("type").and_then(|t| t.as_str()) {
            Some("text") => {
                if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                    parts.push(t.to_string());
                }
            }
            Some("image") => parts.push("[image]".to_string()),
            _ => {}
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lifecycle_phase_and_user_assistant() {
        let mut s = SessionState::new();
        assert_eq!(s.phase, Phase::Idle);
        s.handle(&json!({"type":"agent_start"}));
        assert_eq!(s.phase, Phase::Streaming);
        s.handle(&json!({"type":"message_start","message":{"role":"user","content":[{"type":"text","text":"hello"}]}}));
        s.handle(&json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        s.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":0}}));
        s.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"hi"}}));
        s.handle(&json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"hi"}]}}));
        s.handle(&json!({"type":"agent_settled"}));
        assert_eq!(s.phase, Phase::Idle);
        assert!(matches!(s.entries()[0], Entry::User { .. }));
        assert!(matches!(s.entries()[1], Entry::Assistant { .. }));
    }

    #[test]
    fn unknown_events_are_ignored() {
        let mut s = SessionState::new();
        let before = s.entries().len();
        s.handle(&json!({"type":"some_future_event_xyz","x":[1,2,3]}));
        s.handle(&json!({"no_type":true}));
        assert_eq!(s.entries().len(), before);
    }

    #[test]
    fn delta_accumulation_matches_final() {
        let mut s = SessionState::new();
        s.handle(&json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        s.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_start","contentIndex":0}}));
        s.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","contentIndex":0,"delta":"ab"}}));
        s.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","contentIndex":0,"delta":"cd"}}));
        s.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":1}}));
        s.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":1,"delta":"xy"}}));
        s.handle(&json!({"type":"message_end","message":{"role":"assistant","content":[
            {"type":"thinking","thinking":"abcd"},
            {"type":"text","text":"xy"}
        ]}}));
        let blocks = match &s.entries()[0] {
            Entry::Assistant { blocks, .. } => blocks,
            _ => panic!("assistant"),
        };
        assert_eq!(blocks[0], Block::Thinking { text: "abcd".into() });
        assert_eq!(blocks[1], Block::Text { text: "xy".into() });
    }

    #[test]
    fn toolcall_uses_flattened_id_and_tool_execution() {
        let mut s = SessionState::new();
        s.handle(&json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        s.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"toolcall_start","contentIndex":0,"id":"call_1","toolName":"bash"}}));
        s.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"toolcall_delta","contentIndex":0,"delta":"{\"co"}}));
        s.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"toolcall_delta","contentIndex":0,"delta":"mmand\":\"ls\"}"}}));
        s.handle(&json!({"type":"tool_execution_start","toolCallId":"call_1","toolName":"bash","args":{}}));
        s.handle(&json!({"type":"tool_execution_end","toolCallId":"call_1","toolName":"bash","isError":false,"result":{"content":[{"type":"text","text":"out"}]},"durationMs":12}));
        s.handle(&json!({"type":"message_end","message":{"role":"assistant","content":[
            {"type":"toolCall","id":"call_1","name":"bash","arguments":{"command":"ls"}}
        ]}}));
        match &s.entries()[0] {
            Entry::Assistant { blocks, .. } => match &blocks[0] {
                Block::ToolCall { id, name, args_json, tool } => {
                    assert_eq!(id, "call_1");
                    assert_eq!(name, "bash");
                    assert!(args_json.contains("ls"));
                    assert!(!tool.is_error);
                    assert_eq!(tool.duration_ms, Some(12));
                    assert!(tool.result.is_some());
                }
                other => panic!("{other:?}"),
            },
            _ => panic!(),
        }
    }

    #[test]
    fn tool_event_before_block_is_stashed_and_merged() {
        let mut s = SessionState::new();
        s.handle(&json!({"type":"tool_execution_end","toolCallId":"call_late","toolName":"read","isError":true,"result":{"content":[]}}));
        s.handle(&json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        s.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"toolcall_start","contentIndex":0,"id":"call_late","toolName":"read"}}));
        s.handle(&json!({"type":"message_end","message":{"role":"assistant","content":[
            {"type":"toolCall","id":"call_late","name":"read","arguments":{}}
        ]}}));
        match &s.entries()[0] {
            Entry::Assistant { blocks, .. } => match &blocks[0] {
                Block::ToolCall { id, tool, .. } => {
                    assert_eq!(id, "call_late");
                    assert!(tool.is_error);
                }
                other => panic!("{other:?}"),
            },
            _ => panic!(),
        }
    }

    #[test]
    fn queue_thinking_name_updates() {
        let mut s = SessionState::new();
        s.handle(&json!({"type":"queue_update","steering":["a"],"followUp":["b","c"]}));
        assert_eq!(s.steering_queue, vec!["a".to_string()]);
        assert_eq!(s.follow_up_queue, vec!["b".to_string(), "c".to_string()]);
        s.handle(&json!({"type":"thinking_level_changed","level":"high"}));
        assert_eq!(s.thinking_level.as_deref(), Some("high"));
        s.handle(&json!({"type":"session_info_changed","name":"my session"}));
        assert_eq!(s.session_name.as_deref(), Some("my session"));
    }

    #[test]
    fn multi_turn_keeps_all_assistants() {
        let mut s = SessionState::new();
        for (user, answer) in [("one", "answer1"), ("two", "answer2")] {
            s.handle(&json!({"type":"turn_start"}));
            s.handle(&json!({"type":"message_start","message":{"role":"user","content":[{"type":"text","text":user}]}}));
            s.handle(&json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
            s.handle(&json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":answer}]}}));
            s.handle(&json!({"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":answer}]},"toolResults":[]}));
        }
        let users = s.entries().iter().filter(|e| matches!(e, Entry::User { .. })).count();
        let assistants = s.entries().iter().filter(|e| matches!(e, Entry::Assistant { .. })).count();
        assert_eq!((users, assistants), (2, 2), "{:?}", s.entries());
        let texts: Vec<String> = s
            .entries()
            .iter()
            .filter_map(|e| match e {
                Entry::Assistant { blocks, .. } => blocks
                    .iter()
                    .find_map(|b| match b { Block::Text { text } => Some(text.clone()), _ => None }),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["answer1".to_string(), "answer2".to_string()]);
    }

    #[test]
    fn in_flight_accessor_tracks_streaming_lifecycle() {
        let mut s = SessionState::new();
        assert!(!s.is_in_flight());
        assert!(s.in_flight().is_empty());

        s.handle(&json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        s.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":0}}));
        s.handle(&json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"abc"}}));
        // During streaming, in-flight blocks are visible and transcript untouched.
        assert!(s.is_in_flight());
        let visible: Vec<&Block> = s.in_flight().iter().collect();
        assert!(visible.iter().any(|b| matches!(b, Block::Text { text } if text == "abc")));
        let entries_before = s.entries().len();
        let _ = s.in_flight(); // read is non-mutating
        assert_eq!(s.entries().len(), entries_before);

        s.handle(&json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"abc"}]}}));
        // After finalization, in-flight clears and the entry exists.
        assert!(!s.is_in_flight());
        assert!(s.in_flight().is_empty());
        assert!(s
            .entries()
            .iter()
            .any(|e| matches!(e, Entry::Assistant { .. })));
    }

    #[test]
    fn assistant_error_message_becomes_error_notice() {
        let mut s = SessionState::new();
        s.handle(&json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        s.handle(&json!({"type":"message_end","message":{
            "role":"assistant","content":[],"stopReason":"error",
            "errorMessage":"400: InvalidSubscription (test)"
        }}));
        let notices: Vec<(&str,&str)> = s.entries().iter().filter_map(|e| match e {
            Entry::Notice { kind, text } => Some((kind.as_str(), text.as_str())),
            _ => None,
        }).collect();
        assert_eq!(notices, vec![("error", "400: InvalidSubscription (test)")], "{:?}", s.entries());
        // turn_end 重复同一消息时不得重复追加通知
        s.handle(&json!({"type":"turn_end","message":{
            "role":"assistant","content":[],"stopReason":"error",
            "errorMessage":"400: InvalidSubscription (test)"
        },"toolResults":[]}));
        let count = s.entries().iter().filter(|e| matches!(e, Entry::Notice { kind, .. } if kind == "error")).count();
        assert_eq!(count, 1, "notice must not duplicate on turn_end");
    }

    #[test]
    fn normal_assistant_message_has_no_error_notice() {
        let mut s = SessionState::new();
        s.handle(&json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        s.handle(&json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"ok"}]}}));
        assert!(!s.entries().iter().any(|e| matches!(e, Entry::Notice { .. })));
    }

    #[test]
    fn usage_parses_cost_and_tokens() {
        let v = json!({"input":10,"output":5,"cacheRead":3,"cacheWrite":1,
            "totalTokens":15,
            "cost":{"input":0.001,"output":0.002,"cacheRead":0.0003,"cacheWrite":0.0001,"total":0.012}});
        let u = Usage::from_json(&v).expect("parse");
        assert_eq!((u.input, u.output, u.total_tokens), (10, 5, 15));
        assert_eq!(u.cost_total, 0.012);
        assert_eq!(u.cost_input, 0.001);
    }

    #[test]
    fn usage_missing_cost_defaults_to_zero() {
        let v = json!({"input":1,"output":2,"cacheRead":0,"cacheWrite":0,"totalTokens":3});
        let u = Usage::from_json(&v).expect("parse");
        assert_eq!(u.total_tokens, 3);
        assert_eq!(u.cost_total, 0.0);
        assert_eq!(u.cost_input, 0.0);
        // 非对象返回 None
        assert!(Usage::from_json(&json!([1,2])).is_none());
    }

    #[test]
    fn auto_retry_events_become_retry_notices() {
        let mut s = SessionState::new();
        // 先有正常消息，确保 phase/条目不受影响
        s.handle(&json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        s.handle(&json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"hi"}]}}));
        let msgs_before = s.entries().iter().filter(|e| matches!(e, Entry::Assistant { .. })).count();

        s.handle(&json!({"type":"auto_retry_start","attempt":1,"maxAttempts":3,"delayMs":500,"errorMessage":"500 boom\nsecond line"}));
        s.handle(&json!({"type":"auto_retry_end","success":true,"attempt":1}));

        let retries: Vec<&str> = s.entries().iter().filter_map(|e| match e {
            Entry::Notice { kind, text } if kind == "retry" => Some(text.as_str()),
            _ => None,
        }).collect();
        assert_eq!(retries.len(), 2, "{:?}", s.entries());
        assert!(retries[0].contains("重试 1/3"), "{}", retries[0]);
        assert!(retries[0].contains("500 boom"), "{}", retries[0]);
        assert!(!retries[0].contains("second line"), "摘要应只取首行: {}", retries[0]);
        assert_eq!(retries[1], "重试成功");

        // phase 与消息条目不受影响
        let msgs_after = s.entries().iter().filter(|e| matches!(e, Entry::Assistant { .. })).count();
        assert_eq!(msgs_before, msgs_after);
    }

    #[test]
    fn auto_retry_end_failure_reports_final_error() {
        let mut s = SessionState::new();
        s.handle(&json!({"type":"auto_retry_end","success":false,"attempt":3,"finalError":"still broken"}));
        let text = s.entries().iter().find_map(|e| match e {
            Entry::Notice { kind, text } if kind == "retry" => Some(text.as_str()),
            _ => None,
        }).expect("retry notice");
        assert!(text.contains("重试失败"), "{}", text);
        assert!(text.contains("still broken"), "{}", text);
    }

    #[test]
    fn apply_get_state_sets_fields() {
        let mut s = SessionState::new();
        s.apply_get_state(&json!({
            "model": {"id":"mimo-v2.6-pro","name":"MiMo v2.6 Pro"},
            "thinkingLevel": "high",
            "sessionName": "my session",
            "sessionId": "abc"
        }));
        assert_eq!(s.model.as_deref(), Some("MiMo v2.6 Pro"));
        assert_eq!(s.thinking_level.as_deref(), Some("high"));
        assert_eq!(s.session_name.as_deref(), Some("my session"));
    }

    #[test]
    fn apply_get_state_missing_fields_keep_old_values() {
        let mut s = SessionState::new();
        s.apply_get_state(&json!({"model": {"id":"x","name":"X"}, "sessionName": "keep me"}));
        // 缺 thinkingLevel → 保留 None
        assert_eq!(s.thinking_level, None);
        s.apply_get_state(&json!({"thinkingLevel": "low"}));
        // 缺 sessionName → 保留旧值
        assert_eq!(s.session_name.as_deref(), Some("keep me"));
        assert_eq!(s.thinking_level.as_deref(), Some("low"));
        // model 只有 id 无 name → 退回 id
        s.apply_get_state(&json!({"model": {"id":"only-id"}}));
        assert_eq!(s.model.as_deref(), Some("only-id"));
    }
}
