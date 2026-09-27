//! OAuth adapters: the loopback listener of the authorization code grant.
//! Token requests go through the `HttpClient` port and secrets through the
//! `SecretResolver` port.

pub mod loopback;

pub use loopback::CallbackListener;
