//! Adapters layer (Imperative Shell)
//!
//! This module provides concrete implementations of ports.
//! All side effects (I/O, network, etc.) are contained here.
//!
//! ## Modules
//!
//! - `aws/` - AWS SDK adapters (STS, Federation)
//! - `auth/` - Authentication adapters (1Password)
//! - `config/` - Configuration loaders
//! - `profile/` - Profile management
//! - `env_script` - Export script hand-off to the shell wrapper
//! - `session_cache/` - MFA session cache (macOS keychain)
//! - `token_store/` - OAuth token store (macOS keychain)
//! - `keychain` - the keychain entry helper both stores share
//! - `http` - the reqwest HTTP client
//! - `oauth/` - the loopback listener of the authorization code grant
//! - `profile_lock` - per-profile lock across processes
//! - `browser` - open a URL
//! - `jq` - jq filters through jaq
//! - `sigv4` - SigV4 request signing on aws-sigv4
//! - `secret_resolver` - op:// and aws-*:// references resolved to their values
//! - `openapi` - OpenAPI documents (JSON / YAML) and their cache
//! - `mcp_http` - the HTTP listener of `kurama mcp --listen`
//! - `own_command` - kurama run again as a child, for `kurama mcp`, and the Obsidian CLI
//! - `obsidian_cli` - the official Obsidian CLI, for `kurama obsidian`
//! - `clipboard` - copy text to the clipboard
//! - `editor` - edit a text in `$EDITOR`
//! - `request_history` - the explorer's request history file

pub mod audit_log;
pub mod auth;
pub mod aws;
pub mod browser;
pub mod clipboard;
pub mod completion;
pub mod completion_protocol;
pub mod config;
pub mod data_inputs;
pub mod database;
pub mod duckdb;
pub mod editor;
pub mod env_script;
pub mod error;
pub mod http;
pub mod jq;
pub mod keychain;
pub mod mcp_http;
pub mod oauth;
pub mod obsidian_cli;
pub mod openapi;
pub mod own_command;
pub mod profile;
pub mod profile_lock;
pub mod request_history;
pub mod secret_resolver;
pub mod sigv4;
pub mod token_store;
pub mod utils;

// Re-export commonly used types

pub mod session_cache;
