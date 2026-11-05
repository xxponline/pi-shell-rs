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
    /// `clear_queue` 的返回数据（alt+up 取回排队消息，主线程回填输入框）。
    RestoreQueued(serde_json::Value),
    /// 成功类提示（如"✓ 会话已删除"），显示为绿色通知。
    Info(String),
    /// /model 参数无精确匹配时回退打开模型选择器。
    OpenModelSelect,
    /// 一次性线程取回的异步数据（选择器加载/模型与级别应用结果）。
    AsyncData { kind: AsyncKind, seq: u64, data: serde_json::Value },
}

/// 异步数据类别（决定主线程如何消费）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsyncKind {
    /// get_available_models 的模型列表（按 seq 匹配当前选择器）。
    Models,
    /// get_available_thinking_levels 的级别列表（按 seq 匹配）。
    ThinkingLevels,
    /// set_model 成功，data 为 Model。
    ModelApplied,
    /// cycle_model 结果，data 可能为 null（无可用模型）。
    CycleModel,
    /// set_thinking_level 成功，data 为新级别字符串。
    ThinkingApplied,
    /// cycle_thinking_level 结果，data 为 {level} 或 null。
    CycleThinking,
    /// 无需回填状态的命令完成（如 set_session_name），data 被忽略。
    CommandDone,
    /// get_state 响应：应用状态 + 更新 session_file。
    StateApplied,
    /// get_messages 响应：load_messages 重建 transcript。
    Messages,
    /// 会话列表（SessionSelect 数据），按 seq 匹配填充。
    SessionList,
    /// switch/new/fork/clone 的首跳响应：{command, cancelled}。
    SessionSwitched,
    /// get_tree 响应：填充树浮层（按 seq）。
    Tree,
    /// get_session_stats 响应：填充信息面板（按 seq）。
    Stats,
    /// get_available_models 响应：填充 /model 参数补全缓存。
    ArgCacheModels,
    /// get_available_thinking_levels 响应：填充 /thinking 参数补全缓存。
    ArgCacheLevels,
    /// get_last_assistant_text 响应：/copy 写入剪贴板。
    CopyText,
    /// /settings 打开时的 get_state 响应：回填开关与模型只读行。
    SettingsState,
    /// /scoped-models 的 get_available_models 响应：填充只读列表。
    ScopedModels,
    /// bash 命令结果（BashResult）→ 落 Entry::Bash。
    BashDone,
    /// export_html 响应：{path} → 绿条提示。
    ExportDone,
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
