//! RAII terminal lifecycle: raw mode + alternate screen, restored on drop or
//! panic so the user is never left in a broken terminal.
//!
//! Sequence: `enter()` enables raw mode and installs the panic hook up front
//! (so early failures print cleanly); the alternate screen is only entered by
//! `ratatui::init()` inside `take_terminal()`; `restore_now()` unwinds
//! everything exactly once.

use std::io;
use std::panic;
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::event::{KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
use crossterm::terminal::disable_raw_mode;
use ratatui::DefaultTerminal;

/// kitty 键盘增强是否已推送（restore 时弹出；静态供 panic hook 使用）。
static KB_PUSHED: AtomicBool = AtomicBool::new(false);

/// 键盘线程暂停闸门（外部编辑器子进程占用 stdin 期间为 true——不进入
/// crossterm poll、不读字节，design D3）。
static KB_PAUSED: AtomicBool = AtomicBool::new(false);

/// 启动时探测到的 kitty 键盘增强能力（resume 复用，**不再查询**——恢复期的
/// 光标位置查询会与键盘线程争 crossterm 全局读锁，曾致死锁）。
static KB_SUPPORTED: AtomicBool = AtomicBool::new(false);

/// 设置键盘线程暂停闸门。
///
/// 输入: paused — true=暂停投递（子进程持有 stdin）
/// 输出: 无返回值
pub fn set_keyboard_paused(paused: bool) {
    KB_PAUSED.store(paused, Ordering::SeqCst);
}

/// 键盘线程当前是否暂停。
///
/// 输入: 无
/// 输出: true = 暂停中
pub fn keyboard_paused() -> bool {
    KB_PAUSED.load(Ordering::SeqCst)
}

/// 挂起 TUI 交给子进程（外部编辑器）：弹 kitty 增强 flags、退 alt-screen、
/// 关 raw——子进程拿到的是正常终端。
///
/// 输入: 无
/// 输出: 无返回值；副作用为终端模式切换（KB_PAUSED 由调用方另行置位）
pub fn suspend_for_child() {
    if KB_PUSHED.swap(false, Ordering::SeqCst) {
        let _ = crossterm::execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = crossterm::execute!(io::stdout(), crossterm::terminal::LeaveAlternateScreen);
    let _ = disable_raw_mode();
}

/// 子进程结束后的恢复：进 alt-screen、开 raw、重探测并推送 kitty 增强、
/// 隐藏游标（下一帧 draw 会按需再定位）。
///
/// 输入: 无
/// 输出: 无返回值；副作用为终端模式恢复
pub fn resume_from_child() {
    let _ = crossterm::execute!(io::stdout(), crossterm::terminal::EnterAlternateScreen);
    let _ = crossterm::terminal::enable_raw_mode();
    // 复用启动时的能力探测结论，不在此处查询（查询要读光标位置应答，
    // 会与键盘线程的 crossterm 全局读锁互斥 → 死锁，见 KB_SUPPORTED 注释）
    if KB_SUPPORTED.load(Ordering::SeqCst) {
        let _ = crossterm::execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
        KB_PUSHED.store(true, Ordering::SeqCst);
    }
    let _ = crossterm::execute!(io::stdout(), crossterm::cursor::Hide);
}

/// 嵌入模式的终端副作用出口（窗口标题、剪贴板）。回调可选；缺省时
/// standalone 路径落回真实终端（SetTitle / OSC52）。宿主（GUI）在嵌入
/// 时注入回调，把副作用接到自己的窗口标题栏/系统剪贴板上。
#[derive(Default)]
pub struct TerminalSinks {
    /// 窗口标题回调（`setTitle` 扩展方法）。
    pub set_title: Option<Box<dyn Fn(String) + Send>>,
    /// 剪贴板文本回调（`/copy` 等；替代 OSC52 写 stdout）。
    pub copy_text: Option<Box<dyn Fn(String) + Send>>,
}

pub struct TerminalGuard {
    raw_enabled: bool,
}

impl TerminalGuard {
    /// 进入 raw mode，并在终端支持时开启 kitty 键盘增强协议。
    ///
    /// 背景：`shift+enter`（13;2u）与 `ctrl+-`（45;5u，undo 键）等修饰键
    /// 只有在增强协议下才会以正确键码上报；裸字节会被解析成别的键
    /// （如 0x1f → ctrl+7）。不支持的终端忽略推送，键位优雅降级。
    ///
    /// 输入: 无
    /// 输出: Ok(TerminalGuard)；启用 raw mode 失败时 Err
    pub fn enter() -> io::Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        // 查询需在 raw mode 下、事件线程启动前进行（内部会读 stdin 应答）。
        let supported = crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false);
        KB_SUPPORTED.store(supported, Ordering::SeqCst);
        if supported {
            let _ = crossterm::execute!(
                io::stdout(),
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            );
            KB_PUSHED.store(true, Ordering::SeqCst);
        }
        install_panic_hook();
        Ok(Self {
            raw_enabled: true,
        })
    }

    /// 嵌入模式入口：宿主进程不提供 tty（如 GUI 内嵌），跳过 raw-mode /
    /// kitty 键盘增强推送 / alt-screen，不安装 panic hook（由宿主接管）。
    ///
    /// 输入: 无
    /// 输出: Ok(TerminalGuard)（`raw_enabled=false`，Drop/restore 均为无操作）
    pub fn enter_embedded() -> io::Result<Self> {
        Ok(Self { raw_enabled: false })
    }

    /// Hand ownership of the screen to ratatui (enters alternate screen).
    pub fn take_terminal(&mut self) -> DefaultTerminal {
        ratatui::init()
    }

    /// 恢复终端到正常状态（幂等）：退出 alt screen、退出 raw mode、
    /// 若推送过 kitty 键盘增强则弹出。
    pub fn restore_now() {
        // ratatui::restore() leaves the alternate screen and disables raw mode.
        ratatui::restore();
        if KB_PUSHED.swap(false, Ordering::SeqCst) {
            let _ = crossterm::execute!(io::stdout(), PopKeyboardEnhancementFlags);
        }
    }

    /// Normal-end restore: unwinds and disarms so `Drop` won't repeat it.
    pub fn finish(&mut self) {
        Self::restore_now();
        self.raw_enabled = false;
    }

    /// Restore after an early failure, before `take_terminal` was called:
    /// only raw mode needs undoing (no alternate screen was entered).
    fn restore_early(&mut self) {
        if self.raw_enabled {
            let _ = disable_raw_mode();
            self.raw_enabled = false;
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.raw_enabled {
            Self::restore_now();
        }
    }
}

impl crate::app::App {
    /// Early-failure exit path: unwind raw mode, print, exit non-zero.
    pub(crate) fn fail_early(guard: &mut TerminalGuard, msg: &str, hint: &str) -> ! {
        guard.restore_early();
        eprintln!("{msg}");
        eprintln!("{hint}");
        std::process::exit(1);
    }
}

fn install_panic_hook() {
    let default = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        TerminalGuard::restore_now();
        default(info);
    }));
}

#[cfg(test)]
mod gate_tests {
    use super::*;

    #[test]
    fn keyboard_pause_gate_roundtrip() {
        // 闸门默认关闭（正常键路）
        set_keyboard_paused(false);
        assert!(!keyboard_paused());
        // 子进程期间置位：键盘线程据此跳过 read（字节留给子进程）
        set_keyboard_paused(true);
        assert!(keyboard_paused());
        // 恢复
        set_keyboard_paused(false);
        assert!(!keyboard_paused());
        // suspend/resume 不改闸门语义（由调用方管理）
        assert!(!keyboard_paused());
    }
}

#[cfg(test)]
mod sink_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn embedded_guard_skips_tty() {
        // 嵌入模式不触碰真实终端；drop/restore 均应为无操作
        let g = TerminalGuard::enter_embedded().unwrap();
        drop(g);
    }

    #[test]
    fn title_sink_replaces_stdout() {
        let mut app = crate::app::App::new();
        let got = Arc::new(Mutex::new(String::new()));
        let g = got.clone();
        app.set_terminal_sinks(TerminalSinks {
            set_title: Some(Box::new(move |t| *g.lock().unwrap() = t)),
            ..Default::default()
        });
        app.emit_title("hi".into());
        assert_eq!(*got.lock().unwrap(), "hi");
    }

    #[test]
    fn clipboard_sink_replaces_osc52() {
        let mut app = crate::app::App::new();
        let got = Arc::new(Mutex::new(String::new()));
        let g = got.clone();
        app.set_terminal_sinks(TerminalSinks {
            copy_text: Some(Box::new(move |t| *g.lock().unwrap() = t)),
            ..Default::default()
        });
        app.emit_clipboard("abc");
        assert_eq!(*got.lock().unwrap(), "abc");
    }
}
