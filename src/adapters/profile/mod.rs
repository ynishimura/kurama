//! Profile management module
//!
//! This module provides unified AWS profile management functionality including:
//! - Profile loading from AWS config files
//! - Pure functions for profile operations
//!
//! # Architecture (Functional Core, Imperative Shell)
//!
//! ```text
//! ┌─────────────────────────────────────────────────┐
//! │           loader.rs (I/O Shell)                 │
//! │  Reads files, delegates to pure functions       │
//! └─────────────────────────────────────────────────┘
//!                      │
//!                      ▼
//! ┌─────────────────────────────────────────────────┐
//! │          parser.rs (Pure Core)                  │
//! │  parse_ini_content, parse_aws_config, etc       │
//! └─────────────────────────────────────────────────┘
//!                      │
//!                      ▼
//! ┌─────────────────────────────────────────────────┐
//! │         manager.rs (Pure Functions)             │
//! │  find_profile, create_default_profile           │
//! └─────────────────────────────────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```ignore
//! use kurama::adapters::profile::{load_profiles, find_profile};
//!
//! // Load profiles (I/O)
//! let profiles = load_profiles().await?;
//!
//! // Pure operations
//! let prod = find_profile(&profiles, "prod");
//! ```

pub mod loader;
pub mod manager;
pub mod parser;

// Re-export I/O functions

// Re-export pure functions from manager
pub use manager::{find_profile, load_profiles};

// Re-export pure parser functions
