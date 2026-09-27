//! AssumeRole workflow
//!
//! Pure state machine for AWS role assumption.
//!
//! ## Architecture
//!
//! This workflow follows the "Effects as Data" pattern:
//! - State transitions are pure functions
//! - Side effects are returned as Effect types
//! - The shell layer executes effects
//!
//! ```text
//! Input → step() → (NextState, Vec<Effect>)
//!                          ↓
//!                    Shell executes effects
//!                          ↓
//!                    Event from result
//!                          ↓
//!                    step() → ...
//! ```

mod transitions;
mod types;

pub use transitions::step;
pub use types::{
    AssumeRoleEffect, AssumeRoleEvent, AssumeRoleInput, AssumeRoleOutput, AssumeRoleState,
    SessionCacheSettings,
};
