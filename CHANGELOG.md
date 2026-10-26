# Changelog

User-visible changes, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/); before 1.0, a minor version may
change the command line or the configuration.

Add an entry under `Unreleased` with every change a user can see: a command,
an option, a configuration key, an error code, an exit code or an output
format. Internal changes need none.

## Unreleased

### Changed

- ctutils 0.4.5 (RUSTSEC-2026-0385: non-constant-time comparisons), used by
  the HMAC behind SigV4 signing.
- The AWS SDK is built without SigV4a, which kurama never used: that drops
  crypto-bigint 0.5 (RUSTSEC-2026-0387, timing variability) and 17 other
  elliptic-curve crates.
- tokio 1.54.1 (RUSTSEC-2026-0386: `AsyncFd` constructors were unsound).

## 0.2.0 - 2026-10-22

### Added

- `[auth.<name>] kind = "secrets"`: a table `env` of variable name to secret
  reference that `kurama exec` reads into the environment of one command.
  `unset` clears every variable, and `status` lists the variable names only.
  `token`, `console`, `--readonly` and `env --json` are `KIND_UNSUPPORTED`
  for it; an `[api.*]` naming it is `CONFIG_INVALID`.
- `kurama preset setup <ID>` takes an API from nothing to a first answer:
  configure, check, credential, agent Skill and a first GET, each step
  reported as `done`, `planned`, `needs_action`, `failed` or `skipped` with
  the next command. A rerun resumes after the step that stopped.
- An AWS profile without `role_arn` is an IAM user profile: `exec` and `env`
  hand over its keys, or a GetSessionToken session when it has
  `mfa_serial` (cached like the role path). `status --json` has
  `"auth": "role" | "iam_user"`. `--readonly` on it is `PROFILE_INVALID`,
  `console` is `KIND_UNSUPPORTED`, and an agent's `exec` on it is
  `AGENT_POLICY_DENIED` unless `--confirm`.
- `kurama agent` has a recipe for finding why a Lambda function fails.

### Changed

- `kurama agent ready` is now `kurama status --ready` (JSON kind
  `status_ready`); the MCP `ready` tool runs it.
- `kurama completions zsh` is now `kurama init zsh --completion-only`.
- `kurama preset show` is now `preset setup --dry-run` and `preset add` is
  now `preset setup --offline`; `--open` is gone.
- `kurama env <auth source> --json` is `KIND_UNSUPPORTED`: `token --json` is
  the credential document. `env --json` on an AWS profile is unchanged.
- `KURAMA_AUTH_VAR` lists the exported variable names separated by spaces.
- Dependencies with RustSec advisories are updated: rustls 0.23.46,
  rustls-webpki 0.103.16 and webpki-roots / webpki-root-certs 1.0.10 (which
  no longer trust distrusted emSign intermediates), and the DuckDB build's
  zip 9.

### Fixed

- A profile whose name holds a space (`[profile payer369 ct]`) signs for its
  own region instead of failing with `Missing Region`.
- Resolved secrets and issued tokens print as `[REDACTED]` in `Debug` and
  `Display`.

## 0.1.2 - 2026-10-10

### Added

- `kurama mcp --listen` serves the same MCP tools over Streamable HTTP on
  the loopback address `[mcp] listen`, for a cloud agent behind a TLS front.
  `[mcp]` takes `listen`, `token` (a secret reference the client sends as a
  bearer token), `tools`, `call_timeout` and `max_concurrent_calls`; `tools`
  and `call_timeout` apply to stdio too. A server that cannot listen is
  `MCP_LISTEN_FAILED`.
- The MCP tool `call_api` takes `query`, percent-encoded onto the target.
- `kurama obsidian search|read|files [--json]` and the MCP tools
  `obsidian_search`, `obsidian_read` and `obsidian_files` read an Obsidian
  vault through the official Obsidian CLI, inside `[obsidian] allow_paths`
  only, and never write to it. `[obsidian]` takes `vault`, `allow_paths`,
  `cli_path`, `max_read_bytes` and `timeout`. New error codes:
  `OBSIDIAN_PATH_REFUSED` (exit 2), `OBSIDIAN_UNAVAILABLE` (exit 3) and
  `OBSIDIAN_FAILED`.

### Changed

- The `zendesk` preset gets a token through an OAuth client credentials grant
  (`--set client_id=... --set client_secret=... --set subdomain=...`) instead
  of an API token over HTTP Basic: Zendesk stops creating API tokens on
  2026-10-27 and rejects every one after 2027-04-30. A section an earlier
  `preset add` wrote keeps working until then.
- The next steps `preset show` / `preset add` print for a client credentials
  preset no longer ask for `kurama login`: the first `kurama api` call gets
  the token.
- `kurama data` embeds DuckDB 1.5.6 (was 1.5.5), with its matching httpfs
  extension for S3 reads.

## 0.1.1 - 2026-10-01

### Security

- rustls 0.23.45 fixes RUSTSEC-2026-0285 (TLS 1.3 handshake messages were
  accepted across encryption level boundaries), which affected every HTTPS
  connection kurama makes.

### Added

- Each release archive's build provenance ships as
  `kurama-<target>.sigstore.json` next to it, so
  `gh attestation verify <archive> --repo ynishimura/kurama --bundle <file>`
  checks a download without asking GitHub.

### Changed

- `kurama data` embeds DuckDB 1.5.5 (was 1.5.3), with its matching httpfs
  extension for S3 reads.

## 0.1.0 - 2026-09-28

The first public version. What it covers:

- AWS role switching into the current shell (`kurama env`), into one command
  (`kurama exec`) and into the AWS console (`kurama console`), with MFA codes
  from 1Password and MFA sessions cached in the macOS keychain.
- Credential sources for APIs: OAuth 2.0 / OpenID Connect grants
  (authorization code with PKCE, device code, client credentials) and issued
  API keys read from 1Password, AWS Secrets Manager or Parameter Store, sent
  as a bearer token, in a named header, over HTTP Basic or in a query
  parameter.
- `kurama api`: requests with those credentials or a SigV4 signature, `--jq`,
  pagination, a JSON envelope and `--dry-run` plans with every secret masked.
- An OpenAPI explorer: `--ops`, `--describe`, operations by name with `-P`,
  the explorer TUI and a generated Agent Skill per API.
- `kurama data` for local and S3 files through DuckDB, and `kurama db` for
  SQLite, PostgreSQL, MySQL and Aurora DSQL, read-only by default.
- `kurama config` and `kurama preset` to write the configuration, with
  presets for GitHub, Google, Linear, ElevenLabs, OpenAI, Slack, Contentful,
  Fireworks, Jira, Zendesk and Backlog.
- `kurama agent`: the contract a coding agent reads before it uses kurama.
- The TUI is drawn in 24-bit color from a traditional Japanese palette
  (lapis, dayflower blue, pale green-blue) with heavy frames; `NO_COLOR=1`
  removes the colors.
