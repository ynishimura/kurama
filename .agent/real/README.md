# Real-environment evidence

One `<feature>.json` per feature with a probe, written by
`cargo xtask verify-real <feature>` and committed with the change it verifies.
A feature whose `real` says `cases = "..."` has the `[real]` cases under
`tests/cases/<feature>/` as its probe: `verify-real` runs
`cargo xtask verify --layer real <feature>` and records each case's report
as a contract, paired with the same case on the fake layer.
`cargo xtask verify-real --check` holds each file to the feature's current
files, its fixtures and a credential scan; `cargo xtask branch-check` fails a
changed feature whose evidence is missing, stale or not `verified`. Do not edit
these files by hand: run the probe again.

A `[real]` case that requires `env:NAME` says in its comment what to export.
`mcp` needs `KURAMA_REAL_MCP_TOKEN_REF`, a reference such as
`aws-ssm://<profile>/<parameter>` that the server reads its token through,
and `KURAMA_REAL_MCP_TOKEN`, the value it holds, which the harness sends as
a client would; the layer checks every report for that value.
