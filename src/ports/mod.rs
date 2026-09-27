//! Ports (Traits) for side effect abstraction
//!
//! This module defines traits that abstract over side effects.
//! Adapters implement these traits to provide concrete implementations.
//!
//! ## Purpose
//!
//! Ports enable:
//! - Dependency injection for testability
//! - Separation of business logic from I/O
//! - Easy mocking in tests
//!
//! ## Available Ports
//!
//! - `MfaProvider` - MFA token retrieval
//! - `StsOperations` - AWS STS operations
//! - `SessionCache` - cached MFA sessions
//! - `HttpClient` - HTTP requests of the OAuth flows and `kurama api`
//! - `TokenStore` - OAuth tokens per `[auth.*]` source
//! - `SecretResolver` - `op://` client secrets
//! - `AwsProfileCredentials` - the AWS profile credentials `kurama api` signs with

pub mod mfa;
pub mod sts;

pub use mfa::MfaProvider;
pub use sts::{AssumeRoleRequest, StsError, StsOperations};

pub mod session_cache;
pub use session_cache::{KeychainDenied, SessionCache, SessionCacheError};

pub mod aws_credentials;
pub mod http;
pub mod secret;
pub mod token_store;
pub use aws_credentials::AwsProfileCredentials;
pub use http::{HttpClient, HttpError, HttpRequest, HttpResponse};
pub use secret::{SecretError, SecretResolver};
pub use token_store::{TokenStore, TokenStoreError};
