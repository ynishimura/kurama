//! Pure domain functions
//!
//! This module contains pure functions for domain operations.
//! All functions are:
//! - Pure (no side effects)
//! - Deterministic (same input → same output)
//! - Referentially transparent
//!
//! ## Modules
//!
//! - `assume_role` - AssumeRole request building
//! - `federation` - AWS Console federation URL building
//! - `session` - Session name generation
//! - `export` - Environment export script generation
//! - `validation` - Validation helper functions
//! - `onepassword` - 1Password data processing
//! - `profile_status` - Session state and activity of a profile for `kurama status`
//! - `oauth` - PKCE, token requests and responses, discovery
//! - `auth_status` - Token state of an `[auth.*]` source for `kurama status`
//! - `agent_policy` - the `[agent]` rules a request of a run under an agent is held to
//! - `audit` - the audit log: a URL cut to its path, the SQL fingerprint, `--since` and the listing
//! - `api_body_fit` - the `--shape` / `--sample` cut of a printed `kurama api` body
//! - `api_pages` - the next page of `kurama api --pages` (Link header or cursor)
//! - `api_request` - TARGET parsing, URL resolution and output of `kurama api`
//! - `signing_target` - the SigV4 service and region of an `[api.*]` with `aws_profile`
//! - `openapi` - OpenAPI 3.x / Swagger 2.0 documents normalized into an `ApiSpec`
//! - `discovery` - Google Discovery Documents normalized into an `ApiSpec`
//! - `graphql` - GraphQL introspection results normalized into an `ApiSpec`
//! - `skill_install` - where `agent install` puts each Agent Skill and whether it rewrites one
//! - `graphql_call` - a GraphQL root field, `-P` and `-d` as a query document request; the first error of an answer
//! - `operation_lookup` - find, search and suggest operations of an `ApiSpec`
//! - `operation_request` - an operation plus `-P` parameters and a body as a request
//! - `operation_command` - CLI command rendering for an OpenAPI operation
//! - `spec_output` - the `--ops` and `--describe` output
//! - `api_schema` - the `--schema` contract
//! - `api_skill` - the `--skill` Agent Skill
//! - `mcp` - the messages, tools and command lines of `kurama mcp`
//! - `mcp_http` - what an HTTP request to `kurama mcp --listen` is answered with
//! - `obsidian` - vault paths held to `[obsidian] allow_paths`, the Obsidian CLI argv and its answer shaped

pub mod agent_policy;
pub mod api_body_fit;
pub mod api_pages;
pub mod api_request;
pub mod api_schema;
pub mod api_skill;
pub mod assume_role;
pub mod audit;
pub mod auth_status;
pub mod completion_candidates;
pub mod discovery;
pub mod error_mapping;
pub mod export;
pub mod federation;
pub mod fuzzy_match;
pub mod graphql;
pub mod graphql_call;
pub mod jq_completion;
pub mod mcp;
pub mod mcp_http;
pub mod oauth;
pub mod obsidian;
pub mod onepassword;
pub mod openapi;
mod openapi_schema;
pub mod operation_command;
pub mod operation_lookup;
pub mod operation_request;
pub mod parquet_advice;
pub mod preset_render;
pub mod profile_status;
pub mod s3_handoff;
pub mod s3_scan;
pub mod s3_text;
pub mod session;
pub mod signing_target;
pub mod skill_install;
pub mod source_readiness;
pub mod spec_output;
pub mod validation;

// Re-export commonly used functions
pub use assume_role::{AssumeRoleInput, build_assume_role_params};
pub use session::{SessionNameConfig, validate_session_name_template};

pub mod session_cache;
pub mod totp_window;
