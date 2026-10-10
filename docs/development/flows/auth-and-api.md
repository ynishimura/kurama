# OAuth and API flow

How a PROFILE resolves to an AWS profile or an `[auth.*]` source and how `kurama api` sends the credential.

OAuth flow: `src/shell/cli/commands/source.rs` resolves a `<PROFILE>` to an
AWS profile or an `[auth.*]` source (one namespace; a shared name is
`CONFIG_INVALID`). `ApiRuntime` (`src/shell/api_runtime.rs`) runs
`src/workflows/oauth_token` through `src/shell/oauth_executor.rs` for
`login` / `token` / `env` / `exec` on an `oauth` source and for `api`;
`ApiRuntime::call` adds the bearer token and retries once after a 401. A
`kind = "token"` source (`src/domain/types/token_source.rs`) has no grant
and no store: `ApiRuntime::ensure_credential` reads its `token` reference,
`with_credential_header` puts the value in the `header` the source names
through its `format`, and the request is sent once -- a retry would read
the same value again. `AuthSource` (`src/domain/types/auth_source.rs`) is
the enum every kind resolves to, so each verb matches on it once. A
`kind = "secrets"` source (`src/domain/types/secrets_source.rs`) holds
several references for the environment of `env` / `exec` and no request
credential: `AuthSource::request_auth` narrows to `RequestAuth` (the other
two kinds) for `token`, `api` and `ApiRuntime`, and an `[api.*]` that
names one is `CONFIG_INVALID` when the file is read. `call`
instead
signs the request with SigV4 when the `[api.*]` names an `aws_profile`
(service and region from `--service` / `--region`, the profile, the host,
then the AWS profile's region; the role credentials come from the same
AssumeRole executor as `env`). Ports: `HttpClient` (reqwest), `TokenStore`
(keychain service `kurama-token`), `SecretResolver` (every reference form).
The files: `cargo xtask map api-client` and `cargo xtask map oauth`.
