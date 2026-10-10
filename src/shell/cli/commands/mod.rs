//! CLI command handlers.
//!
//! `profile` (`env`, `exec`, `console`) and `unset` follow the "Effects as
//! Data" pattern: a pure `plan_*_command` produces an effect list that
//! `super::executor` runs. `login` runs the `mfa_login` or the `oauth_token`
//! workflow; `token`, `api` and `env` / `exec` on an `[auth.*]` source run
//! the `oauth_token` workflow through `ApiRuntime`; `status` and `logout`
//! read the profiles, the session cache and the token store directly.
//! `source` resolves a `<PROFILE>` to an AWS profile or an `[auth.*]` source.

pub mod agent;
pub mod agent_catalog;
pub(crate) mod agent_install;
pub(crate) mod agent_ready;
pub mod api;
pub(crate) mod api_command;
pub(crate) mod api_pages;
pub(crate) mod api_plan;
pub mod api_spec;
pub mod audit;
pub mod config;
mod config_add;
pub mod config_check;
mod config_edit;
mod config_show;
pub mod data;
pub mod data_command;
pub mod data_contract;
mod data_plan;
mod data_render;
pub mod db;
pub mod db_command;
pub mod db_contract;
mod db_render;
pub mod init;
pub mod inventory;
pub mod login;
pub mod logout;
pub mod mcp;
pub mod obsidian;
pub mod preset;
pub mod preset_setup;
pub mod profile;
pub mod s3;
pub mod s3_command;
pub mod s3_contract;
pub mod s3_read;
pub mod s3_status;
pub mod source;
pub mod status;
pub mod token;
pub mod unset;

pub use init::{zsh_completions, zsh_init_script};
pub use unset::handle_unset_command;
