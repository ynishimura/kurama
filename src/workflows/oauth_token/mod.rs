//! OAuth token workflow: a usable token for one `[auth.*]` source.
//!
//! `kurama login`, `kurama token`, `kurama env` / `exec` on an auth source
//! and `kurama api` all run it. A usable stored token ends the workflow at
//! once; an expired one with a refresh token is refreshed; otherwise the
//! source's grant runs, which for `authorization_code` and `device_code`
//! needs a person and therefore a terminal.
//!
//! ```text
//! Start -> LoadToken --usable--> Completed { reused: true }
//!             | expired + refresh token          | none / expired
//!             v                                   v
//!          [Discover] -> [ResolveSecret] -> Refresh --ok--> Completed + StoreToken
//!                                             | rejected (an OAuth error body)
//!                                             v
//!          [Discover] -> [ResolveSecret] -> grant:
//!             authorization_code: Listen -> Authorize -> WaitForCallback -> Exchange
//!             device_code:        RequestDeviceCode -> ShowDeviceCode -> (Sleep -> Poll)*
//!             client_credentials: RequestToken
//!          every grant ends in Completed + StoreToken
//! ```
//!
//! Without a terminal a grant that needs a person fails with
//! `TokenFailure::LoginRequired` before anything is requested. A refresh
//! answered with a bare status outside 2xx (a gateway 503) fails as
//! `Rejected` without a grant: the refresh token is still good.

mod transitions;
mod types;

pub use transitions::step;
pub use types::{
    TokenEffect, TokenEvent, TokenFailure, TokenInput, TokenMode, TokenOutput, TokenState,
};

#[cfg(test)]
mod tests;
