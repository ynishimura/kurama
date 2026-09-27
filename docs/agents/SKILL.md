---
name: kurama
description: Use when a task needs AWS credentials, an OAuth bearer token or an API call through kurama, when a command must run with such credentials, or when ~/.config/kurama/config.toml needs an [auth.*] source or an [api.*] profile. Covers kurama status, exec, env, token, api, login and the setup of new sources.
---

# kurama

Run `kurama agent` first and follow what it prints. It is the contract of
the installed version: the commands and their stdout, the `status --json`
fields, `kurama api`, the exit codes, and the Setup chapter for adding an
`[auth.*]` source or an `[api.*]` profile.

Two rules hold whatever the task is:

- Never write a secret into `config.toml`. `client_secret` takes an
  `op://<vault>/<item>/<field>` reference; ask the person to put the secret
  in 1Password when it is not there yet.
- Before calling an `[api.*]` profile that has an OpenAPI description,
  read its contract with `kurama api <API> --schema [OP]`: the parameters,
  body schemas and `limitations` come from there, not from guesses.
  `kurama api <API> --skill` writes an API-specific Skill from the same
  contract, for an agent that works with one API often.
- Exit code 3 means a person must act (`kurama login <source>` in a
  terminal, `op signin`, an unlocked keychain). Pass the `hint:` line to
  them instead of retrying.
