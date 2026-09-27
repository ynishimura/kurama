//! Terminal management for the TUI, and the one decision whether a run is interactive

use crate::adapters::error::CoreError;
use crate::shell::agent_policy::is_agent_run;
use crossterm::{
    execute,
    terminal::{
        Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
        enable_raw_mode,
    },
};
use ratatui::prelude::*;
use std::io::{self, IsTerminal, Stdout};

// Whether a run is interactive is decided here and nowhere else
// (`tests/architecture/`): a stream that is a terminal counts only when the
// run was not declared non-interactive. An agent's run (`KURAMA_AGENT`) is
// declared so, since an agent reads bytes and answers no screen, whatever
// its harness attached: it gets what a pipe gets.

/// Both TUI entry points require interactive streams and cursor addressing.
pub fn supports_tui() -> bool {
    person_at(io::stdin().is_terminal() && io::stdout().is_terminal()) && addresses_the_cursor()
}

/// Whether progress may rewrite a line on stderr in place, and a log line
/// carry ANSI styles.
///
/// A carriage return means nothing in a file and nothing on a dumb terminal,
/// where the intended single line becomes one fragment per update. This is the
/// same rule `supports_tui` applies to the screen, stated once.
pub fn rewrites_stderr_lines() -> bool {
    person_at(io::stderr().is_terminal()) && addresses_the_cursor()
}

/// Whether stdout is read by a person, who gets a JSON body laid out and
/// fewer logs; anyone else gets the bytes a pipe gets.
pub fn stdout_is_interactive() -> bool {
    person_at(io::stdout().is_terminal())
}

/// Whether stderr is read by a person, who can follow a browser that opens.
pub fn stderr_is_interactive() -> bool {
    person_at(io::stderr().is_terminal())
}

/// Whether a person could answer a login prompt, and so whether a JSON run
/// keeps stderr for one: stdin and stderr are terminals. Blind to the
/// declaration on purpose: it changes what a run shows, never how it
/// authenticates.
pub fn can_prompt_a_person() -> bool {
    io::stdin().is_terminal() && stderr_is_a_terminal()
}

/// Whether stderr is a terminal a login prompt would reach; see
/// `can_prompt_a_person`.
pub fn stderr_is_a_terminal() -> bool {
    io::stderr().is_terminal()
}

fn person_at(terminal: bool) -> bool {
    terminal && !is_agent_run()
}

fn addresses_the_cursor() -> bool {
    std::env::var_os("TERM").is_none_or(|term| term != "dumb")
}

/// Terminal wrapper for the TUI
pub struct Terminal {
    terminal: ratatui::Terminal<CrosstermBackend<Stdout>>,
    restored: bool,
    no_color: bool,
}

impl Terminal {
    /// Create a new terminal
    pub fn new() -> Result<Self, CoreError> {
        enable_raw_mode().map_err(|e| CoreError::Other(format!("Terminal I/O error: {}", e)))?;
        // Until the terminal exists its `Drop` cannot run: this undoes the
        // steps taken so far when a later one fails.
        let mut partial = PartialSetup {
            alternate_screen: false,
        };
        let mut stdout = io::stdout();

        injected_failure("alternate_screen")?;
        execute!(stdout, EnterAlternateScreen)
            .map_err(|e| CoreError::Other(format!("Terminal I/O error: {}", e)))?;
        partial.alternate_screen = true;

        injected_failure("terminal")?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = ratatui::Terminal::new(backend)
            .map_err(|e| CoreError::Other(format!("Terminal I/O error: {}", e)))?;
        std::mem::forget(partial);

        Ok(Self {
            terminal,
            restored: false,
            no_color: std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty()),
        })
    }

    /// Draw to the terminal
    pub fn draw<F>(&mut self, f: F) -> Result<(), CoreError>
    where
        F: FnOnce(&mut Frame),
    {
        self.terminal
            .draw(|frame| {
                f(frame);
                if self.no_color {
                    super::theme::remove_colors(frame.buffer_mut());
                }
            })
            .map_err(|e| CoreError::Other(format!("Terminal I/O error: {}", e)))?;
        Ok(())
    }

    /// Give the terminal to another program (`$EDITOR`): leave the
    /// alternate screen and raw mode until `resume`.
    pub fn suspend(&mut self) -> Result<(), CoreError> {
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)
            .map_err(|e| CoreError::Other(format!("Terminal I/O error: {}", e)))?;
        disable_raw_mode().map_err(|e| CoreError::Other(format!("Terminal I/O error: {}", e)))?;
        self.terminal
            .show_cursor()
            .map_err(|e| CoreError::Other(format!("Terminal I/O error: {}", e)))
    }

    /// Take the terminal back after `suspend`; the next draw repaints it all.
    pub fn resume(&mut self) -> Result<(), CoreError> {
        enable_raw_mode().map_err(|e| CoreError::Other(format!("Terminal I/O error: {}", e)))?;
        execute!(
            self.terminal.backend_mut(),
            EnterAlternateScreen,
            Clear(ClearType::All)
        )
        .map_err(|e| CoreError::Other(format!("Terminal I/O error: {}", e)))?;
        // Both buffers blank: the next draw diffs against an empty screen
        // and repaints everything. (`Terminal::clear` would ask the terminal
        // for the cursor position and wait for an answer.)
        self.terminal.swap_buffers();
        self.terminal.swap_buffers();
        Ok(())
    }

    /// Restore the terminal to its original state
    pub fn restore(&mut self) -> Result<(), CoreError> {
        if self.restored {
            return Ok(());
        }

        // Leave alternate screen
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)
            .map_err(|e| CoreError::Other(format!("Terminal I/O error: {}", e)))?;

        // Disable raw mode
        disable_raw_mode().map_err(|e| CoreError::Other(format!("Terminal I/O error: {}", e)))?;

        // Show cursor
        self.terminal
            .show_cursor()
            .map_err(|e| CoreError::Other(format!("Terminal I/O error: {}", e)))?;

        self.restored = true;

        Ok(())
    }
}

/// Raw mode, and the alternate screen once entered, of a `Terminal::new`
/// that has not finished; dropped on its failure, forgotten on its success.
struct PartialSetup {
    alternate_screen: bool,
}

impl Drop for PartialSetup {
    fn drop(&mut self) {
        if self.alternate_screen {
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
        }
        let _ = disable_raw_mode();
    }
}

/// A scenario makes the initialization step `KURAMA_TEST_TERMINAL_FAILS_AT`
/// names fail, since a real terminal cannot be made to.
#[cfg(feature = "test-fakes")]
fn injected_failure(step: &str) -> Result<(), CoreError> {
    if std::env::var("KURAMA_TEST_TERMINAL_FAILS_AT").is_ok_and(|failing| failing == step) {
        return Err(CoreError::Other(format!(
            "Terminal I/O error: injected failure at {step}"
        )));
    }
    Ok(())
}

#[cfg(not(feature = "test-fakes"))]
fn injected_failure(_step: &str) -> Result<(), CoreError> {
    Ok(())
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // Best effort to restore terminal
        let _ = self.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::utils::test_env;
    use crate::shell::agent_policy::AGENT_ENV;

    #[test]
    #[serial_test::serial]
    fn an_agent_run_is_no_person_whatever_its_streams_are() {
        let before = std::env::var_os(AGENT_ENV);
        for (value, person) in [
            (None, true),
            (Some("0"), true),
            (Some(""), true),
            (Some("1"), false),
        ] {
            test_env::set_or_remove(AGENT_ENV, value);
            assert_eq!(person_at(true), person, "KURAMA_AGENT={value:?}");
            assert!(
                !person_at(false),
                "a pipe is never a person, KURAMA_AGENT={value:?}"
            );
        }
        test_env::set_or_remove(AGENT_ENV, before);
    }
}
