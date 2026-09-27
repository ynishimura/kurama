//! Pure workflow layer
//!
//! This module contains pure state machines for business workflows.
//! All functions return (NextState, Effects) without executing side effects.
//!
//! ## Available Workflows
//!
//! - `assume_role` - Role assumption workflow
//! - `mfa_login` - Get and cache an MFA session (`kurama login`)
//! - `oauth_token` - A usable OAuth token for an `[auth.*]` source

pub mod assume_role;
pub mod common;
pub mod mfa_login;
pub mod oauth_token;
