//! HTTP client port: sends one request, returns the response. The OAuth
//! token flows and `kurama api` both go through it, so one fake serves every
//! scenario and one mock every unit test.

use async_trait::async_trait;

pub use crate::domain::types::http::{HttpRequest, HttpResponse};

#[derive(Debug, Clone, thiserror::Error)]
pub enum HttpError {
    #[error("request timed out after {seconds}s")]
    Timeout { seconds: u64 },
    #[error("could not connect: {0}")]
    Connect(String),
    #[error("{0}")]
    Other(String),
}

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait HttpClient: Send + Sync {
    /// Send the request and return whatever status comes back; only
    /// transport failures are errors.
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, HttpError>;
}
