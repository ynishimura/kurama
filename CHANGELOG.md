# Changelog

User-visible changes, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/); before 1.0, a minor version may
change the command line or the configuration.

Add an entry under `Unreleased` with every change a user can see: a command,
an option, a configuration key, an error code, an exit code or an output
format. Internal changes need none.

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
