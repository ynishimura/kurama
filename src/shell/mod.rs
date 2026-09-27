//! Shell Layer for KURAMA
//!
//! This module provides:
//! - User interfaces (CLI and TUI)
//! - Dependency injection container (Runtime)
//! - Workflow execution engine (Executor)
//!
//! ## Architecture
//!
//! The shell layer is the outermost layer that:
//! - Wires together all dependencies
//! - Executes effects returned by workflows
//! - Handles user interaction
//!
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │                 Shell Layer                  │
//! │  ┌─────────┐  ┌──────────┐  ┌───────────┐  │
//! │  │ Runtime │  │ Executor │  │ CLI / TUI │  │
//! │  └────┬────┘  └────┬─────┘  └─────┬─────┘  │
//! └───────┼────────────┼──────────────┼────────┘
//!         │            │              │
//!         ▼            ▼              ▼
//!    Dependencies   Workflows    User Input
//! ```

pub mod agent_policy;
pub mod api_error;
pub mod api_runtime;
pub mod audit;
pub mod aws_profile_credentials;
pub mod cli;
pub mod db_connection;
pub mod executor;
pub mod mfa_login_executor;
pub mod oauth_executor;
pub mod runtime;
pub mod spec_loader;
pub mod tui;
