//! Unified UI event channel fed by two source threads: crossterm keyboard and
//! the RpcClient event stream.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use crossterm::event::{KeyEvent, KeyModifiers};
use crossterm::event::{self as ct_event};
use pi_shell_rs::rpc::{ClientEvent, RpcClient};
use serde_json::Value;

use crate::app::scroll::{WheelAccel, ALT_WHEEL_MULTIPLIER};

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
    /// 宿主请求关闭（embed shutdown）：退出事件循环，线程侧杀 pi 子进程。
    Shutdown,
    /// 终端尺寸改变：重绘以触发 ratatui autoresize。
    /// embed：宿主写共享槽后注入；standalone：SIGWINCH → crossterm Resize 转发，
    /// draw 时经 ioctl 取新尺寸。
    Resize,
    /// 滚轮滚动消息区：有符号行数（负=上滚看更早内容、正=下滚看更新内容）；
    /// 行数已在事件源线程经 WheelAccel 加速并含 Alt 倍率（design D2）。
    Wheel(i16),
    /// A detached command send reported failure.
    SendFailed(String),
    /// `clear_queue` 的返回数据（alt+up 取回排队消息，主线程回填输入框）。
    RestoreQueued(serde_json::Value),
    /// 成功类提示（如"✓ 会话已删除"），显示为绿色通知。
    Info(String),
    /// /model 参数无精确匹配时回退打开模型选择器。
    OpenModelSelect,
    /// get_commands 响应（动态命令缓存回填；静默拉取，仅成功时回报）。
    CommandsFetched(serde_json::Value),
    /// ctrl+g：请求外部编辑器（仅事件循环持有 terminal，挂起/恢复在那里执行）。
    OpenExternalEditor,
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

/// 事件源句柄：主循环消费端 + 上报端 + 键注入端。
///
/// - `events`: 主循环 `recv_timeout` 消费的事件流（键盘线程与 RPC 线程共用）
/// - `report`: detached 命令线程回报失败用
/// - `keys`: `UiEvent::Key` 注入入口——嵌入模式下宿主（GUI）经此送键，
///   crossterm poll 线程不启动（design D3）
pub struct EventSources {
    pub events: Receiver<UiEvent>,
    pub report: Sender<UiEvent>,
    pub keys: Sender<UiEvent>,
}

/// Spawn the RPC reader thread and (unless `embedded`) the crossterm
/// keyboard thread. `embedded = true` skips the keyboard thread so the host
/// injects keys through [`EventSources::keys`] instead of a real tty.
pub fn spawn_event_sources(
    client: &mut RpcClient,
    embedded: bool,
) -> EventSources {
    let (tx, rx) = mpsc::channel();

    // Keyboard source：poll + 暂停闸门（design D3）——外部编辑器子进程
    // 占用 stdin 期间，poll 到数据也不 read，字节留给子进程。
    let key_tx = tx.clone();
    if !embedded {
    let thread_key_tx = key_tx.clone();
    // 滚轮加速器随线程存活（design D2：时间戳在读取点打点，避免
    // 主循环 batch 处理时时间戳塌缩、速度信息丢失）。
    let mut accel = WheelAccel::new();
    thread::spawn(move || loop {
        // 暂停闸门前置（design D3 修订）：暂停期不进入 crossterm poll——
        // 既不会消费子进程（外部编辑器）的 stdin 字节，也不持有全局事件读锁
        // （否则与恢复路径的能力查询争锁死锁）。
        if crate::app::terminal::keyboard_paused() {
            thread::sleep(std::time::Duration::from_millis(50));
            continue;
        }
        match ct_event::poll(std::time::Duration::from_millis(150)) {
            Ok(true) => {
                // 二次确认（poll 等待期间可能被置位）
                if crate::app::terminal::keyboard_paused() {
                    continue;
                }
                match ct_event::read() {
                    Ok(ev) => {
                        // Resize 必须转发（否则改终端尺寸后不重绘）；滚轮翻译
                        // 为 UiEvent::Wheel，其余鼠标事件（点击/移动）仍丢
                        if let Some(ui_ev) = ui_event_from_crossterm(ev, &mut accel)
                            && thread_key_tx.send(ui_ev).is_err()
                        {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            Ok(false) => {}
            Err(_) => break,
        }
    });
    }

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

    // report 供 detached 命令线程回报失败; keys 为宿主(嵌入)键注入入口。
    EventSources {
        events: rx,
        report: tx.clone(),
        keys: key_tx,
    }
}

/// 把 crossterm 原始事件翻译成 [`UiEvent`]；不关心的事件返回 `None`。
///
/// 输入: ev — 键盘线程 `poll`/`read` 得到的 crossterm 事件；
///       accel — 滚轮加速器（状态随事件源线程存活，读取点打时戳）
/// 输出: Key → `UiEvent::Key`；Resize → `UiEvent::Resize`（触发重绘，
///       autoresize 在 draw 时取新尺寸）；滚轮 → `UiEvent::Wheel(行数)`
///       （负=上滚、正=下滚，已含加速与 Alt 倍率）；点击/移动/拖拽/
///       横向滚轮 → `None`（点击转发属 add-tool-card-click，design D4/D7）
fn ui_event_from_crossterm(ev: ct_event::Event, accel: &mut WheelAccel) -> Option<UiEvent> {
    match ev {
        ct_event::Event::Key(key) => Some(UiEvent::Key(key)),
        ct_event::Event::Resize(..) => Some(UiEvent::Resize),
        ct_event::Event::Mouse(m) => {
            let dir = match m.kind {
                ct_event::MouseEventKind::ScrollUp => -1i8,
                ct_event::MouseEventKind::ScrollDown => 1i8,
                _ => return None,
            };
            let now = accel.now_ms();
            let mut lines = accel.next(dir, now);
            if m.modifiers.contains(KeyModifiers::ALT) {
                lines *= ALT_WHEEL_MULTIPLIER;
            }
            Some(UiEvent::Wheel(dir as i16 * lines as i16))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    /// 键盘事件原样透传。
    /// 输入: crossterm Key 事件；输出: 等价的 UiEvent::Key
    #[test]
    fn key_event_passes_through() {
        let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        assert!(matches!(
            ui_event_from_crossterm(
                ct_event::Event::Key(key),
                &mut WheelAccel::with_accelerate(false)
            ),
            Some(UiEvent::Key(_))
        ));
    }

    /// 终端 resize 必须转成 UiEvent::Resize（否则改尺寸后不重绘）。
    /// 输入: crossterm Resize 事件；输出: UiEvent::Resize
    #[test]
    fn resize_event_becomes_ui_resize() {
        assert!(matches!(
            ui_event_from_crossterm(
                ct_event::Event::Resize(120, 40),
                &mut WheelAccel::with_accelerate(false)
            ),
            Some(UiEvent::Resize)
        ));
    }

    /// 任务 2.2：滚轮翻译为有符号行数（上滚负、下滚正）。
    /// 输入: ScrollUp/ScrollDown；输出: UiEvent::Wheel(∓1)（不加速=1 行）
    #[test]
    fn wheel_scroll_translates_to_signed_lines() {
        use crossterm::event::{MouseEvent, MouseEventKind};
        let mk = |kind| MouseEvent {
            kind,
            column: 5,
            row: 3,
            modifiers: KeyModifiers::empty(),
        };
        let mut accel = WheelAccel::with_accelerate(false);
        assert!(
            matches!(
                ui_event_from_crossterm(
                    ct_event::Event::Mouse(mk(MouseEventKind::ScrollUp)),
                    &mut accel
                ),
                Some(UiEvent::Wheel(n)) if n == -1
            ),
            "上滚 = 负行数"
        );
        assert!(
            matches!(
                ui_event_from_crossterm(
                    ct_event::Event::Mouse(mk(MouseEventKind::ScrollDown)),
                    &mut accel
                ),
                Some(UiEvent::Wheel(n)) if n == 1
            ),
            "下滚 = 正行数"
        );
    }

    /// 任务 2.2：Alt+滚轮 = 5 倍行数（ALT_WHEEL_MULTIPLIER，design D5）。
    /// 输入: ScrollDown + ALT；输出: UiEvent::Wheel(5)（不加速 1×5）
    #[test]
    fn alt_wheel_multiplies_five() {
        use crossterm::event::{MouseEvent, MouseEventKind};
        let mouse = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 5,
            row: 3,
            modifiers: KeyModifiers::ALT,
        };
        assert!(matches!(
            ui_event_from_crossterm(
                ct_event::Event::Mouse(mouse),
                &mut WheelAccel::with_accelerate(false)
            ),
            Some(UiEvent::Wheel(n)) if n == ALT_WHEEL_MULTIPLIER as i16
        ));
    }

    /// 任务 2.2：点击/移动/拖拽/横向滚轮仍丢弃——边界钉给
    /// add-tool-card-click（其 change 才开 Down 转发，design D4/D7）。
    /// 输入: 各类非滚轮鼠标事件；输出: None
    #[test]
    fn click_and_motion_still_dropped_for_add_tool_card_click() {
        use crossterm::event::{MouseEvent, MouseEventKind, MouseButton};
        let mk = |kind| MouseEvent {
            kind,
            column: 5,
            row: 3,
            modifiers: KeyModifiers::empty(),
        };
        let mut accel = WheelAccel::with_accelerate(false);
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
            MouseEventKind::Moved,
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::ScrollLeft,
            MouseEventKind::ScrollRight,
        ] {
            assert!(
                ui_event_from_crossterm(ct_event::Event::Mouse(mk(kind)), &mut accel).is_none(),
                "{kind:?} 应仍被丢弃"
            );
        }
    }
}
