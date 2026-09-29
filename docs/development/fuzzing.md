# Fuzzing

`fuzz/` holds cargo-fuzz targets for the code that reads input kurama does
not control. Each target feeds arbitrary bytes to one parser and passes as
long as the parser returns: an error is a fine answer, a panic, an overflow or
a hang is a finding.

| Target | Reads | Why it is exposed |
| --- | --- | --- |
| `openapi_spec` | `adapters::openapi::parse_spec`, as OpenAPI and as a Discovery document | a description fetched from a URL is whatever the server sent |
| `oauth_answers` | `parse_token_response` and `parse_callback_query` | an authorization server's answer and the redirect a browser brings back |
| `aws_config` | `adapters::profile::parser::parse_aws_config` | `~/.aws/config` is written by other tools and read at every `status` and completion |
| `secret_ref` | `SecretRef::parse` | every configured secret goes through it when the configuration loads |

## Running

cargo-fuzz is pinned in `mise.toml`; the targets need a nightly toolchain
(`rustup toolchain install nightly --profile minimal`). From the repository
root:

```bash
cargo +nightly fuzz list
cargo +nightly fuzz run -O openapi_spec -- -max_total_time=60
```

`fuzz/` is a crate of its own with its own `Cargo.lock`, outside the
workspace, so the gate never builds it. Its builds land in the repository's
`target/<host triple>/release/`, apart from the gate's, and the first one
compiles DuckDB again (about 20 minutes). A crash writes the input to `fuzz/artifacts/<target>/`, and
`cargo +nightly fuzz run <target> <file>` replays it. Fix the parser, then add
the input as a unit test next to it, so the regression is caught without
nightly.

`.github/workflows/fuzz.yml` runs every target for five minutes once a week
and on manual dispatch (`gh workflow run fuzz.yml -f seconds=600`), and
uploads the crashing inputs when one fails.

A new target goes where input arrives from outside: a file another tool
writes, a network answer, a value a person types into the configuration. Add
it to `fuzz/Cargo.toml` and to the table above.
