//! Domain types (value objects)
//!
//! This module contains all domain value objects.
//! All types are validated at construction time, making invalid states unrepresentable.
//!
//! ## Design Principles
//!
//! - **Type Safety**: Invalid states cannot be constructed
//! - **Immutability**: Value objects are immutable by default
//! - **Pure Functions**: All methods are pure (no side effects)
//! - **Builder Pattern**: Fluent construction via `with_*()` methods

pub mod api_spec;
pub mod audit;
pub mod auth_source;
pub mod credentials;
pub mod database;
mod database_error;
mod database_output;
pub mod dataset;
mod dataset_error;
mod dataset_output;
pub mod http;
pub mod json_shape;
pub mod limits;
pub mod oauth_client;
pub mod oauth_token;
pub mod output_format;
pub mod preset;
pub mod profile;
pub mod request_history;
pub mod s3_browse;
pub mod s3_object;
pub mod secret;
pub mod secret_ref;
pub mod secrets_source;
pub mod session_duration;
pub mod token_source;

// Re-export types
pub use auth_source::{AuthKind, AuthSource, RequestAuth, SourceCredential, check_env_var};
pub use credentials::Credentials;
pub use http::{ApiHeaders, HttpRequest};
pub use oauth_client::{
    DEFAULT_TOKEN_ENV_VAR, EndpointSource, GrantType, OAuthClientConfig, OAuthEndpoints,
};
pub use oauth_token::OAuthToken;
pub use output_format::OutputFormat;
pub use profile::Profile;
pub use secret::Secret;
pub use secret_ref::{AwsSecretRef, AwsSecretStore, SecretFailure, SecretRef};
pub use secrets_source::SecretsSourceConfig;
pub use session_duration::SessionDuration;
pub use token_source::{
    DEFAULT_TOKEN_FORMAT, DEFAULT_TOKEN_HEADER, TokenPlacement, TokenSourceConfig,
};

pub mod cached_session;
pub use cached_session::CachedSession;
