# Security policy

kurama handles AWS role credentials, MFA sessions, OAuth tokens and API keys.
A flaw that exposes one of them is the kind of report this policy is for.

## Reporting a vulnerability

Report it privately through GitHub: **Security > Report a vulnerability** on
this repository, or directly at
<https://github.com/ynishimura/kurama/security/advisories/new>. Do not open
a public issue, and do not include a real
credential in the report; a redacted value or a description of where it
appeared is enough.

Please include the kurama version (`kurama --version`), the command, the
relevant configuration with secrets replaced, and what you observed.

## What is in scope

- A credential, token, client secret or API key written to a file, printed on
  stdout or stderr, logged, or shown in `--dry-run` / `-v` output.
- A credential sent to a host other than the one the configuration names
  (for example through a redirect, an OpenAPI description or a paginated
  `next` link).
- The shell integration (`kurama init zsh`, `env`, `exec`) evaluating
  something other than the export script kurama wrote.
- A request, AWS call or 1Password read that kurama makes without the
  configuration or the command asking for it.
- `kurama mcp --listen`, the one mode that serves requests from outside the
  machine: a request
  served without the `[mcp] token`, a listener on an address other than
  loopback, or the token appearing in output, logs or a response. TLS is
  outside kurama (Tailscale Funnel or another front terminates it), and so is
  telling callers apart: whoever holds the token acts with your credentials.

The guarantees kurama is built to keep are listed as invariants in
[AGENTS.md](AGENTS.md#invariants).

## What is not in scope

- Access by someone who already controls your user account, your shell or
  your keychain.
- Weaknesses of the services kurama talks to (AWS, 1Password, an OAuth
  provider or an API).
- A secret you wrote as a literal in `config.toml` where a reference was
  accepted.

## What happens next

You will get an acknowledgement within a week. Once the issue is confirmed, a
fix is prepared privately and released, and the advisory is published with
credit to you unless you ask otherwise. Only the latest release receives
security fixes.
