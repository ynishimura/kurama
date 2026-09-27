## Shell completion

After `compinit`, `eval "$(kurama init zsh)"` registers a wrapper and dynamic
zsh completion bound to the generating binary's absolute path. Completion
bypasses the wrapper. `kurama completions zsh` emits just the completer.
Tab offers the command's AWS/auth namespace, API names, and operation IDs
for TARGET, `--describe` or the `--ops` query, and the HTTP methods for `-X`.
An argument whose value cannot be completed offers nothing rather than files.
For an operation target, `-P` completes `name=`
(required first, excluding assigned names), then enum/boolean/default/example
values after `=`. It retains `NAME=VALUE` as one shell word. Only configuration and local/cached OpenAPI
descriptions are read: no STS, OAuth, keychain, 1Password, HTTP or disk writes.
Missing/invalid sources yield no candidates. Uncached URL descriptions need
a normal `api --ops` call before their operations can be completed.

`--jq` completes response paths using the profile's `openapi`, including
quoted filters such as `'.content[] | select(.na'`. It resolves operationId,
`METHOD /path`, or a plain path (GET by default, POST with `-d`); `-X` wins.
With `--json`, response fields are below `.body` in the envelope. jq candidates add no space or closing quote, allowing the expression to continue. An absent description, unknown operation, empty
schema or dictionary-only schema yields no candidates. Normalization stops at
depth 6 and chooses the first `oneOf` / `anyOf` branch. Examples/defaults do not
invent fields, schemas may differ from live bodies, and completion does not
infer shapes produced by transformations such as `map({a: .x}) | .[0].`.

