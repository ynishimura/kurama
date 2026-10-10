# Agent runs

How `KURAMA_AGENT` and the `[agent]` policy gate a run, and what the audit log records.

Agent runs: `KURAMA_AGENT` (anything but empty or `0`) marks a run as an
agent's (`src/shell/agent_policy.rs`). The `[agent]` policy
(`src/domain/functions/agent_policy.rs`, `[api.<name>.agent]` replacing
its keys) refuses an `api` request, a `db --commit` and `exec`'s full
permissions as `AGENT_POLICY_DENIED` before any credential is read, unless
`--confirm`. `src/shell/cli/mod.rs` opens one audit entry per `api`,
`exec`, `db` and `data` run (`src/shell/audit.rs`), the commands note the
method and path (never the query), the status and the SQL fingerprint, and
the entry is appended to `~/.local/state/kurama/audit.jsonl` when the run
ends -- or before `exec` replaces kurama. A write failure is a warning.
The harness holds every audit line to the fields `AuditEntry` names.
Files: `cargo xtask map agent-policy` and `cargo xtask map audit`.
