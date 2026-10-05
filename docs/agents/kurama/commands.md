## Commands

| Command | stdout | When |
| --- | --- | --- |
| `kurama status --json` | JSON array | Find names and whether a person must act first |
| `kurama agent install [--dir DIR] [--offline] [--dry-run] --json` | `{dir, dry_run, skills: [{skill, api, path, status, reason}]}`; `status` is `written`, `unchanged` or `skipped` | Install kurama's Agent Skill and one per `[api.*]` with a description (`kurama-api-<name>/SKILL.md`) under `~/.claude/skills`; a file is written only when it differs, nothing else is touched, and `--dry-run` reads descriptions from the cache only and writes nothing |
| `kurama agent ready --json` | `{ready, sources: [{name, kind, state, reason, expires_at, next_actions}]}` | Before a run: which sources work now, and the command a person runs for the rest |
| `kurama config check --json` | one report: the file in use, each `openapi`, every problem | After editing config.toml: every problem at once, without a secret store, a keychain or the network |
| `kurama config add --file - [--dry-run] --json` | `{path, changed, applied, changes, warnings}` | Add new `[auth.*]` / `[api.*]` / ... sections, checked whole before the file is written |
| `kurama config set PATH VALUE --json` / `config set --file - --json` / `config unset KEY... --json` / `config remove SECTION... --json` (each with `--dry-run`) | `{path, changed, applied, changes, warnings}` | Change a key, replace whole sections, remove keys or sections in place; the result is checked whole first |
| `kurama config show [PATH] --json` / `config list [SECTION] --json` / `config path --json` | the saved values (literal secrets redacted), the sections and keys, the file in use | See what config.toml holds without resolving anything |
| `kurama preset --json` / `preset show <ID> --set k=v... --json` | the bundled provider presets; one expanded as `{api, auth, toml, setup, warnings}` | Add GitHub, Google, Linear, ElevenLabs, OpenAI, Slack, Contentful, Fireworks, Jira, Zendesk or Backlog without looking up endpoints, scopes and headers |
| `kurama preset add <ID> --set k=v... [--as NAME] [--auth-as NAME] [--dry-run] --json` | `{path, changed, applied, changes, warnings}` as `config add`, plus `next_steps` | Append a preset's sections to config.toml, reusing a compatible `[auth.*]` |
| `kurama exec <profile> -- <cmd>...` | the command's output | Run tools with the credentials or the token in their environment |
| `kurama env <profile> --json` | credential_process JSON (AWS) or `{access_token, ...}` (auth) | A program needs the credentials as JSON |
| `kurama unset` | `unset` lines for every variable `env` exports | Clear the credentials from a shell you exported them into |
| `kurama api <API> <TARGET> [--json] [--jq F]` | response body, envelope, or jq results | Call an API with the source's bearer token or a SigV4 signature |
| `kurama api <API> --ops [QUERY] --json` | JSON array of operations | Find an operation in the API's OpenAPI description |
| `kurama api <API> --describe <OP> --json` | parameters, request/response shapes, scopes, example command | Learn what an operation takes before calling it |
| `kurama api <API> --schema [OP]` | one versioned JSON contract: options, full parameter/body schemas, limitations | Build calls to an API from its description without calling it |
| `kurama api <API> --skill` | a `SKILL.md` (Agent Skill) for the API, written from the `--schema` contract | Install an API-specific Skill for an agent |
| `kurama api <API> <OP> -P name=value ... [-d BODY]` | response body, envelope, or jq results | Call an operation of the description; kurama places the parameters |
| `kurama data [WORKSPACE_OR_INPUT] --query SQL --json` | one bounded result | Analyze a workspace or local/S3 CSV, JSONL and Parquet input |
| `kurama agent --kind data --json` | capability and request schemas | Discover the data CLI without configuration or service calls |
| `kurama db <DATABASE> --tables --json` | one bounded result | See what a configured database or a SQLite file holds |
| `kurama db <DATABASE> --query SQL --json` | one bounded result | Run one read-only statement against it |
| `kurama agent --kind db --json` | capability and request schemas | Discover the database CLI without configuration or a connection |
| `kurama agent --kind s3 --json` | operations, their flags and default bounds | Discover the S3 CLI without configuration or a request |
| `kurama s3 <S3> [TARGET] --list --json` | one bounded listing with `complete` and a `cursor` | See the prefixes and keys under an S3 prefix |
| `kurama s3 <S3> [TARGET] --search TEXT --json` | the matching keys, `scanned_objects`, `complete` | Find keys by a literal substring |
| `kurama s3 <S3> s3://bucket/key --head --json` | size, type, ETag, storage class | See one object's metadata |
| `kurama s3 <S3> s3://bucket/key --preview --json` | at most `--bytes` of it, `next_offset`, `complete` | Read the start (or the next range) of an object |
| `kurama s3 <S3> [TARGET] --search-content TEXT --json` | matching lines, `skipped`, `complete` | Find text inside the objects under a prefix |
| `kurama token <source> [--json]` | the access token | A tool needs the bearer token itself |
| `kurama token <source> --fingerprint [--json]` | `sha256:<hex>` of the token, or `{fingerprint}` | Check that two sources, or a source before and after a change, hold the same value without printing it |
| `kurama login <profile> [--force]` | nothing | Cache an MFA session or store a token before unattended work; `--force` replaces one that is still valid |
| `kurama logout <profile>` or `--all` | nothing | Drop cached MFA sessions and stored tokens |
| `kurama agent [--skill]` | this page, or the Agent Skill that points at it | Before configuring kurama, or when a command's behavior is unclear |
| `kurama agent --json` | one JSON catalog of every subcommand | Build a command line from the binary's own definition instead of from this page |
| `kurama audit [--since 1h] --json` | `{entries: [{time, command, target, agent, method, path, status, program, sql_sha256, exit_code, error_code, duration_ms}]}` | See the `api`, `exec`, `db` and `data` calls the audit log recorded: a `KURAMA_AGENT` run's by default, every run's with `[audit] enabled = true` |
| `kurama mcp [--listen]` | JSON-RPC on stdout, one message per line; with `--listen`, MCP Streamable HTTP on `[mcp] listen` and nothing on stdout | Register kurama as an MCP server: its tools run `api`, `data`, `db`, `status` and `agent ready` as an agent (see `kurama mcp` below) |

`kurama agent --json` is read off the binary's argument definition, so it
cannot drift from what the parser accepts. Like the rest of `agent` it reads
no configuration and calls nothing. It is one line of JSON:
`schema_version` (1; bumped when a field is removed or renamed), `binary`
(`name`, `version`), `commands` (keyed by subcommand name in `--help` order,
each with `about`, `json_errors` -- whether a failure with `--json` or `--jq`
is one JSON error document on stderr -- `usage_error` -- `{code,
without_json}` when kurama reports a bad argument itself, null when clap's
text with no `error[CODE]` line is all it prints -- `request_contract` --
whether `agent --kind <name> --json` describes its request -- `arguments`,
`groups` and nested `subcommands`) and `exit_codes` (each `exit` with its `meaning`
and every `error[CODE]` that ends with it). An argument carries `id`,
`positional`, `long`, `short`, `value_name`, `takes_value`,
`value_optional` (the value may be left out, as in `api --schema [OP]`),
`required`, `multiple`, `values` (the accepted values when they are a fixed
list), `default`, `value_hint`, `completer` (kurama completes its value),
`help`, `conflicts_with`, `requires` and
`required_unless_present`, all naming other arguments by `id`; a group
without `multiple` takes at most one of its `args`. The same binary prints
the same bytes every time. `agent --json` takes no `--jq`: pipe it to `jq`
(`kurama agent --json | jq '.commands.api.arguments[].long'`).

Prefer `exec` and `api`. The credentials go only into the command's
environment or the request; `env --json` and `token` print them on stdout,
so use those two only to feed a program. `status --json`, `token --fingerprint` and `api ...
--dry-run` check a source without printing a secret (`--dry-run` on an
operation target still fetches the description, with the API's credential
when it sets `openapi_auth`), and a printed token never goes into a report,
a log or a message.
Add `-r` / `--readonly` to `exec` or `env` on an AWS profile to attach the
`ReadOnlyAccess` policy. `exec` replaces kurama with the command, so its exit
code is the command's.

The home screen and API explorer also require `TERM` other than `dumb`;
otherwise they exit 2 with a hint. `status`, `api --ops`, `--describe`, `--schema`, `--skill` and
explicit API targets work without the TUI. A nonempty `NO_COLOR` disables
TUI colors while retaining selection and badge emphasis.

