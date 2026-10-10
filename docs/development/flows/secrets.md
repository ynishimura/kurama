# Secret references

How a configured secret reference is parsed, resolved once per process and classified when it fails.

Secret references: a configured secret (`[auth.*] client_secret`,
`[auth.*] token`, `[db.*]
username` and `password`) is one string, parsed once at deserialization by
`src/domain/types/secret_ref.rs` -- `op://`, `aws-secrets://`, `aws-ssm://`
or a literal, and a `<scheme>://` that is none of them is `CONFIG_INVALID`
with its line rather than a password nobody notices. One `SecretResolver`
implementation (`src/adapters/secret_resolver.rs`) dispatches on the variant
to `src/adapters/auth/secret.rs` (`op item get`, or `op read` for a section or an attribute) or `src/adapters/aws/
secret_store.rs` (`GetSecretValue`, `GetParameter`), and holds each secret it
read for the process whichever backend answered, so `#username` and
`#password` of one managed secret are one `GetSecretValue` and two fields of
one 1Password item are one `op item get` -- one biometric prompt. AWS credentials come through `AssumedRoles`
(`src/shell/aws_profile_credentials.rs`), the shared cache in front of the
`kurama env` AssumeRole path, so a tunnel, an IAM token and every reference
naming one profile assume its role once. A failure carries a typed
`SecretFailure` the whole way -- through the pure OAuth workflow too -- and
that is what picks `SECRET_UNAVAILABLE` / `SECRET_INVALID` /
`SECRET_REJECTED` / `SECRET_FAILED`. Getting the credentials is not one of
them: `SecretError::credentials` keeps the AssumeRole chain, so a wrong TOTP
stays `STS_*` and exit 3 instead of becoming a reference that is malformed.
The value comes back as a `Secret` (`src/domain/types/secret.rs`), which
redacts `Debug` and `Display`, zeroizes on drop and is read through
`expose()` only where a request, an export script or stdout takes it.
The files: `cargo xtask map secrets`.

Every `op` call goes through `src/adapters/auth/op_cli.rs`: it passes a
1Password service account token (the environment wins, else the keychain
entry named by `[onepassword] service_account_keychain`, read for `$USER`)
and, when no terminal is attached, kills the CLI after
`[onepassword] timeout` seconds, so an unanswered biometric prompt is
`SECRET_UNAVAILABLE` (exit 3) instead of a hang.
