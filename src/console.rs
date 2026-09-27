//! stderr for a person: progress lines (`# ...`) and log output.
//!
//! While the TUI owns the terminal (`hold`), both are kept in memory and
//! written to stderr once the terminal is restored, so nothing is printed
//! over the alternate screen. Everywhere else they go to stderr at once.

use std::io::Write;
use std::sync::Mutex;

use tracing_subscriber::fmt::MakeWriter;

static HELD: Mutex<Option<Vec<u8>>> = Mutex::new(None);
static SILENT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn silence() -> Silence {
    SILENT.store(true, std::sync::atomic::Ordering::SeqCst);
    Silence
}
pub struct Silence;
impl Drop for Silence {
    fn drop(&mut self) {
        SILENT.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// A progress line for the person running kurama, like `eprintln!`.
macro_rules! progress {
    ($($arg:tt)*) => {
        $crate::console::write_line(&format!($($arg)*))
    };
}
pub(crate) use progress;

pub fn write_line(line: &str) {
    write(format!("{line}\n").as_bytes());
}

fn write(bytes: &[u8]) {
    if SILENT.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let mut held = HELD.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    match held.as_mut() {
        Some(buffer) => buffer.extend_from_slice(bytes),
        None => {
            let _ = std::io::stderr().write_all(bytes);
        }
    }
}

/// One progress line rewritten in place, for a person watching something slow.
///
/// Nothing is written until `update` is called, and dropping the line ends it,
/// so a result or an `error[...]` line never lands on top of it. Only a
/// terminal is rewritten this way: the carriage return says nothing in a file,
/// which is why the caller checks before it ticks.
pub struct ProgressLine {
    written: bool,
}

impl ProgressLine {
    pub fn new() -> Self {
        Self { written: false }
    }

    pub fn update(&mut self, line: &str) {
        self.written = true;
        write(format!("\r{line}").as_bytes());
    }
}

impl Drop for ProgressLine {
    fn drop(&mut self) {
        if self.written {
            write(b"\n");
        }
    }
}

/// Hold every progress line and log record until the guard is dropped.
pub fn hold() -> Hold {
    *HELD.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Vec::new());
    Hold
}

/// Releases the held output to stderr when dropped.
pub struct Hold;

impl Drop for Hold {
    fn drop(&mut self) {
        let bytes = HELD
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(bytes) = bytes {
            let _ = std::io::stderr().write_all(&bytes);
        }
    }
}

/// Capture JSON-mode silence when tracing starts. SDK cleanup can log after
/// the command scope ends, so each writer retains that output policy.
#[derive(Clone, Copy)]
pub struct LogWriter {
    silent: bool,
}

impl LogWriter {
    pub fn new() -> Self {
        Self {
            silent: SILENT.load(std::sync::atomic::Ordering::SeqCst),
        }
    }
}

impl<'a> MakeWriter<'a> for LogWriter {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        *self
    }
}

impl Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if !self.silent {
            write(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    /// `hold()` redirects one process-wide buffer, so two tests that use it
    /// would each take the other's output. They take this instead.
    fn serially() -> std::sync::MutexGuard<'static, ()> {
        static CONSOLE: Mutex<()> = Mutex::new(());
        CONSOLE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    use super::*;

    #[test]
    fn held_output_is_buffered_until_the_guard_drops() {
        let _serial = serially();
        let guard = hold();
        progress!("# held {}", 1);
        LogWriter::new().write_all(b"log line\n").unwrap();
        LogWriter { silent: true }
            .make_writer()
            .write_all(b"silenced cleanup\n")
            .unwrap();
        // Tests running in parallel may add lines of their own meanwhile.
        let buffered = String::from_utf8(HELD.lock().unwrap().clone().unwrap()).unwrap();
        assert!(buffered.contains("# held 1\n"), "{buffered}");
        assert!(!buffered.contains("silenced cleanup"));
        assert!(buffered.contains("log line\n"), "{buffered}");
        drop(guard);
        assert!(HELD.lock().unwrap().is_none());
    }

    /// One test, because `hold()` redirects a process-wide buffer: two of them
    /// running at once would each take the other's output.
    #[test]
    fn a_progress_line_is_rewritten_in_place_and_ended_only_when_written() {
        let _serial = serially();
        let guard = hold();
        let silent = HELD.lock().unwrap().clone().unwrap().len();
        drop(ProgressLine::new());
        assert_eq!(
            HELD.lock().unwrap().clone().unwrap().len(),
            silent,
            "a line that never updated writes nothing at all"
        );
        let mut line = ProgressLine::new();
        line.update("# scanning 1s");
        line.update("# scanning 2s");
        drop(line);
        let buffered = String::from_utf8(HELD.lock().unwrap().clone().unwrap()).unwrap();
        assert!(
            buffered.contains("\r# scanning 1s\r# scanning 2s\n"),
            "{buffered:?}"
        );
        drop(guard);
    }
}
