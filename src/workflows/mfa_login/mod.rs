//! MFA login workflow: get an MFA session for one device and cache it.
//!
//! `kurama login` runs it. A usable cached session ends the workflow without
//! asking for a code; otherwise it fetches a code, calls GetSessionToken and
//! stores the session. An invalid code waits for the next TOTP window and
//! retries once, like the AssumeRole workflow.
//!
//! ```text
//! Start -> LoadSession --usable--> Completed { reused: true }
//!             | none (or force)
//!             v
//!          GetMfaToken -> GetSessionToken -> StoreSession -> Completed { reused: false }
//! ```

mod transitions;
mod types;

pub use transitions::step;
pub use types::{LoginEffect, LoginEvent, LoginInput, LoginOutput, LoginState};

#[cfg(test)]
use crate::workflows::common::{FailureKind, MfaAttempt};

#[cfg(test)]
mod tests;
