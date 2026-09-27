## `kurama status --json`

One array with one object per AWS profile, `[auth.*]` source, `[api.*]`
profile, `[data.*]` workspace and `[s3.*]` connection. It is printed as a single line; it is wrapped here for reading:

```json
[{"name":"ops","kind":"aws","active":false,
  "role_arn":"arn:aws:iam::123456789012:role/Ops","region":"ap-northeast-1",
  "mfa_serial":"arn:aws:iam::123456789012:mfa/me","session":"valid",
  "logged_in":true,"expires_at":"2026-09-17T03:00:00Z","needs_human":false},
 {"name":"github","kind":"auth","auth_kind":"oauth","grant_type":"authorization_code",
  "header":null,"env_var":"GITHUB_TOKEN",
  "active":false,"token":"valid","logged_in":true,"expires_at":"2026-09-17T03:00:00Z",
  "refreshable":false,"needs_human":false},
 {"name":"example","kind":"auth","auth_kind":"token","grant_type":null,
  "header":"X-API-Key","env_var":"EXAMPLE_TOKEN",
  "active":false,"token":"not_checked","logged_in":false,"expires_at":null,
  "refreshable":false,"needs_human":false},
 {"name":"github","kind":"api","base_url":"https://api.github.com","description":"GitHub REST API",
  "auth":"github","auth_status":{"token":"valid","logged_in":true,"expires_at":"...",
  "refreshable":false,"needs_human":false},"aws_profile":null},
 {"name":"apigw-dev","kind":"api","base_url":"https://abc123.execute-api.ap-northeast-1.amazonaws.com/prod",
  "description":null,"auth":null,"auth_status":null,"aws_profile":"dev"}]
```

- `session` (aws) is `not_required` (no MFA), `valid`, `missing`, `unreadable`
  (keychain error) or `cache_disabled`. `token` (auth) is `valid`, `expired`,
  `missing`, `unreadable` or `not_checked`; `refreshable` means an expired
  token can be refreshed without a person.
- `auth_kind` is `oauth` (a grant issues the token, which is cached) or
  `token` (a credential issued elsewhere, read from its secret store on every
  use). A `token` source reports `"token":"not_checked"` and never
  `"missing"`: `status` does not read secret stores, and there is nothing to
  log in to. `grant_type` is null for it and `header` names the header its
  credential is sent in; for an `oauth` source `header` is null.
- `needs_human: true` means `exec`, `env`, `token` and `api` will stop with
  exit code 3. For an AWS profile: MFA is required, no cached session is
  valid and 1Password is not configured; ask a person to enable
  `[onepassword]` or to run `kurama` in a terminal and type the code. For an
  auth source: no usable token and the grant needs a browser or a code; ask a
  person to run `kurama login <source>` in a terminal. `client_credentials`
  and `auth_kind: "token"` sources never need a person. An API with `aws_profile` follows the AWS
  profile it names: read that profile's `needs_human`.
- `active: true` marks the profile (`KURAMA_AWS`) or source (`KURAMA_AUTH`)
  whose credentials the calling shell holds.
- An `s3` row is the `[s3.*]` as configured: `aws_profile`, `region`,
  `bucket`, `prefix`, `"connection_status":"not_checked"` and
  `"auth_status":null`. Neither the role nor the bucket is reached, so it
  says nothing about whether either answers; `kurama s3 <S3> --list` does.
- `--only <KIND>` keeps one kind of row, which is how to read the APIs
  without the AWS profiles first: `aws`, `auth`, `api`, `data` or `s3`.
  Databases are listed by `kurama status --kind db --json` instead.
- `status` calls neither AWS, nor 1Password, nor a token endpoint.

```sh
kurama status --only api --json
```

## `kurama agent ready --json`

One JSON document, one line, that says for every AWS profile, `[auth.*]`,
`[api.*]` and `[db.*]` whether a call through it goes through now, so a
person can be asked before the first exit code 3:

```json
{"schema_version":1,"kind":"agent_ready","ready":false,"sources":[
 {"name":"ops","kind":"aws","state":"ready","reason":"MFA session valid for 11h 52m",
  "expires_at":"2026-09-17T03:00:00Z","next_actions":[]},
 {"name":"github","kind":"auth","state":"needs_human",
  "reason":"no token is stored; the authorization_code grant needs a person to log in",
  "expires_at":null,"next_actions":["kurama login github"]},
 {"name":"github","kind":"api","state":"needs_human",
  "reason":"through auth github: no token is stored; ...","expires_at":null,
  "next_actions":["kurama login github"]}]}
```

- `state` is `ready` (nobody is needed), `will_prompt` (1Password asks a
  person to approve a read first: no service account is configured),
  `needs_human` (the call exits 3 until a person runs `next_actions`) or
  `misconfigured` (the configuration names an AWS profile that does not
  exist; `next_actions` is `kurama config check`). `ready` at the top is
  `true` only when every source is.
- An `[api.*]` or `[db.*]` is as ready as the least ready of what it goes
  through: its `[auth.*]`, its `aws_profile`, the references of its
  username and password, its IAM or tunnel profile.
- It reads what `status` reads and calls nothing: no STS, token endpoint,
  API, AWS secret store or 1Password. A secret is judged by the kind of its
  reference, never read, and no reference is printed. Unlike the rest of
  `kurama agent`, it reads the configuration.
