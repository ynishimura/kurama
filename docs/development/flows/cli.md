# CLI flow

How a command line becomes effects, which subcommands answer agents in JSON, and where an error gets its code.

CLI flow: `src/shell/cli/args.rs` (clap) -> `parser.rs` (`CliCommand`) ->
`bootstrap.rs` (config, logging) -> `dispatch.rs` ->
`commands/*.rs` -> effects -> `cli/executor.rs` -> `src/shell/executor.rs`
(the only AssumeRole interpreter; the TUI calls it too) -> `src/adapters/*`.
Inherited `AWS_*` credentials are cleared at the SDK boundary, in
`src/adapters/aws/config_builder.rs`.
`kurama login` on an AWS profile runs `src/workflows/mfa_login` through
`src/shell/mfa_login_executor.rs`, which shares the GetSessionToken and
TOTP-window helpers of `src/shell/executor.rs`.

JSON clients: a subcommand that answers an agent with one JSON document on
stdout and one JSON error document on stderr is a `ClientKind`
(`src/shell/cli/client.rs`). That one table decides the `--kind` values of
`agent` and `status`, which subcommand `main.rs` lets answer its own usage
failures and with what code, and it carries what those subcommands share:
reading a `--request` file or stdin, and projecting the envelope with
`--jq`. The error document itself is `src/shell/cli/client_error.rs`.
`api`, `status`, `env` and `token` have that error document under
`--json` / `--jq` but no request contract: they are the other variants of
`JsonErrorKind` in the same file, so `agent --kind` cannot list them, and
`main.rs` answers their usage failures only when JSON was asked for.
`CliCommand::leaves_stderr_to_json_errors` silences their progress lines
and logs when stderr is not a terminal.

Errors: `src/shell/cli/error_code.rs` maps the typed error chain to
`error[CODE]` and an exit code. `rg -n <CODE> src tests` lands on the
classification arm and the scenario that pins it; `tests/architecture/`
fails for a code that no scenario expects.
