//! The one way `xtask/tests/` starts the xtask binary: inside a scratch
//! directory, with everything that would reach the person's own environment
//! pointed into it.
//!
//! A test that inherits HOME copies the real kurama configuration, one that
//! inherits CARGO_HOME can install into the real `~/.cargo/bin`, and an
//! exported `KURAMA_CONFIG_PATH` or `KURAMA_XTASK_ROOT` would point xtask at
//! the person's files or repository. Every such variable is set or removed
//! here, so no test can forget one (`tests/architecture/` keeps the binary
//! from being started anywhere else).

use std::path::{Path, PathBuf};
use std::process::Command;

/// `tests/fakes/`, put first on PATH so `gh` is the fake.
pub fn fakes() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fakes")
}

/// The xtask binary with `scratch/home` as HOME and `scratch/cargo` as
/// CARGO_HOME, no inherited kurama or xtask override, and the fakes on PATH.
pub fn xtask(scratch: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
    command
        .env("HOME", scratch.join("home"))
        .env("CARGO_HOME", scratch.join("cargo"))
        .env_remove("KURAMA_CONFIG_PATH")
        .env_remove("KURAMA_XTASK_ROOT")
        .env_remove("CARGO_TARGET_DIR")
        .env(
            "PATH",
            format!(
                "{}:{}",
                fakes().display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        );
    command
}
