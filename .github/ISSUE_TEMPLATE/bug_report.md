---
name: Bug report
about: kurama did something other than what its documentation or `kurama agent` says
labels: bug
---

<!-- A vulnerability (a credential written, printed, logged or sent somewhere
     it should not be) goes to SECURITY.md, not here. Never paste a real
     credential: replace it with <redacted>. -->

Affects: <feature>

<!-- The features whose files the fix touches, as `cargo xtask map` names them
     (for example `api-client, oauth`). Leave `<feature>` if you do not know;
     a maintainer fills it in. -->

## What happened

<!-- The command, the output (stderr included), and the exit code. -->

```console
$ kurama ...
```

## What was expected

## Environment

- kurama version (`kurama --version`):
- OS and version:
- Shell:
- Relevant configuration (secrets replaced):

```toml
```

## Scenarios

<!-- Optional: the runtime scenario that should fail now and pass after the
     fix, one `- [ ] <feature>_<behavior>` per line. -->
