//! Terminal lifecycle: raw mode, alternate screen, mouse capture.
//!
//! Rust note: ava paired every `Terminal.init` with a manual
//! `defer term.restore()`. Here `Drop` runs the teardown automatically when
//! `Tui` goes out of scope — including on an early `return` or a `?`
//! propagation, which is exactly what `defer` bought, without the discipline.

use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use ratatui::DefaultTerminal;
use std::io::{IsTerminal, stdout};

pub struct Tui {
    terminal: DefaultTerminal,
}

impl Tui {
    /// Enter raw mode + alternate screen (via `ratatui::init`, which also
    /// installs a panic hook that restores the terminal before printing),
    /// then enable mouse capture on top.
    pub fn new() -> std::io::Result<Self> {
        let terminal = ratatui::try_init()?;
        execute!(stdout(), EnableMouseCapture)?;
        Ok(Self { terminal })
    }

    pub fn terminal(&mut self) -> &mut DefaultTerminal {
        &mut self.terminal
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        // Best-effort: if the terminal has gone away there is nothing useful
        // to do with the error, but termios must still be restored.
        let _ = execute!(stdout(), DisableMouseCapture);
        ratatui::restore();
    }
}

/// caleb needs an interactive terminal on both ends.
pub fn is_tty() -> bool {
    std::io::stdin().is_terminal() && stdout().is_terminal()
}

/// NO_COLOR — any non-empty value disables color. See https://no-color.org.
pub fn color_enabled() -> bool {
    match std::env::var("NO_COLOR") {
        Ok(v) => v.is_empty(),
        Err(_) => true,
    }
}
