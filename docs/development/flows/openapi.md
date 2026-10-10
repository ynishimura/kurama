# OpenAPI flow

How an API description is loaded, cached, revalidated and normalized into an `ApiSpec`.

OpenAPI flow: `[api.*] openapi` (a URL or an absolute file) is loaded by
`src/shell/spec_loader.rs`: a file is read as is; a URL is fetched through
`ApiRuntime` (with the API's credential when `openapi_auth`, which the
config restricts to the origin of `base_url`), cached under
`~/.cache/kurama/openapi/`, reused for `[openapi] revalidate_after` seconds
(default 3600; 0 checks every time), then revalidated with `ETag` /
`Last-Modified`. A 304 restarts the interval by updating only the matching
generation's metadata. Short per-entry write locks prevent an older 304
from replacing a newer fetch; reads and completion do not create locks.
An expired cached copy is used
with a warning when the server is unreachable. `--refresh-spec` ignores
the interval; files are always reread. Local-file reads,
home/cache paths and byte parsing/normalization live in `adapters/openapi`,
with no HTTP client or credential dependency. The document is
normalized into an `ApiSpec` (`src/domain/types/api_spec.rs`) by the
format its key names (`SpecFormat`): `openapi` by `functions/openapi.rs`,
`discovery` (a Google Discovery Document, loaded the same way) by
`functions/discovery.rs`, which rewrites it as OpenAPI 3 first,
`graphql` (an endpoint path, introspected by a POST with the API's
credential, or a `graphql_schema` file) by `functions/graphql.rs`; the
`ApiSpec` is what
`--ops`, `--describe`, operation targets with `-P` and the explorer TUI
read. The files: `cargo xtask map api-explorer`.
