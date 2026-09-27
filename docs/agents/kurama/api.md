## `kurama api`

`kurama api <API> <TARGET>` with `TARGET` = `/path` (under `base_url`), a
URL on the origin of `base_url` (another host is `API_ARGUMENT_INVALID`: the
bearer token never leaves the API), `METHOD /path`, or an operation of the
API's OpenAPI description (an `operationId` such as `issues/create`, or
`METHOD /path/{param}` with the path template). Options: `-X METHOD`,
`-H "Name: value"` (repeatable), `-d DATA` (`@file`, `@-` for stdin; makes
the default method POST), `-P name=value` (repeatable; an operation's path,
query or header parameter, placed where the description says), `--json`,
`--jq FILTER`, `-o` / `--output PATH`, `--pages N`, `--cursor PATH=PARAM`,
`--shape`, `--sample N`, `--dry-run`, `-v`, `-k` (the API request only; the
authorization server is always verified), `--timeout SECONDS` (60).
`-v` also prints one `< secret read: ...` line on stderr each time a secret
is read from its store -- `ssm:GetParameter <id> (<region>)`,
`secretsmanager:GetSecretValue <id> (<region>)` or `op item get <item> --vault <vault>`, never
the value. A secret the process already read prints nothing, and
`--dry-run` reads none. `token`, `env`, `exec` and `login` take the same
`-v` for the secrets of an `[auth.*]` source; `exec` prints the lines before
the command starts.

The description is `openapi` under `[api.<name>]` (a URL, cached under
`~/.cache/kurama/openapi/` and reused without a request for
`[openapi] revalidate_after` seconds, default `3600`, integer seconds only;
`0` checks on every use). After that interval, `ETag` / `Last-Modified`
revalidate the copy, and a 304 starts a new interval. An expired cached copy
is used with a `# warning:` line when the server is unreachable.
`--refresh-spec` ignores the interval, and `-v` prints the source and cache
timestamp. Local JSON / YAML files, absolute or under `~`, are always reread.
A Google API describes itself with a Discovery Document instead:
`discovery = "<URL or file>"` in place of `openapi` (both on one API is
`CONFIG_INVALID`), read, cached and revalidated the same way. Each method is
an operation named by its Discovery id (`sheets.spreadsheets.get`), its path
is relative to `rootUrl + servicePath` -- the `base_url` to set
(`https://sheets.googleapis.com`) -- and a `{+name}` parameter is sent with
its `/` and `!` as they are. The document-level parameters (`fields`,
`alt`) are not an operation's `-P`: put them in the query of a plain path
(`kurama api sheets '/v4/spreadsheets/<id>?fields=properties.title'`).
A GraphQL API names its endpoint instead: `graphql = "/graphql"` (the path
under `base_url`; exclusive with `openapi` and `discovery`). Its schema is
introspected -- one POST of the introspection query, always with the API's
credential -- and cached like a description (a POST has no validators, so an
expired copy is introspected again). Every field of the Query and Mutation
types is an operation `query.<field>` / `mutation.<field>`, `POST
<endpoint>`; `--describe` gives its arguments as the `request_body` (the
variables object: `GraphQL variables: input: IssueCreateInput!`, a
`skeleton` with the input type's non-null fields, the rest `optional`) and
its return type as the `response` (object types two levels deep).
An operation id as TARGET calls the field: kurama writes the query
document (`query($first: Int) { issues(first: $first) { nodes { id } } }`)
and POSTs `{"query", "variables"}` to the endpoint. `-d` is the variables
object and each `-P name=value` one variable over it, typed by the
argument's GraphQL type (`-P first=3` is the number 3, `-P input='{...}'`
the object); a non-null argument without a value is
`error[API_PARAMETER_MISSING]` before anything is sent. `--select 'nodes {
id title }'` is the selection set; without it the return type's scalar and
enum fields are selected, and a type with none is `API_ARGUMENT_INVALID`
naming its fields. An answer with `errors` is `error[API_GRAPHQL_ERROR]`
(exit 4) with the first error's message and path, whatever the status;
with `--json` the envelope, `data` included, is printed first. `--dry-run`
prints the document and variables it would send. A mutation is sent only
when its `mutation.<field>` is the TARGET, and under the `[agent]` policy
every GraphQL call is a POST to the endpoint. `kurama api <name> POST
/graphql -d '{"query":"..."}'` (or `/graphql -d ...`) still sends a body as
it is. A server that
refuses introspection is `error[API_INTROSPECTION_REFUSED]` (exit 4): save
an introspection result (JSON with `__schema`) and name it with
`graphql_schema = "<file>"` next to `graphql`; it is read without a request.
`--ops [QUERY] --json` lists `{id, method, path, summary, tags, scopes,
deprecated}` for the operations matching QUERY (id, method, path, summary
or tag; every operation without QUERY). `--describe <OP> --json` returns
the parameters (`name`, `in`, `type` = the schema type, `required`, `enum`,
`default`), the `request_body` with its `content_type`, a `skeleton`
holding the required properties only (seeded with the document's example,
default or first enum value: replace them) and `optional`, the paths of the
properties the skeleton leaves out (`labels`, `reviewer.email`,
`assignees[].role`; add the ones you need yourself), the `scopes`, what is
`unsupported`, and an `example_command` to run. `response` is null when the
selected response has no schema; otherwise it contains `status`,
`content_type`, `description` and `shape` (all properties, including optional
ones, with scalar types at the leaves). Selection is `200`, `201`, the first
other 2xx, then `default`; `application/json` wins over other media types.
References stop at depth six and cycles become `{}`; `oneOf` / `anyOf`
keep only the first branch. `--jq` applies to
either JSON document. `--refresh-spec` fetches the description again
(alone, or with `--ops` / `--describe` / `--schema` / a TARGET).

`--schema [OP]` prints the contract to build calls from: one JSON document
on one line (whatever the terminal), for every operation or for OP. It
loads the description the way `--ops` does and calls no operation of the
API. Keys: `schema_version` (1; raised when a key changes meaning or goes
away), `api` (`name`, `base_url`), `description` (`title`, `version`,
`description`, `server`, and `source`: `kind` = `file` with `path`, or
`fetched` / `cached` / `validated` / `stale` with `url` and, for the cache,
`fetched_at`), `options` (every option of `kurama api`: `name`, `short`,
`value`, `repeatable`, `help`, read from the parser itself), `operations`
and `warnings`. Each operation carries what `--describe` does plus whole
JSON Schemas instead of the skeleton and shape: each parameter's `schema`,
`request_body.schema` and `response.schema` (`type` -- an array such as
`["string", "null"]` for a 3.1 type array or a 3.0 `nullable` --, `enum`, `default`,
`example`, `items`, `properties`, `required`, and `$ref` for a reference
outside the document), and `limitations`: every place the reduction left
something out, with `at` a JSON pointer into the operation and `kind` one
of `first_alternative_only` (`keyword`, `alternatives`), `cycle` (`ref`),
`depth_bound`, `unresolved_ref` (`ref`), `other_content_types` (media types whose schema
is not read) and `unsupported_parameter` (`detail`; a call is refused). An
empty `limitations` means the schemas are the document's own. `--jq`
applies to the document; `--json` changes nothing.

`--skill` prints an Agent Skill for the profile on stdout: one `SKILL.md`,
written from the same contract `--schema` prints (no second reading of the
description), and nothing else -- no operation is called. It holds a
frontmatter with `name: kurama-api-<name>` and a one-line double-quoted
`description` without `<` / `>`. The `name` is the profile name when it
is already lowercase letters, digits and single hyphens; otherwise it is
reduced to them, `claude` / `anthropic` are replaced, it is cut to 64
characters, and an eight-digit hash of the profile name is appended, so
two profiles never share a name,
then the profile (base URL; how it authenticates, by source and header
name or AWS profile only, never a reference or a value; whether the
description is a file or a URL), the steps to find, read, preview
(`--dry-run --json`) and call an operation and how to read a failure (the
JSON error document's `code`, `category`, `retry`, `next_actions`; the exit
codes), every option of `kurama api` from the parser, and the operations.
Up to 25 operations each get their description (behind `About:`), their
parameters (location, type, up to five enum values, default), body,
response, scopes, limitation count and an example command that uses a
parameter's first enum value or default where it has one; above that, the tags with their
counts and one line per operation, at most 200 lines, then how many more
`--ops QUERY` finds. Description text is put on one line, stripped of
control characters and cut (160 characters a line, 800 for the API's own
description), and no line starts with it, so it cannot end the
frontmatter, open a fence, an HTML comment or a heading. The same
description and configuration give the same bytes, wherever the
description was read from. It loads the description the way `--schema`
does: a file is read and nothing else; a URL is read from the cache within
`[openapi] revalidate_after`, otherwise fetched or revalidated -- with the
API's credential when `openapi_auth` is set, which is the only way it can
start a credential source. `--refresh-spec` is the explicit fetch.
`--skill` refuses a TARGET, `--ops`, `--describe`, `--schema`, `--json`,
`--jq` and `--output`. To install it, name the directory after the
printed `name` (a loader requires the two to match):

```sh
skill="$(kurama api <API> --skill)" &&
dir=~/.claude/skills/"$(printf '%s\n' "$skill" | sed -n 's/^name: //p' | head -n 1)" &&
mkdir -p "$dir" && printf '%s\n' "$skill" > "$dir/SKILL.md"
```

and again after the description changes.

- stdout is the response body. With `--json` it is one line
  `{"status": 200, "headers": {...}, "body": <JSON or string>}`; with `--jq`
  it is one line per result (strings raw, other values compact JSON) applied
  to the body, or to the envelope with `--json`. Header names are lowercase
  and a repeated header is joined with `, `, except `set-cookie`, which is
  always an array with one string per cookie (`.headers["set-cookie"][0]`).
- On a terminal a JSON body is laid out over several lines (two-space
  indentation) for the person reading it: only the whitespace between tokens
  changes, so the values, the key order and the number notation stay the
  server's. A pipe or a file gets the server's bytes exactly, with nothing
  added (not even a final newline); a terminal gets a final newline. A
  `KURAMA_AGENT` run gets the pipe's bytes on a terminal too.
- `--json` and `--jq` print the same thing wherever they run, but both build
  their output from the parsed body rather than from its bytes: `10.00` comes
  back as `10.0`. Integers up to 64 bits (ids included) are exact; one beyond
  that becomes a double and loses digits, and a number outside a double's
  range makes the whole body a string in the envelope. Read an exact decimal
  from the body itself.
- `--output PATH` writes a 2xx body to PATH byte for byte (binary, an
  empty body, the server's number notation and duplicate keys included) and
  prints nothing on stdout. The bytes go to a temporary file in PATH's
  directory that is renamed over PATH once all of them are written, so PATH
  is either what it was or the whole body. PATH is replaced, not written
  through: a symlink or a hard link at PATH is replaced by a new file (its
  target and the other links are left alone), and the old file's mode does
  not carry over -- the new file has mode 600. A status outside 2xx, a
  timeout or a failed connection writes nothing and keeps an existing file.
  There is no fsync, and a process killed during the write can leave a
  `.tmp` file beside PATH. A read-only file or a directory at PATH is
  `error[API_OUTPUT_FAILED]` (exit 1) before any request; a directory the
  file cannot be created in (missing, no write permission) is the same
  error after the request, with the path and the cause only. `--output -`
  is stdout, the same as leaving the option out, and combines with `--json`
  / `--jq`; an empty `--output` is a usage error.
  `--output PATH` with `--json` or `--jq` is `error[API_ARGUMENT_INVALID]`
  (exit 2) before anything is sent: the file takes the body, and the
  envelope or the filter results would have to go elsewhere. A dry run
  accepts them, because `--dry-run --json` prints the plan and writes no
  file; the plan lists the write as a `file_write` effect with
  `performed: false` and `when: "on_success"`. `--output` needs a TARGET.
- `--pages N` fetches up to N pages of a listing; without it a run is one
  request, whatever the response says about a next page. Each next page
  is found one way, named by the caller, never guessed from the
  description: the `Link` header's `rel="next"` (RFC 8288; a relative URL
  is resolved against the request), or with `--cursor PATH=PARAM` the value
  at PATH in the JSON body (dot-separated keys, a number for an array
  index, a leading `.` allowed: `meta.next_cursor`) sent as the query
  parameter PARAM of the first request, in place of any value it had (the
  rest of the query keeps the bytes it was written with). `--pages` takes
  a GET only: any other method, `-X` or the POST `-d` defaults to, is
  `error[API_ARGUMENT_INVALID]` (exit 2) before any request, in a dry run
  too, because each page repeats the request. Each
  later request is the first one with only its URL
  changed, sent through the same credential path: the bearer token (an
  `oauth` 401 still refreshes and retries once, per request), the header
  of a `token` source, or a SigV4 signature over that request. A next page
  off the origin of `base_url` is not requested. Requests are sequential.
  stdout gets one line per page as it arrives: the body as compact JSON (a
  body that is not JSON becomes a JSON string, as in the envelope), with
  `--json` that page's envelope, with `--jq` the filter's results on that
  page (on its envelope with `--json`). The last stderr line says why
  paging stopped: with `--json` or `--jq` one JSON document
  `{"schema_version": 1, "pages": {"fetched", "limit", "stopped", "next",
  "detail"}}` (without `-v`, the only thing on stderr), otherwise `# pages: 3 fetched
  (--pages 3); stopped: ...`. `stopped` is `limit` (N pages were fetched
  and the last of them named one more: `next` is its URL, a TARGET to
  continue from; whether the server has anything there is not known),
  `last` (a response said there is no next page: a `Link` without `next`
  on any page, `null` or `""` at PATH, or a later page without the marker
  earlier ones carried; a separate flag such as `has_more` is not read, so
  an API that keeps the cursor on its last page, such as OpenAI's
  `last_id` beside `has_more: false`, costs one more GET whose empty page
  ends it, and `limit` can name a page that turns out empty),
  `unsupported` (exit 0: the first page carried no `Link` header or nothing
  at PATH, any page's body is not JSON under `--cursor`, or the next page is
  one already requested -- a server that ignores PARAM or echoes the cursor
  would otherwise answer the same page up to the limit; `detail` says which
  -- such a listing may be one page long or page another way, and
  `--cursor` names the other way), or `refused` (exit 0: a next page was
  named and is not requested -- it is off the origin of `base_url`, not a
  URL, or the value at PATH is not a string or a number; `next` is null and
  `detail` says why). A page that fails ends the run as a single request
  would (`API_HTTP_ERROR` exit 4, a timeout `API_REQUEST_FAILED`, a
  `--jq` error `JQ_ERROR`, ...) with its message starting `page N:` and no
  stop report: every line already on stdout is a page fetched whole, and
  with `--json` the failing page's envelope (its `status` outside 2xx)
  follows them. `--pages` needs a TARGET and refuses `--output` (a file
  holds one body); `--cursor` needs `--pages`. `--dry-run` sends nothing
  and adds `# up to N pages: ...` on stderr; its plan adds
  `"pages": {"limit", "follow": "link" | "cursor", "cursor": null |
  {"path", "param"}}` and, for N above 1, a second `api_request` effect
  with `when: "on_next_page"` for the up to N-1 requests that may follow.
- `--shape` and `--sample N` fit a large JSON body into a context; they
  change what is printed, never the request. `--shape` prints the type of
  every value and no value: `"string"`, `"number"`, `"boolean"`, `"null"`,
  an object with its keys, an array as `{"array": COUNT, "of": SHAPE}` (no
  `of` when empty) with its elements' shapes merged (an inner array's COUNT
  is then the total over the elements), and shapes that still differ as a
  JSON array of the alternatives. `--sample N` keeps the first N
  elements of every array at any depth and ends one it cut with
  `{"…": {"omitted": M}}`. A body that is not JSON has the shape `"string"`
  and is not sampled. Both apply to the body before `--json` (whose status
  and headers stay the server's) and `--jq`, and to each page of `--pages`,
  whose next page is still read from the body the server sent. They exclude
  each other and `--output`, which writes the server's bytes: either beside
  it is a usage error with exit code 2.
- A status outside 2xx is `error[API_HTTP_ERROR]: HTTP 404 Not Found: <body
  excerpt>` with exit code 4. With `--json` the envelope is still printed
  on stdout, and stderr is the JSON error document with that code and
  `category: "remote"`. On an `oauth` source a 401 discards the token, gets a new one and
  retries once; on a `token` source nothing was cached and reading the
  reference again would send the same value, so the 401 is the answer.
- `--dry-run` prints the request on stderr with `Authorization: Bearer ****`
  -- or `****` in the header a `kind = "token"` source names -- and sends no
  request to the API. A dry run of a TARGET starts no credential source: no
  token store, authorization server, 1Password or STS call. An operation
  target loads the description to build the request; a URL description is
  fetched (or read from the cache) as usual, but one behind `openapi_auth`
  is only read from the cache -- without a cached copy the dry run is
  `error[API_SPEC_UNAVAILABLE]` (exit 1), and the hint says to run `kurama
  api <name> --refresh-spec` without `--dry-run` first. `--ops`,
  `--describe`, `--schema` and the explorer ignore `--dry-run`.
- `--dry-run --json` prints the plan on stdout as one JSON document (and
  `--jq` filters it); stderr carries only the JSON error document, or with
  `-v` the human dry run too. The plan:
  `{"schema_version": 1, "dry_run": true, "api", "target", "request":
  {"method", "url", "headers": [{"name", "value", "masked"}], "body": null
  | {"bytes", "json"}}, "auth", "description", "effects"}`. Headers are
  masked by name: `Authorization`, `Proxy-Authorization`, `Cookie`,
  `X-Api-Key`, `X-Amz-Security-Token`, `Private-Token`, `X-Auth-Token`,
  `X-Access-Token`, `X-Api-Token` and the header a `kind = "token"` source
  names; a credential passed with `-H` under another name is printed as
  given. The body is its size and whether it is JSON, never its bytes.
  `auth` is `{"mode": "none"}`, `{"mode": "bearer", "kind": "oauth" |
  "token", "source", "header"}` (the credential of an `[auth.*]` source in
  `header`; `grant_type` for `oauth`), `{"mode": "basic", "kind": "token",
  "source", "header"}` (a `username` source), `{"mode": "query", "kind":
  "token", "source", "query"}` (a `query` source; the URL carries `<token>`
  in that parameter) or `{"mode": "sigv4", "aws_profile", "service",
  "region"}`. `description` is `null` when the target needed
  none, else `{"source": {"kind": "file" | "fetched" | "cached" |
  "validated" | "stale" | "unchecked", ...}, "cache_hit", "network"}`,
  where `network` says whether this dry run reached the server for it.
  `effects` lists `{"effect", "performed", "network", "when",
  "may_require_human", "detail"}`: `performed: true` is what the dry run
  did (`description_fetch`, `filesystem`); `performed: false` with `when:
  "always" | "if_needed" | "on_success" | "on_next_page"` is what the run would do (`api_request`,
  `description_fetch`, `token_store`, `authorization_server`, `browser`,
  `1password`, `keychain`, `sts`, `aws_secret_store`, and `file_write` for
  `--output PATH`, after a 2xx only). `filesystem` is only ever a read
  the dry run did (`-d @file`).
  `may_require_human: true` marks a browser or a grant a person answers.
  The plan reads no keychain, so it cannot know whether a usable token is
  stored: an `authorization_server` effect with `when: "if_needed"` and
  `may_require_human: true` can end the run in `OAUTH_LOGIN_REQUIRED`
  (exit 3) without a terminal. A setting the run would stop on (an unknown
  AWS profile, an undecidable SigV4 target, a missing parameter) fails the
  plan with the same `code` and `hint`.
- An operation target is refused before any request when a required
  parameter or the body is missing (`error[API_PARAMETER_MISSING]`, exit 2,
  the missing names listed), when a value does not fit its type or enum, or
  when an unknown parameter is given (`error[API_ARGUMENT_INVALID]`, exit
  2). An operation the description lacks is `error[API_OPERATION_NOT_FOUND]`
  (exit 2) with up to three candidates; `--ops` lists them. Without
  `openapi` under `[api.<name>]`, `--ops`, `--describe`, `--schema`,
  `--skill`, `--refresh-spec` and the explorer are `error[API_SPEC_REQUIRED]` (exit 2). A description
  that cannot be read or fetched (and is not cached) is
  `error[API_SPEC_UNAVAILABLE]` (exit 1); one that is not OpenAPI 3.x /
  Swagger 2.0 (or, under `discovery`, not a Google Discovery Document) is
  `error[API_SPEC_INVALID]` (exit 2).
- A `METHOD /path` that the description does not know is sent as a plain
  request when no `-P` is given, so undocumented paths still work.
- `headers` under `[api.<name>]` go with every request, over the default
  `Accept`; `-H` replaces one of the same name (case does not matter).
  `--dry-run` and `-v` print them. They are signed like any other header
  on an `aws_profile` API. A credential header (the names the plan
  masks, listed under `--dry-run --json`, and the one a `kind = "token"`
  source names) or a
  value that is not printable ASCII on one line is
  `error[CONFIG_INVALID]` (exit 2) with its line.
- An API with `aws_profile` is SigV4-signed with that profile's role
  credentials (the same MFA session cache, 1Password and STS path as
  `kurama exec <profile>`) and sent once: a rejection is
  `error[API_HTTP_ERROR]` (exit 4) with no retry. The service and region
  come from `--service` / `--region`, the profile's `service` / `region`,
  the host (API Gateway, Lambda function URLs, OpenSearch, AppSync,
  `<service>.<region>.amazonaws.com`), then the AWS profile's region. When
  they cannot be told (a custom domain), kurama stops before any STS call
  with `error[API_SIGNING_TARGET_REQUIRED]` (exit 2) and a `hint:` naming
  the `[api.<name>]` keys and the options. `--dry-run` signs with
  placeholder keys: the printed `Authorization` shows the resolved
  `Credential=<access-key-id>/<date>/<region>/<service>/aws4_request` scope
  with `Signature=****`, and `x-amz-security-token: ****`.

