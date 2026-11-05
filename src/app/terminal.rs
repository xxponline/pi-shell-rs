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
        if crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false) {
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
