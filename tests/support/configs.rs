//! Configuration fixtures more than one feature's scenarios read.
//! A fixture only one feature uses stays in that feature's file, so
//! this module holds what would otherwise be a cross-file reference.

/// A source that needs a person: authorization code with PKCE.
pub const AUTH_CODE_CONFIG: &str = "\
[auth.github]
kind = \"oauth\"
grant_type = \"authorization_code\"
auth_url = \"{server}/oauth/authorize\"
token_url = \"{server}/oauth/token\"
client_id = \"gh-client\"
scopes = [\"repo\"]
env_var = \"GITHUB_TOKEN\"

[api.github]
base_url = \"{server}/api\"
";
pub const STORED_ACCESS_TOKEN: &str = "stored-access-token";
/// An API without a source: no token, so a scenario is about the request or
/// the output alone.
pub const PUBLIC_API_CONFIG: &str = "\
[api.public]
base_url = \"{server}/api\"
";
pub const COMPLETION_CONFIG: &str = r#"
[auth.agent]
kind = "oauth"
grant_type = "client_credentials"
token_url = "{server}/oauth/token"
client_id = "agent-client"
client_secret = "op://Agent/never-read/secret"
[auth.worker]
kind = "oauth"
grant_type = "client_credentials"
token_url = "{server}/oauth/token"
client_id = "worker-client"
[api.pets]
base_url = "{server}/api"
description = "Pets: dev"
openapi = "{fixtures}/petstore.json"
[api.cached]
base_url = "{server}/api"
description = "Cached pets"
openapi = "{server}/spec/petstore.json"
[api.remote]
base_url = "{server}/api"
openapi = "{server}/spec/not-cached.json"
"#;
