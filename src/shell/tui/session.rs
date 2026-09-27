//! Shared terminal session and event loop for TUI screens.

use std::time::Duration;

use crate::adapters::error::CoreError;
use crate::shell::tui::event::{Event, EventHandler};
use crate::shell::tui::terminal::Terminal;

/// A screen that participates in the shared TUI lifecycle.
#[async_trait::async_trait]
pub trait TuiApplication {
    fn should_exit(&self) -> bool;
    fn draw(&self, terminal: &mut Terminal) -> Result<(), CoreError>;
    async fn initialize(
        &mut self,
        terminal: &mut Terminal,
        events: &mut EventHandler,
    ) -> Result<(), CoreError>;
    async fn handle_event(
        &mut self,
        event: Event,
        terminal: &mut Terminal,
        events: &mut EventHandler,
    ) -> Result<(), CoreError>;
}

/// Run the common initial draw, event polling and redraw lifecycle.
pub async fn run_application<A: TuiApplication + ?Sized>(
    app: &mut A,
    terminal: &mut Terminal,
    events: &mut EventHandler,
) -> Result<(), CoreError> {
    app.draw(terminal)?;
    app.initialize(terminal, events).await?;
    app.draw(terminal)?;

    while !app.should_exit() {
        let event = match events.next().await? {
            Some(event) => event,
            None => {
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            }
        };
        app.handle_event(event, terminal, events).await?;
    }

    Ok(())
}

/// Own the alternate-screen terminal, event reader and held console output.
/// The session restores the terminal before releasing output when `run`
/// finishes, including when the screen returns an error.
pub struct TuiSession {
    held_output: Option<crate::console::Hold>,
    terminal: Terminal,
    events: EventHandler,
}

impl TuiSession {
    pub fn new() -> Result<Self, CoreError> {
        let held_output = crate::console::hold();
        let terminal = Terminal::new()?;
        Ok(Self {
            held_output: Some(held_output),
            terminal,
            events: EventHandler::new(),
        })
    }

    /// Give the terminal's input back: after `run`, before another program
    /// or screen reads the keyboard.
    pub fn close(self) {
        self.events.close();
    }

    pub async fn run<A: TuiApplication + ?Sized>(&mut self, app: &mut A) -> Result<(), CoreError> {
        let run_result = run_application(app, &mut self.terminal, &mut self.events).await;
        let restore_result = self.terminal.restore();
        drop(self.held_output.take());
        run_result?;
        restore_result
    }
}
