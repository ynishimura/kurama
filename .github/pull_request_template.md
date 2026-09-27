## What changed

<!-- One paragraph. Link the issue. -->

## Impact

<!-- Paste `features`, `dependent_features` and `scenarios` from `cargo xtask impact`. -->

```json
```

## Verification report

<!-- Paste target/agent/verification-report.md from `cargo xtask verify affected`
     (or `verify all`). The table lists every scenario that ran, its result,
     the STS and 1Password calls it observed and whether any file was written. -->

## Definition of done

- [ ] Existing implementation searched; no duplicate capability added
- [ ] Tests written first; `cargo test --locked --features test-fakes -- <filters>` passes
- [ ] User-visible change has a scenario in `tests/scenarios/`
- [ ] `cargo xtask verify affected` passes (no secrets on disk, no error logs, no extra STS/op calls, clean stdout)
- [ ] `cargo xtask check` passes
- [ ] `.agent/features/`, `AGENTS.md`, `README.md` updated where structure or behavior changed

## Not verified

<!-- Anything the fakes cannot reach (real keychain, real AWS, browser, TTY)
     and what you did instead, if anything. -->
