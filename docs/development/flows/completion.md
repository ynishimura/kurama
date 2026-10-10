# Dynamic zsh completion

How completion reparses the argv prefix and generates candidates without authenticating or fetching.

Dynamic zsh completion: `main` runs `CompleteEnv` before runtime setup;
`shell/cli/completion.rs` reparses the completed argv prefix with clap and
uses `adapters/completion.rs` to read configuration, AWS profiles and local
or cached OpenAPI descriptions synchronously. Candidate generation is
pure in `domain/functions/completion_candidates.rs`. The printed zsh
function calls the generating binary's absolute path without the export
wrapper; `name=` candidates suppress the trailing space. Completion never
authenticates, fetches or writes files. The harness marks direct and real-zsh
completion sessions automatically and applies shared zero-call/file checks.
Only direct runs expose child exit codes; the completing child's stderr
goes to `KURAMA_COMPLETION_STDERR` when set and to `/dev/null` otherwise,
so a person pressing Tab sees nothing while every scenario fails on a
diagnostic. Real-zsh PTY also checks visible diagnostics and command
insertion.
Files: `cargo xtask map shell-integration`.
