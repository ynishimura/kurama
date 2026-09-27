//! Authentication module
//!
//! This module provides authentication services including 1Password integration.

pub mod credentials_provider;
pub mod onepassword;
pub mod op_cli;
pub mod secret;

pub use credentials_provider::OnePasswordCredentialsProvider;
pub use onepassword::OnePasswordManager;
pub use secret::OnePasswordSecrets;
