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
    /// 宿主切换配色（宿主配色注入 design D5）：应用新值、清渲染双缓存、
    /// 置 need_draw 重绘——一帧内全部注入面按新值重发。
    Theme(crate::app::theme::TuiTheme),
    /// 滚轮滚动消息区：有符号行数（负=上滚看更早内容、正=下滚看更新内容）；
    /// 行数已在事件源线程经 WheelAccel 加速并含 Alt 倍率（design D2）。
    Wheel(i16),
    /// 左键点击（0-based 屏幕坐标）——`Down(Left)` 转发。
    Click { x: u16, y: u16 },
    /// 指针移动（0-based 屏幕坐标）——`Moved` 转发，滚动条悬停判定用
    /// （add-message-scrollbar design D4）。
    PointerMove { x: u16, y: u16 },
    /// 左键拖拽移动（0-based 屏幕坐标）——`Drag(Left)` 转发，滚动条
    /// scrub 用（design D4）。
    PointerDrag { x: u16, y: u16 },
    /// 左键抬起——`Up(Left)` 转发，滚动条拖拽结束（design D4；坐标不需，
    /// 与 pi release 同）。
    PointerUp,
    /// A detached command send reported failure.
    SendFailed(String),
    /// `clear_queue` 的返回数据（alt+up 取回排队消息，主线程回填输入框）。
    RestoreQueued(serde_json::Value),
    /// `/settings` Theme 行循环请求（GPUI 专属行经共享 apply_settings 回流；
    /// A3 不落盘——GPUI 泵循环 `ThemeSetting` 并应用，TUI 无此行不可达）。
    ThemeCycle,
    /// edit 执行前预览回填（align-edit-diff-rendering；app 层独立线程读
    /// 文件计算——Ok=行模型（存 `Vec<DiffRow>` 而非文本，避免上下文行以
    /// `-` 开头的再解析歧义）、Err=错误文案；迟到回填按 toolId 幂等丢弃，
    /// 见 `App::apply_edit_preview`）。
    EditPreview {
        tool_id: String,
        result: Result<Vec<crate::app::tools::DiffRow>, String>,
    },
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
/// 输出: Key（仅 Press/Repeat；**Release 丢弃**，见下）→ `UiEvent::Key`；
///       Resize → `UiEvent::Resize`（触发重绘，
///       autoresize 在 draw 时取新尺寸）；滚轮 → `UiEvent::Wheel(行数)`
///       （负=上滚、正=下滚，已含加速与 Alt 倍率）；左键按下 →
///       `UiEvent::Click{x,y}`；左键抬起 → `UiEvent::PointerUp`；移动/
///       左键拖拽 → `UiEvent::PointerMove/PointerDrag{x,y}`（滚动条，
///       design D4）；右键/横向滚轮 → `None`
///
/// Release 丢弃的原因: Windows 控制台会为每次按键同时投递 key-down 与
/// key-up 记录，crossterm 0.29 把 key-up 解析成 `KeyEventKind::Release`
/// （unix tty 只有 Press），不丢就会一次按键触发两次动作（实测回归）。
fn ui_event_from_crossterm(ev: ct_event::Event, accel: &mut WheelAccel) -> Option<UiEvent> {
    match ev {
        ct_event::Event::Key(key) => {
            if key.kind == ct_event::KeyEventKind::Release {
                return None;
            }
            Some(UiEvent::Key(key))
        }
        ct_event::Event::Resize(..) => Some(UiEvent::Resize),
        ct_event::Event::Mouse(m) => {
            // 任务 3.1（add-message-scrollbar design D4）：pointer 三件套
            // 与 Click 转发；右键/横向滚轮仍丢。
            match m.kind {
                ct_event::MouseEventKind::Down(ct_event::MouseButton::Left) => {
                    return Some(UiEvent::Click { x: m.column, y: m.row });
                }
                ct_event::MouseEventKind::Up(ct_event::MouseButton::Left) => {
                    return Some(UiEvent::PointerUp);
                }
                ct_event::MouseEventKind::Moved => {
                    return Some(UiEvent::PointerMove { x: m.column, y: m.row });
                }
                ct_event::MouseEventKind::Drag(ct_event::MouseButton::Left) => {
                    return Some(UiEvent::PointerDrag { x: m.column, y: m.row });
                }
                _ => {}
            }
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

    /// Windows 回归：key-up 解析出的 Release 必须丢弃，否则一次按键触发
    /// 两次（crossterm 0.29 在 Windows 上 key-down/key-up 成对投递）。
    /// 输入: kind=Release / Press 的 Key 事件；输出: None / Some(Key)
    #[test]
    fn key_release_event_is_dropped() {
        use crossterm::event::KeyEventKind;
        let release = KeyEvent::new_with_kind(
            KeyCode::Char('a'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        assert!(
            ui_event_from_crossterm(
                ct_event::Event::Key(release),
                &mut WheelAccel::with_accelerate(false)
            )
            .is_none(),
            "Release 必须丢弃"
        );
        let press = KeyEvent::new_with_kind(
            KeyCode::Char('a'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        );
        assert!(
            matches!(
                ui_event_from_crossterm(
                    ct_event::Event::Key(press),
                    &mut WheelAccel::with_accelerate(false)
                ),
                Some(UiEvent::Key(_))
            ),
            "Press 必须转发"
        );
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

    /// 任务 3.1：pointer 转发边界（add-message-scrollbar design D4）——
    /// 前置 add-tool-card-click 的「仅 Down(Left) 转发」断言在此按计划
    /// 翻转：左键按下/抬起、移动、左键拖拽四类转发；右键/横向滚轮仍丢。
    /// 输入: 各类鼠标事件；输出: 四类 → Some(对应 UiEvent{0-based 坐标})，
    ///       其余 None
    #[test]
    fn pointer_events_forwarded_for_scrollbar_and_click_boundary() {
        use crossterm::event::{MouseEvent, MouseEventKind, MouseButton};
        let mk = |kind| MouseEvent {
            kind,
            column: 7,
            row: 4,
            modifiers: KeyModifiers::empty(),
        };
        let mut accel = WheelAccel::with_accelerate(false);

        // 转发四类（坐标原样、0-based）
        assert!(matches!(
            ui_event_from_crossterm(
                ct_event::Event::Mouse(mk(MouseEventKind::Down(MouseButton::Left))),
                &mut accel
            ),
            Some(UiEvent::Click { x: 7, y: 4 })
        ));
        assert!(matches!(
            ui_event_from_crossterm(
                ct_event::Event::Mouse(mk(MouseEventKind::Up(MouseButton::Left))),
                &mut accel
            ),
            Some(UiEvent::PointerUp)
        ));
        assert!(matches!(
            ui_event_from_crossterm(
                ct_event::Event::Mouse(mk(MouseEventKind::Moved)),
                &mut accel
            ),
            Some(UiEvent::PointerMove { x: 7, y: 4 })
        ));
        assert!(matches!(
            ui_event_from_crossterm(
                ct_event::Event::Mouse(mk(MouseEventKind::Drag(MouseButton::Left))),
                &mut accel
            ),
            Some(UiEvent::PointerDrag { x: 7, y: 4 })
        ));

        // 仍丢：右键按下/抬起/拖拽、横向滚轮
        for kind in [
            MouseEventKind::Down(MouseButton::Right),
            MouseEventKind::Up(MouseButton::Right),
            MouseEventKind::Drag(MouseButton::Right),
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
