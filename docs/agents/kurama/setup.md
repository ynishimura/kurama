## Setup: add an `[auth.*]` source or an `[api.*]` profile

`~/.config/kurama/config.toml` holds the `[auth.*]` sources and the
`[api.*]` profiles; `KURAMA_CONFIG_PATH` names another file, which must
exist (`CONFIG_INVALID` otherwise) -- except for `kurama config add` and
`kurama preset add`, which create it. AWS profiles come from `~/.aws/config`
and are not configured here. For GitHub, Google Sheets / Docs / Drive,
Linear, ElevenLabs, OpenAI, Slack, Contentful, Fireworks, Jira, Zendesk and Backlog, `kurama preset add <ID> --set <key>=<value>... --json` writes the
sections (see **Presets** below). Otherwise add sections with
`kurama config add` (see **Adding sections** below), change or remove them
with `kurama config set` / `unset` / `remove` (see **Changing and removing
sections** below), or edit the file directly: every key is checked
when the file is read, an unknown key is refused with its line, every
`CONFIG_INVALID` hint names the file in use, and `kurama config check --json`
is the check (it reads the file and calls nothing; see **Checking the file**
below).

1. Ask the person for what only they know. For an OAuth source
   (`kind = "oauth"`): the provider, the grant type, the client id, the
   scopes, and where the client secret is kept. For an API key that was
   issued elsewhere (`kind = "token"`): where the key is kept, and which
   header the API reads it from. Never write a secret literally:
   `client_secret` takes a reference and `token` takes nothing else.
   Three stores are read (see **Secret references** below); ask the person
   to create the item, secret or parameter if it does not exist yet.
2. Append the sections, with `kurama preset add <ID> --set <key>=<value>...
   --json` when a preset covers the provider (see **Presets**), else with
   `kurama config add --file - --json` (see **Adding sections**). Names are
   one namespace with the AWS profiles: an
   `[auth.<name>]` that is also an AWS profile name is `CONFIG_INVALID`
   when a command resolves it. An `[api.<name>]` without `auth` uses the
   `[auth.<name>]` of the same name and sends no credential when there is
   none; `aws_profile` signs with SigV4 instead and excludes `auth`.

   ```toml
   [auth.<name>]
   kind = "oauth"                       # optional: the default
   grant_type = "authorization_code"   # or device_code, client_credentials
   issuer = "https://accounts.example.com"   # OpenID Connect discovery; or write the URLs: token_url (always), auth_url (authorization_code), device_auth_url (device_code); client_credentials needs token_url alone; https only, http just for localhost / a loopback address
   client_id = "..."
   client_secret = "op://Agent/<item>/client_secret"   # or aws-secrets:// / aws-ssm://; omit for a public client
   scopes = ["openid", "email"]          # plural, an array
   env_var = "EXAMPLE_TOKEN"            # kurama env / exec put the token here; default KURAMA_TOKEN
   # redirect_port = 8080               # authorization_code: a fixed loopback port when the provider needs one

   [auth.example]                       # the other kind, under a name of its own
   kind = "token"                       # an API key issued elsewhere: no grant, nothing cached
   token = "aws-secrets://<aws-profile>/<secret-id>"   # or op:// / aws-ssm://; a literal is CONFIG_INVALID
   header = "X-API-Key"                 # default Authorization
   format = "{token}"                   # default "Bearer {token}"; {token} is the only placeholder
   # username = "me@example.com"        # instead of header/format: HTTP Basic, the credential as the password
   # query = "apiKey"                   # instead of header/format/username: the credential in this query parameter
   env_var = "EXAMPLE_TOKEN"            # kurama env / exec put the credential here; default KURAMA_TOKEN

   [api.<name>]
   description = "..."
   base_url = "https://api.example.com"
   auth = "<name>"                      # or aws_profile = "<AWS profile>" (SigV4)
   # openapi = "https://api.example.com/openapi.json"   # or an absolute file; enables --ops / --describe
   # discovery = "https://sheets.googleapis.com/$discovery/rest?version=v4"  # a Google Discovery Document instead of openapi
   # graphql = "/graphql"               # a GraphQL endpoint instead: its schema by introspection, with the credential
   # graphql_schema = "~/linear.json"   # with graphql: a saved introspection result instead of introspecting
   # openapi_auth = true                # fetch it with the API's credential; same origin as base_url
   # headers = { Accept = "application/vnd.x+json" }  # sent every time; -H replaces one; no credential header
   # service = "execute-api"            # aws_profile only, when the host does not name them
   # region = "ap-northeast-1"
   ```

3. Run `kurama config check --json` and fix every `diagnostics` entry it
   lists, then `kurama status --json`. Exit code 2 is a JSON error document
   on stderr with `code` `CONFIG_INVALID`: its `message` names the key (and
   line) to fix and its `hint` the file in use. On success the new source is
   listed with
   `"token": "missing"` (`"not_checked"` for `kind = "token"`, whose
   credential `status` does not read) and the API with its `auth` or
   `aws_profile`.
4. Get the first token. `kind = "token"` has nothing to get: the credential
   is read from its reference on every use, `kurama login <name>` says so
   and does nothing, and `kurama logout <name>` has nothing to remove. For
   `kind = "oauth"`, `client_credentials` needs nobody: the first
   `kurama api` or `kurama token <name>` fetches it. `authorization_code`
   and `device_code` need a person: ask them to run `kurama login <name>`
   in a terminal (there, `--no-browser` prints the URL instead of opening
   the browser; the login still waits in that terminal for the callback or
   the code). Without a terminal, `login`, `token`, `env`, `exec` and `api`
   on that source exit 3 with that hint and print no URL.
5. Verify. `kurama api <name> /path --dry-run` prints the request without
   sending it; `kurama api <name> /path` sends it. With `openapi`,
   `kurama api <name> --ops` lists the operations (with `openapi_auth`,
   only once the source has a token: the description is fetched with it).

An API signed with `aws_profile` needs no login of its own: it follows the
AWS profile it names (`needs_human` of that profile in `kurama status
--json`).

### An API without a description

When the provider publishes no OpenAPI, Discovery or GraphQL description
(Backlog, Notion, Trello), write the few operations you need as a minimal
OpenAPI file and name it with `openapi = "<absolute path>"`: `openapi` and
`paths` are all it needs. Without `info` the title is `API`; without
`responses` an operation has no response shape; without `operationId` its
id is `METHOD /path`; a `{name}` of the path that no parameter declares is a
required string path parameter. Declare a query parameter (and a type
other than string) under `parameters`. Then `--ops`, `--describe`,
`--schema` and `kurama api <name> <operationId> -P name=value` work as with
a published description. For Backlog, with `base_url =
"https://<space>.backlog.com"` and the API key as a `kind = "token"` source
with `query = "apiKey"`:

```yaml
openapi: 3.1.0
paths:
  /api/v2/users/myself:
    get:
      operationId: users/myself
      summary: The user the API key belongs to
  /api/v2/issues:
    get:
      operationId: issues/list
      parameters:
        - {name: "projectId[]", in: query, schema: {type: integer}}
        - {name: count, in: query, schema: {type: integer}}
  /api/v2/issues/{issueIdOrKey}:
    get:
      operationId: issues/get
```


## Secret references

Anywhere kurama takes a secret -- `[auth.*] client_secret`, `[auth.*] token`,
`[db.*] username` and `[db.*] password` -- the value is one of these, and
never the secret itself:

```
op://<vault>/<item>/<field>
aws-secrets://<aws-profile>/<secret-id>[?region=<region>][#<json-key>]
aws-ssm://<aws-profile>/<parameter-name>[?region=<region>]
```

`op://` is read through the 1Password CLI. The two `aws-` forms are read with
the role of an AWS profile from `~/.aws/config`, through the same AssumeRole
path as `kurama env` (session cache, 1Password TOTP, STS); a tunnel, an IAM
token and every reference that name one profile assume its role once. The
`<secret-id>` is a name or a full ARN and may contain `/`; a parameter name
may leave its leading `/` out. `#<json-key>` takes one top-level string of a
JSON `SecretString`, which is the shape an RDS managed secret has -- reading
two keys of one secret is one `GetSecretValue`. The region is the ARN's, then
`?region=`, then the AWS profile's; an ARN and a `?region=` that disagree are
`CONFIG_INVALID`. `SecureString` and `SecretBinary`: a parameter is always
decrypted, and binary is refused, because bytes are not a password.

A value shaped like `<scheme>://` that is not one of these three is
`CONFIG_INVALID` with its line, so `aws-secret://` (one letter short) never
passes as a literal password. A value that is not shaped like a scheme is the
literal, which `[db.*] password` and `[auth.*] token` refuse and the other
two accept.

The role needs `secretsmanager:GetSecretValue` or `ssm:GetParameter`, plus
`kms:Decrypt` for a customer-managed key or a `SecureString`. A refusal is
`SECRET_REJECTED` (exit 4) and the `hint:` names the action; a reference or a
value that cannot be used is `SECRET_INVALID` (exit 2); a store that could not
be reached is `SECRET_FAILED` (exit 1); 1Password needing a person is
`SECRET_UNAVAILABLE` (exit 3). Getting the credentials is the AssumeRole path
and keeps that path's own codes, so a wrong TOTP or a role without
`sts:AssumeRole` reports `STS_*` and not a reference that is wrong. Each secret
is read once per process whichever store holds it: two fields of one managed
secret are one `GetSecretValue`, and two fields of one 1Password item are one
`op item get` and one biometric prompt (a reference with a section or a
`?attribute=` is its own `op read`).
Nothing is resolved by `status`, `--dry-run` or
shell completion, and a resolved value is never written to a file, a log or an
error message.

### Presets

For GitHub, Google Sheets / Docs / Drive and Linear, kurama carries the
sections itself. `kurama preset --json` lists the presets (`id`, `title`,
`auth` with its `name` and `kind`, `api`, the `inputs` each needs, and the
`setup` page) and reads no configuration.
`kurama preset show <ID> --set <key>=<value>... [--as NAME] [--auth-as NAME] --json` prints
one preset expanded against config.toml as `{id, path, api, auth: {name,
action}, toml, setup, warnings}`; without `--json` stdout is the TOML alone
and the setup steps go to stderr. It writes nothing and resolves nothing.

`kurama preset add <ID> --set <key>=<value>... [--as NAME] [--auth-as NAME] [--dry-run] --json`
appends that same TOML to the file and prints the save result of
`kurama config add` plus the steps left: `{path, changed, applied, changes:
[{section, action}], warnings, next_steps}`, where `action` is `add`, or
`reuse` for the existing `[auth.*]` the new API uses unchanged, and
`next_steps` are the remaining setup steps as text -- adding scopes,
`kurama login`, a first `kurama api` call. Without `--json` stdout is empty
and stderr says what was added, then those steps. `--dry-run` prints the
TOML `preset show` prints and writes nothing (`applied` is false); its steps
start with the `kurama preset add` that would append it. The file's own bytes, comments and
order are kept, the whole result is checked before the one write, and a
save that fails is `CONFIG_WRITE_FAILED` (exit 1) with the path in the hint,
as for `config add`. It resolves nothing: `kurama api <API>` is the first
use of the credential. For example:

```sh
kurama preset add linear --set secret=op://Agent/kurama-linear/credential --json
kurama api linear /graphql -d '{"query":"{ viewer { id name } }"}'
```

Every rule below holds for `show` and `add` alike; a refusal writes nothing.

- Every input is given with `--set`; secret inputs (`secret`,
  `client_secret`) take a reference, and a value that is not one is
  `CONFIG_INVALID` naming the key, before anything else is checked. An
  input the sections need and nobody gave is `CONFIG_INVALID` naming each
  key; an id the catalog lacks is `PRESET_NOT_FOUND` (exit 2).
- An existing `[auth.<name>]` of the auth name (the preset's, or `--auth-as`) is reused
  (`auth.action` is `reuse` and the TOML holds only the `[api.*]`) when its
  kind, grant, issuer or endpoints and token header/format are the preset's
  and a `client_id` or secret reference given with `--set` is the one it
  holds; otherwise it is `CONFIG_INVALID`. Scopes it lacks are a `warnings`
  entry and a setup step before the login: the step and the warning name
  the `kurama config set auth.<auth>.scopes '[...]'` that writes every scope
  the auth holds plus the missing ones, then `kurama login --force <auth>`.
- `--as NAME` renames the `[api.*]` only; the auth keeps the preset's name.
  An `[api.*]` name the file already has is `CONFIG_INVALID` with a hint
  naming `--as`.
- `--auth-as NAME` names the `[auth.*]` the preset writes and the `[api.*]
  auth` reference NAME; the reuse rule above applies to that name, and the
  `env_var` stays the preset's. An existing auth of the preset's name that
  is not compatible is refused with a hint naming `--auth-as` first.
- An existing `[auth.*]` is never changed by a preset: a scope it lacks is
  added with `kurama config set`, and nothing logs in by itself. A preset
  is never updated or removed as a whole; its comment in the file is
  history only, and `kurama config remove api.<name>` removes one API and
  keeps the auth the others share.
- The sections are checked with the file as `kurama config add` checks them:
  an auth named like an AWS profile is refused, the `--auth-as` name
  included. The hint is to name the auth with `--auth-as` or rename the
  profile, never `--as`.

### Adding sections

`kurama config add --file FILE [--dry-run] [--json]` (`FILE` `-` reads
stdin) appends TOML that carries its own section headers -- `[auth.<name>]`,
`[api.<name>]`, `[db.<name>]`, ... -- to the file in use, and can add
several sections that name each other in one run. With `new.toml` holding

```toml
[auth.example]
kind = "token"
token = "op://Agent/example/credential"
header = "X-API-Key"
format = "{token}"

[api.example]
base_url = "https://api.example.com"
auth = "example"
```

`kurama config add --file new.toml --dry-run` prints what it would append,
and `kurama config add --file new.toml --json` appends it.

- The file's own bytes, comments and order are kept; the input goes after
  them. A missing file is created, readable by its owner only.
- A section the file already has refuses the whole input
  (`CONFIG_INVALID`): nothing is merged and no `[auth.*]` is reused. So is
  an `[auth.*]` named like an AWS profile.
- `client_secret`, `token` and `password` take a secret reference only;
  anything else -- a literal string, a number -- is `CONFIG_INVALID` naming
  the key, and its value is not printed back.
- A refusal of the input names the input's own line (`the input line 3:`)
  and its hint says to fix the input; the file was not changed. A mistake in
  the file itself names `config.toml` and its line.
- The file plus the input is checked whole -- keys, types, references
  between sections and to `~/.aws/config` -- before anything is written;
  any failure leaves the file untouched. Without an AWS config the
  `aws_profile` references are not checked, which `warnings` says.
- `--dry-run` prints the lines it would append (`+` on each) and writes
  nothing. `--json` prints `{path, changed, applied, changes: [{section,
  action}], warnings}`; `applied` is false for a dry run.
- A save that fails (a directory that is not writable, the file changed by
  something else after it was read) is `CONFIG_WRITE_FAILED`, exit 1, with
  the path in the hint; the file is not changed.
- A symbolic link at the path stays a link: the file it points to is
  replaced. Nothing is resolved or contacted: a saved section says nothing
  about whether the provider accepts it; `kurama status` and a first call do.

### Changing and removing sections

```text
kurama config set PATH VALUE [--dry-run] [--json]
kurama config set --file FILE [--dry-run] [--json]
kurama config unset KEY... [--dry-run] [--json]
kurama config remove SECTION... [--dry-run] [--json]
```

A section is a fixed table (`core`, `aws`, `onepassword`, `openapi`) or one
named entry (`auth.<name>`, `api.<name>`, `data.<name>`, `db.<name>`,
`s3.<name>`). For example:

```sh
kurama config set core.log_level '"debug"'
kurama config set aws.session_cache.duration 3600
kurama config set auth.google.scopes '["openid", "email"]'
kurama config unset api.example.description
kurama config remove api.example auth.example --dry-run
```

- `set PATH VALUE` writes one dotted key inside a section the file has;
  `VALUE` is TOML (quote a string, `'"debug"'`), and an array is replaced
  whole. A fixed table the file omits is created; a new named entry is
  `kurama config add`'s, and `set` refuses it (`CONFIG_INVALID`).
- `set --file FILE` (`-` reads stdin) takes TOML with its own section
  headers and replaces each of those sections whole: a key it leaves out is
  removed, nothing is merged, so changing an auth's `kind` gives the whole
  section with every key it needs. Several sections change in one write;
  the sections it does not name keep every byte.
- `unset KEY...` removes keys, so their defaults apply again; `remove
  SECTION...` removes whole sections with the comments above them.
- A key or section the file does not have, one given twice, or a key given
  with the section that holds it is `CONFIG_INVALID`, and so is removing an
  `[auth.*]` that an API left in the file still uses -- by `auth`, or by
  having the auth's name and no `auth`: the message lists those APIs.
  Removing the auth with every API that uses it in one `remove` is fine.
  Removing an API never removes the auth it used.
- Only what is named changes: every other value, comment and the order stay
  as saved. `client_secret`, `token` and `password` take a secret reference
  only. The whole result is checked once, as `kurama config add` checks it,
  so a file that was invalid can be repaired by an edit whose result is
  valid; a file that is not TOML is not edited. Any refusal leaves the file
  untouched; its hint says the file was not changed.
- `--dry-run` prints the lines that would change (`-` / `+`, with the lines
  around them, every literal secret already in the file as `<redacted>`)
  and writes nothing. `--json` prints `{path, changed, applied, changes:
  [{section, action, key}], warnings}` with `action` `set`, `add` (a fixed
  table `set --file` created), `replace`, `unset` or `remove`, and `key` for
  `set` and `unset`; when the file already held the value, `changed` and
  `applied` are false, `changes` is empty and nothing is written. A save that fails is
  `CONFIG_WRITE_FAILED` (exit 1), as for `config add`.

`kurama config path [--json]` prints the file in use (with `--json`: `path`,
`source` -- `KURAMA_CONFIG_PATH` or `default` -- and `exists`) without
reading it. `kurama config list [SECTION] [--json]` names each saved section
with its keys, and `kurama config show [PATH] [--json]` prints the saved
values, the whole file or one dotted key (`auth.example`,
`aws.session_cache.duration`), with comments, without defaults or
environment overrides, and with every literal `client_secret`, `token` or
`password` shown as `<redacted>`. Both read the TOML syntax only, so they
work on a file that `config check` rejects.

### Checking the file

`kurama config check --json` reads config.toml, the AWS config
(`AWS_CONFIG_FILE`, else `~/.aws/config`) and the OpenAPI cache, and nothing
else: no 1Password, keychain, STS, OAuth, API or browser call, no
secret resolved, no URL fetched. Each `[auth.*]`, `[api.*]`, `[data.*]`,
`[db.*]` and `[s3.*]` section and each other top-level table is read on its
own, so one run lists a problem in every section that has one (the first of
each); a TOML syntax error is listed alone, since nothing after it can be
read. A problem that only follows from another (an `[api.*]` naming an
`[auth.*]` that did not read) is not listed twice. It is stricter than
`status`, which lists a section without following every reference: an
`error` here is what the command that uses the section would fail on.

stdout is one document whatever the run found: `schema_version` (1),
`valid`, `config` (`path`, `source` -- `KURAMA_CONFIG_PATH` or `default` --
and `exists`; an absent default file is the default configuration),
`aws_config` (`path`, `source` -- `AWS_CONFIG_FILE` or `default` -- and
`exists`; without the file `aws_profile` values are not checked, and an
`[auth.default]` still collides with the `default` profile, as `status`
says), `verification` (`network`: `not_contacted`, `credentials`:
`not_resolved`), `openapi` (each `[api.*]` with one: `api`, `location`,
`state` -- `read` for a file that normalized, `cached` for a URL whose cached
copy did, with `fetched_at`, `not_cached` for a URL never fetched, which is
unverified, `invalid` with a diagnostic -- and `path`, the file read) and
`diagnostics`, ordered by line: `severity` (`error` or `warning`), `code`
and `hint` (what the command that reads the setting would print:
`CONFIG_INVALID`, `PROFILE_NOT_FOUND` for an `aws_profile` that
`~/.aws/config` lacks, `API_SPEC_INVALID` / `API_SPEC_UNAVAILABLE` for a
description), `section` (`api.github`; null for the file as a whole),
`line` (null when unknown) and `message`. A cached copy of a description
whose body does not parse is an `error` naming the cached body: `kurama api`
uses that body as it is and fails. Cache metadata that cannot be read (or
that belongs to another URL) is a `warning` naming the file: kurama fetches
the description again.

The exit is the verdict. No `error`: exit 0 and stderr empty. Any `error`:
the run fails as every configuration error does, whatever codes the
diagnostics carry -- exit 2 and one `CONFIG_INVALID` JSON error document on
stderr (`error[CONFIG_INVALID]` and `hint:` lines without `--json`), with the
document still on stdout. Without `--json` stdout is the same report as text.

### Serving MCP to a cloud client

`kurama mcp --listen` reads `[mcp]`: `listen` (a loopback address and port,
such as `127.0.0.1:8807`), `token` (an `op://`, `aws-secrets://` or
`aws-ssm://` reference to 32 or more random bytes; a literal is refused),
`tools` (the tool names offered; every tool when absent), `call_timeout`
(seconds, default 600; keep it below the client's own timeout, 30 seconds
for ElevenLabs) and `max_concurrent_calls` (default 2). Write the section
into a configuration of its own, named by `KURAMA_CONFIG_PATH`, that holds
only the `[api.*]` the agent may reach, each with an `[api.<name>.agent]`
`allow_paths`.
