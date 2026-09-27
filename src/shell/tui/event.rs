//! Event handling for the TUI

use crate::adapters::config::constants;
use crate::adapters::error::CoreError;
use crossterm::event::{self, Event as CrosstermEvent, KeyEvent};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

/// TUI event types
#[derive(Debug, Clone)]
pub enum Event {
    /// Key press event
    Key(KeyEvent),
    /// Terminal resize event
    Resize,
    /// Tick event for periodic updates
    Tick,
}

/// Event handler for the TUI
pub struct EventHandler {
    rx: mpsc::Receiver<Event>,
    _tx: mpsc::Sender<Event>,
    /// While set, the reader thread leaves stdin alone (another program
    /// has the terminal).
    paused: Arc<AtomicBool>,
    /// Set by the reader thread while it honours `paused`.
    idle: Arc<AtomicBool>,
    /// Ends the reader thread once it is paused.
    closed: Arc<AtomicBool>,
    _handler: thread::JoinHandle<()>,
}

impl Default for EventHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl EventHandler {
    /// Create a new event handler
    pub fn new() -> Self {
        let tick_rate = Duration::from_millis(constants::tui::TICK_RATE_MS);
        let (tx, rx) = mpsc::channel();

        let event_tx = tx.clone();
        let paused = Arc::new(AtomicBool::new(false));
        let idle = Arc::new(AtomicBool::new(false));
        let closed = Arc::new(AtomicBool::new(false));
        let pause_flag = Arc::clone(&paused);
        let idle_flag = Arc::clone(&idle);
        let closed_flag = Arc::clone(&closed);
        let handler = thread::spawn(move || {
            let mut last_tick = std::time::Instant::now();

            loop {
                if pause_flag.load(Ordering::SeqCst) {
                    if closed_flag.load(Ordering::SeqCst) {
                        break;
                    }
                    idle_flag.store(true, Ordering::SeqCst);
                    thread::sleep(Duration::from_millis(20));
                    continue;
                }
                idle_flag.store(false, Ordering::SeqCst);
                let timeout = tick_rate
                    .checked_sub(last_tick.elapsed())
                    .unwrap_or_else(|| Duration::from_secs(0));

                if event::poll(timeout).unwrap_or(false)
                    && let Ok(event) = event::read()
                {
                    let event = match event {
                        CrosstermEvent::Key(key) => Event::Key(key),
                        CrosstermEvent::Resize(_, _) => Event::Resize,
                        _ => continue,
                    };
                    if event_tx.send(event).is_err() {
                        break;
                    }
                }

                if last_tick.elapsed() >= tick_rate {
                    if event_tx.send(Event::Tick).is_err() {
                        break;
                    }
                    last_tick = std::time::Instant::now();
                }
            }
        });

        Self {
            rx,
            _tx: tx,
            paused,
            idle,
            closed,
            _handler: handler,
        }
    }

    /// Stop reading stdin until `resume`. Returns once the reader thread is
    /// out of `poll`, which reads and queues a key on its own: a key typed
    /// afterwards belongs to the other program.
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
        while !self.idle.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Stop the reader thread for good, once it is out of `poll`: the next
    /// screen's reader gets every key typed from now on.
    pub fn close(self) {
        self.pause();
        self.closed.store(true, Ordering::SeqCst);
    }

    /// `idle` is cleared here, not only when the thread wakes up, so a
    /// `pause` that follows at once cannot return on the previous one's flag.
    pub fn resume(&self) {
        self.idle.store(false, Ordering::SeqCst);
        self.paused.store(false, Ordering::SeqCst);
    }

    /// Drop the events queued so far: keys typed while an effect ran
    /// belong to the screen that was showing, not to the one that follows.
    pub fn drain(&self) {
        while self.rx.try_recv().is_ok() {}
    }

    /// A sender for work done off the event loop: a `Tick` sent through it
    /// wakes the loop at once instead of at the next tick.
    pub fn waker(&self) -> mpsc::Sender<Event> {
        self._tx.clone()
    }

    /// Get the next event
    pub async fn next(&mut self) -> Result<Option<Event>, CoreError> {
        match self.rx.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err(CoreError::Internal(
                "Event handler disconnected".to_string(),
            )),
        }
    }
}
