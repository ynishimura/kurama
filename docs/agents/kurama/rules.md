## Rules

- stdout carries data only. Progress lines (`# ...`), logs, `error[CODE]:`
  and `hint:` go to stderr, without ANSI colors when piped.
- With `--json` (or `--jq`, which only `api`, `data` and `db` take),
  `api`, `status`, `env`, `token`, `config`, `data` and `db` report a failure as one
  JSON error document on stderr instead of the `error[CODE]` and `hint:`
  lines, usage failures included:
  `{"schema_version": 1, "error": {"code", "category", "exit_code",
  "message", "hint", "retry", "next_actions": [{"message"}]}}`. `category`
  is `tool`, `usage`, `human` or `remote` (exit code 1 to 4); `retry` is
  `never` or `after_human` (and `read_only_retry` for `db`). Branch on
  `code` and `retry`, never on the message. With stderr not a terminal
  those runs print no progress line or log either, so stderr is empty or
  that one document (`api --pages` after its last page: the stop report
  `{"schema_version": 1, "pages": {...}}` instead, told apart by its
  `pages` key); `api --dry-run --json` puts its plan on stdout, and
  `-v` and a `--dry-run` without `--json` still print what they were
  asked for. That silence drops warnings too: `# warning: using the cached
  API description ...` (a stale OpenAPI description used because the server
  was unreachable) is not printed in JSON mode. A usage failure of `api` is
  `API_ARGUMENT_INVALID`; of `status`, `env` and `token` it is
  `ARGUMENT_INVALID`, whose `hint` names `kurama <subcommand> --help`;
  of `s3` it is `S3_INVALID`, with or without `--json`;
  a configuration kurama refuses is `CONFIG_INVALID`. Without `--json` the
  same failure is the text lines. `agent --kind` lists `data`, `db` and
  `s3`: `data` and `db` carry a request schema, `s3` its operations and
  default bounds.
- Always pass a command. Plain `kurama` is the TUI and exits 2 without a
  terminal; `kurama api <API>` without a `TARGET` (the explorer) exits 2
  too. `--ops`, `--describe`, `--schema` and `--skill` are the explorer for scripts.
- Set `KURAMA_AGENT=1` when your harness runs commands on a pseudo terminal
  (a PTY, tmux, `script`): such a run is non-interactive whatever its
  streams are. No TUI opens (the exit 2 above), stdout is the same bytes a
  pipe gets (no laid-out JSON body), and no progress line is rewritten. It
  changes nothing about how kurama authenticates.
- Nothing prompts without a terminal. `login`, `env`, `exec`, `console`,
  `token` and `api` run `op` when they need a TOTP code, the long-term key or
  an `op://` client secret. Without a terminal the 1Password desktop app would
  ask a person to approve it, so kurama kills `op` after `[onepassword]
  timeout` seconds (default 30) and stops with `SECRET_UNAVAILABLE` (exit
  code 3, `MFA_PROVIDER_FAILED` for a TOTP code). To run unattended, give it
  a 1Password service account: export `OP_SERVICE_ACCOUNT_TOKEN`, or ask the
  person to store the token in a keychain entry with
  `security add-generic-password -a "$USER" -s <service> -w`, which reads it
  from a prompt instead of taking it on the command line, and name `<service>`
  in `[onepassword] service_account_keychain`. Never pass a token on a command
  line yourself. A grant that needs a browser or a code entry stops with exit
  code 3 instead of waiting.
- The keychain is a cache, not a requirement. macOS grants access per build
  signature, so a rebuilt kurama cannot read the entries an older one wrote.
  A token that cannot be cached is still returned and used, and the next run
  fetches a new one; an unreadable MFA session is fetched again. Only the
  service account token has no fallback, so an unreadable entry there leaves
  `op` asking a person, which the deadline then ends.
- An `op://` reference is `op://<vault>/<item>/<field>` and the item part
  cannot contain `(`, `)` or a space-separated title with those: use the
  item's id (`op item list --vault <vault>`) when the title has them, or
  `op read` refuses the reference before anything is fetched. A secret AWS
  holds is `aws-secrets://` or `aws-ssm://` instead and needs no `op` at all;
  the Setup chapter has the grammar and the permissions.
- Parallel `kurama api` calls on one source refresh the token once; the
  others wait for the lock and reuse it.
- A rebuilt binary is not on the keychain's access list, and no terminal
  means no dialog to add it. Cached MFA sessions and stored tokens then read
  as `macOS denied this build access to the keychain entry`, one warning per
  process however many entries were refused (each is named at debug under
  `RUST_LOG=kurama=debug`): `status` shows
  the session as `unreadable`, `env` and `exec` fetch a fresh session (one
  more `op` call), and a stored token that cannot be read counts as no token;
  a grant that needs a person then exits 3 with the `kurama login <source>`
  hint, while a `client_credentials` source fetches a new token and succeeds.
  Ask the person to run kurama once in a terminal and choose Always Allow, or
  to install it with `cargo xtask install-signed` so the signature stops
  changing. Either way you can keep working: the keychain is a cache.
- A run whose environment sets `KURAMA_AGENT` (to anything but empty or `0`)
  is held to the `[agent]` policy; a person's run never is. `api` sends only
  the methods in `allow_methods` (default `GET`, `HEAD`), to paths no
  `deny_paths` pattern matches and, when `allow_paths` is set, one of its
  patterns does (`*` is any run of characters; the query is never matched);
  `[api.<name>.agent]` replaces those keys for one API. `exec` on an AWS
  profile attaches ReadOnlyAccess (`exec_readonly`, default true); a profile
  without `role_arn` (an IAM user) has no role session to attach it to, so
  that `exec` is refused. `db --commit` is refused whatever `allow_write` says. A refused call is
  `AGENT_POLICY_DENIED` (exit code 3), decided before any credential is
  read, so nothing was sent. Ask the person; when they agree, rerun the same
  command with `--confirm` (`api`, `exec`, `db --commit`). Never add
  `--confirm` on your own. A `--dry-run` sends nothing and is never refused.
- Each `api`, `exec`, `db` and `data` call of a `KURAMA_AGENT` run is
  recorded in the audit log without a secret, a query string, a header or a
  body; `kurama audit --json` lists what the person will see, and
  `kurama audit --watch` is the screen where they watch it live (a
  terminal screen: never open it yourself).
