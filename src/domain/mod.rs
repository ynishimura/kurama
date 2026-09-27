//! Domain layer
//!
//! This module contains the pure domain layer following functional programming principles:
//!
//! - **types/**: Value objects with validation (invalid states unrepresentable)
//! - **functions/**: Pure functions for domain operations
//! - **constants.rs**: Values that business rules depend on
//!
//! Errors are declared next to the type or function that produces them
//! (for example `SessionDurationError`); the application-level error type
//! lives in `adapters::error`.
//!
//! ## Design Principles
//!
//! 1. **Type Safety**: All value objects are validated at construction
//! 2. **Immutability**: Data structures are immutable by default
//! 3. **Pure Functions**: All functions are pure (no side effects)
//! 4. **Explicit Errors**: Errors are part of the type signature

pub mod constants;
pub mod functions;
pub mod types;

// Re-export types for convenience
pub use types::{Credentials, OutputFormat, Profile};

// Re-export functions
