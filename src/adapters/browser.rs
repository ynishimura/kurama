//! Open a URL in the person's browser: `open` on macOS, `xdg-open`
//! elsewhere. Runtime scenarios put a fake `open` on `PATH`.

use std::process::{Command, ExitStatus, Stdio};

/// Why the URL opener did not open the URL.
#[derive(Debug, thiserror::Error)]
pub enum BrowserError {
    #[error("could not run `{program}`")]
    Spawn {
        program: &'static str,
        #[source]
        source: std::io::Error,
    },

    #[error("`{program}` exited with {status}")]
    Exited {
        program: &'static str,
        status: ExitStatus,
    },
}

/// Hand `url` to the opener and wait for it: the opener returns once the
/// browser has the URL, and its exit status is the only word on whether it
/// did.
pub fn open_url(url: &str) -> Result<(), BrowserError> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let status = Command::new(program)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|source| BrowserError::Spawn { program, source })?;
    if status.success() {
        Ok(())
    } else {
        Err(BrowserError::Exited { program, status })
    }
}
