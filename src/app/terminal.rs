//! RAII terminal lifecycle: raw mode + alternate screen, restored on drop or
//! panic so the user is never left in a broken terminal.
//!
//! Sequence: `enter()` enables raw mode and installs the panic hook up front
//! (so early failures print cleanly); the alternate screen is only entered by
//! `ratatui::init()` inside `take_terminal()`; `restore_now()` unwinds
//! everything exactly once.

use std::io;
use std::panic;

use crossterm::terminal::disable_raw_mode;
use ratatui::DefaultTerminal;

pub struct TerminalGuard {
    raw_enabled: bool,
}

impl TerminalGuard {
    pub fn enter() -> io::Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        install_panic_hook();
        Ok(Self {
            raw_enabled: true,
        })
    }

    /// Hand ownership of the screen to ratatui (enters alternate screen).
    pub fn take_terminal(&mut self) -> DefaultTerminal {
        ratatui::init()
    }

    /// Restore the terminal to its normal state (idempotent).
    pub fn restore_now() {
        // ratatui::restore() leaves the alternate screen and disables raw mode.
        ratatui::restore();
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
