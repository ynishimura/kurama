//! # Kurama - AWS Credential Switcher
//!
//! A fast and secure AWS profile switcher with multi-factor authentication support.
//!
//! ## Architecture
//!
//! This crate follows the "Functional Core, Imperative Shell" pattern:
//!
//! - **domain**: Pure types and functions (business logic)
//! - **workflows**: Pure state machines and transitions
//! - **ports**: Trait definitions for side effects
//! - **adapters**: Concrete implementations of ports
//! - **shell**: Application entry points and effect execution
//!
//! ## Features
//!
//! - Fast profile switching
//! - Multi-factor authentication (MFA) support
//! - AWS console federation
//! - 1Password integration
//! - zsh shell integration (`kurama init zsh`)

// Public for the integration tests and the doctests, which makes rustc treat
// every `pub` item in them as used. `--cfg dead_code_audit` compiles them
// crate-private so the dead-code lint sees them too (`cargo xtask check`).
#[cfg(not(dead_code_audit))]
pub mod adapters;
#[cfg(dead_code_audit)]
pub(crate) mod adapters;
#[cfg(not(dead_code_audit))]
pub mod domain;
#[cfg(dead_code_audit)]
pub(crate) mod domain;
#[cfg(not(dead_code_audit))]
pub mod ports;
#[cfg(dead_code_audit)]
pub(crate) mod ports;
#[cfg(not(dead_code_audit))]
pub mod workflows;
#[cfg(dead_code_audit)]
pub(crate) mod workflows;

// The shell is private: only the binary entry points are exported, so rustc's
// dead-code lint applies to everything else in it.
mod console;
mod shell;
pub use adapters::completion_protocol::ZshCompletion;
pub use shell::cli::{
    ClientInvocation, ClientKind, ErrorCode, JsonErrorKind, build_command, classify_invocation, run,
};
