//! TEA (The Elm Architecture) implementation for TUI
//!
//! This module provides a functional architecture for the TUI:
//! - Message: User inputs and async results
//! - Model: Application state (immutable)
//! - Effect: Side effects as data
//! - update: Pure function (Model, Message) -> (Model, Vec<Effect>)
//! - view: Pure function Model -> View

pub mod effects;
pub mod messages;
pub mod palette;
pub mod palette_view;
pub mod runtime;
pub mod sources;
pub mod update;
pub mod view;

pub use runtime::TeaRuntime;
