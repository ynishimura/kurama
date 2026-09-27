# Embedded file analysis

`kurama data` reads local and S3 CSV, JSONL/NDJSON (including gzip) and
Parquet through DuckDB 1.5.3, without importing them into a persistent database
or requiring the DuckDB CLI. From `[s3.<name>]` it takes only the credential
reference and bounded S3 input enumeration; browsing a bucket is `kurama s3`.
`kurama data` has no screen of its own.

```sh
kurama data ./events.jsonl
kurama data 'https://my-bucket.s3.amazonaws.com/orders.parquet' --aws-profile dev
kurama data ./events.jsonl --query 'SELECT count(*) FROM data' --jq '.rows'
kurama data --from ./orders.csv --type 'amount=DECIMAL(18,2)' --describe data --json
kurama data --from ./events.jsonl --query 'SELECT event.name, count(*) FROM data GROUP BY event.name' --json
kurama data --from './orders/*.parquet' --preview data --columns amount --max-rows 20 --json
kurama data --from 's3://my-bucket/month/*.parquet' --aws-profile dev --region ap-northeast-1 --query 'SELECT sum(amount) FROM data' --json
kurama data lake --file ./monthly.sql --export ./monthly.parquet --json
kurama agent --kind data --json
kurama status --kind data --json
```

Named workspaces can join multiple sources. Relative workspace paths require
an absolute `root_dir`; ad-hoc, SQL and export paths are relative to the caller.

A positional argument containing `/` or ending in a supported file extension is
shorthand for `--from`; other names select workspaces. Put the input/workspace
before an option whose TABLE is omitted. Ad-hoc input without an operation
previews at most 100 rows. Workspaces still need an explicit operation.
`--describe`, `--preview` and `--summary` may omit TABLE when there is exactly
one source, including in a JSON request. Multiple sources require a table name.
`--dry-run` without an operation remains a static input plan.

Ad-hoc S3 HTTPS object URLs (standard virtual-hosted or path-style, global or
regional endpoints) are converted to `s3://bucket/key`. Query and fragment are
discarded, and percent-encoded keys are decoded once without changing `+`,
repeated slashes or dot segments. URL credentials are never used for requests
or included in result metadata. `versionId`, `partNumber` and URL keys containing
glob characters are refused. Use `s3://` for explicit input globs; other HTTPS
services, website endpoints and custom domains are unsupported.

S3 still requires `--aws-profile` or `--s3-source`; bucket names never choose
credentials. A region named by `--region`, the workspace, `[s3.*]` or a regional
HTTPS input host is used as it is. Without one, the AWS profile's region is only
where the first request is signed: S3 answers a request routed to the wrong
region with the bucket's own in `x-amz-bucket-region`, and kurama retries that
request there once. The remaining HEAD/LIST calls and the httpfs secret of that
bucket then use the region S3 named, so a bucket outside the profile's region is
read where it lives instead of failing with `PermanentRedirect` or crossing
regions. A named region is never corrected: the 301 becomes `DATA_REJECTED`
naming both regions, with a hint to use the bucket's. A 301 without
`x-amz-bucket-region` cannot be followed and becomes `DATA_REJECTED` naming the
region the request was signed for, with a hint to set the bucket's region; no
301 is reported as a permission. AssumeRole keeps the AWS profile's
region: where STS is called is not where a bucket lives. `meta.region` reports
where the buckets were read, comma-separated when they live in several regions,
and `meta.region_source` whether that region was `named`, is the `profile`'s
(where the first request is signed, not yet confirmed: what `--dry-run` shows)
or what S3 answered for each `bucket`. Pasting a signed URL into a command can put it
in shell history and argv before kurama runs. Prefer `--request -` with `args.from`
passed through stdin; do not save signed URLs in config, request files or logs.

`--jq FILTER` implies JSON mode and applies to the result envelope (including a
dry-run plan). It must produce exactly one JSON value; use an array expression
for multiple outputs. Strings remain JSON strings. For example, `--jq '.rows'`
returns the rows alone. Projection errors emit one sanitized `DATA_INVALID`
error with no stdout. Incomplete queries and byte-limited results still exit 1 even if the projection
omits truncation metadata; a row-limited preview is a successful window (exit 0). Query result limits still apply
before projection, and failed projection does not publish an export file.

JSONL inputs contain one JSON object per line. Extensions `.jsonl`, `.ndjson`,
`.jsonl.gz` and `.ndjson.gz` select the JSONL reader; named sources can set
`format = "jsonl"` for other filenames. DuckDB infers columns and nested
structures; missing fields become NULL. Use field access such as `event.name`
and SQL casts for conversions. CSV reader options (`types`, `header`,
`delimiter`, `date_format`, `timestamp_format`) apply only to CSV. Malformed
JSONL rows fail the query. Export formats remain CSV and Parquet.

```toml
[s3.assets]
aws_profile = "dev"
region = "ap-northeast-1"

[data.lake]
root_dir = "/absolute/path/datasets"
s3_source = "assets"

[[data.lake.sources]]
name = "orders"
path = "s3://my-bucket/month/*.parquet"

[[data.lake.sources]]
name = "customers"
path = "customers.csv"
types = { customer_id = "VARCHAR" }
```

```sh
kurama data lake --query 'SELECT c.region, sum(o.amount) AS sales FROM orders o JOIN customers c USING (customer_id) GROUP BY c.region' --json
```

Operations are `--tables`, `--describe TABLE`, `--preview TABLE`,
`--summary TABLE`, or `--query SQL` / `--file SQL_FILE`. Only queries accept
`--explain` or `--export`. Summary is DuckDB's whole-input `SUMMARIZE` operation
(count, min/max, null percentage and its other statistics); it is never automatic.
Preview defaults to 100 rows. `--request FILE` and `--request -` accept the same
typed operations; only `-` reads stdin. They cannot be mixed with operation
arguments. The capability document separates `request_operations` from `result_operations`;
explain and export are modifiers of a query request. Request and result schemas
are generated from the same Rust types used at execution. Resource defaults
use the request field names, including `query_timeout_secs`.

```json
{"operation":"query","args":{"sql":"SELECT count(*) AS count FROM orders","max_rows":100}}
```

SQL uses registered source views and CTEs. Single read-only SELECT statements,
aggregates, JOINs, windows, and pure `range` / `generate_series` / `unnest`
table functions are supported. DuckDB parses the statement and its table
references with CTE scopes and declaration order preserved; catalog inspection
(`SHOW`, `DESCRIBE` inside user SQL) and unknown table reference classes are
refused by an allowlist. Arbitrary table functions (including secret introspection and
dynamic SQL), DDL, DML, COPY, INSTALL, LOAD and ATTACH are rejected. Internal
export and view creation use separate paths. Only the resolved exact input
files and an export's temporary file are allowed external paths. This is not
an OS sandbox for hostile SQL or hostile file formats.

Result columns remain ordered, including duplicate names. Numeric values are
strings to preserve integer and DECIMAL precision; null is JSON null, booleans
are booleans, binary is base64, and nested values are JSON strings with numeric
leaves also represented as strings. Column `data_type` uses Arrow's type spelling,
except that unsigned 128-bit integer columns retain the DuckDB name `UHUGEINT`.
Non-JSON output escapes terminal control characters.

`--describe` on a Parquet source adds a `parquet` block to the result: the
number of objects whose footer was read (and how many were left out), the row
groups in total and the largest count in one object, the rows, and one entry
per column with its Parquet physical type, compressed and uncompressed bytes
and whether any row group carries min/max statistics. Footers are read for at
most 100 objects, the same bound the `meta.inputs` list uses. The logical type
stays in the describe rows. Text mode adds one line under the column table;
the per-column detail is JSON only. Other formats have no `parquet` block.
`meta.next_actions` names the column that dominates the read and the columns a
filter cannot skip row groups on. The Parquet metadata table functions are
reachable only from this path: the SQL validator's allowlist is unchanged, so
`--query 'DESCRIBE data'` and `parquet_metadata(...)` in user SQL are still
refused.

## Limits and completion

Defaults: 1 GB DuckDB memory, two threads, 60 seconds, 1,000 result rows,
8 MiB serialized result rows (minimum 2 bytes for the JSON array), 1,000 examined input candidates and 10 GiB of
selected input sizes. Spill is enabled with a 10 GB limit. All are configurable
under `[data.*]` or with the corresponding CLI options shown by `data --help`.
The integer component of `memory_limit` and `max_temp_directory_size` must be
in `1..=18446744073709551615` (u64), followed by a supported size unit.
The published schema checks the syntax with a pattern and documents this numeric
bound in each field's description; runtime validation rejects larger integers.
`max_rows` and `max_result_bytes` bound returned rows, not scan volume; provenance
and column metadata are additional. No result limit is inserted into the user's SQL.

`max_source_objects` bounds enumeration for every operation: a glob wider than
it stops while the candidates are being listed, whatever is asked of them.
`max_source_bytes` is different. Object size is not transfer and not cost, so
it bounds only the objects an operation reads end to end:

<!-- checked against `DataRequest::spends_byte_budget` by tests/architecture/ -->

| Operation | CSV / JSONL | Parquet |
| --- | --- | --- |
| `--tables`, `--describe`, `--explain` | no | no |
| `--preview TABLE --columns NAME`, `--query`, `--export` | yes | no |
| `--preview TABLE` without `--columns`, `--summary` | yes | yes |

A row format is read from the first byte to the last, so its object size is its
transfer whatever is asked of it. A columnar one is refused for its size only
when the request is known to read every column: `SUMMARIZE` computes statistics
for all of them, and a preview without `--columns` selects all of them.
Arbitrary SQL is not assumed to read every column -- refusing it for size would
refuse `count(*)`, which reads statistics only -- so `--query` and `--export`
on Parquet are bounded by the query timeout, the memory limit and the result
limits instead. `--dry-run` never reaches the check: it reads no inputs at all.

So `--describe` and `count(*)` work on a Parquet object larger than the budget,
while the same budget still refuses a full CSV scan and a `--summary` of the
same Parquet.

The projection saving is real over S3, not only for a local file: reading one
column of a 1,031,140-byte object measures 97,577 bytes transferred across 15
requests (`data_s3_requests_and_transferred_bytes_are_measured`). What the
exemption does not cover is a request that reads every column anyway, which is
why `--summary` is in the row above. `meta.bytes_transferred` is what says
which of the two happened on a given run.

Result limits produce `truncated: true` and a stop reason. A preview stopped
by `max_rows` succeeds with exit 0; byte limits and incomplete non-preview
results exit 1. Arbitrary SQL
has no continuation cursor. Export writes the complete query result without the
display limit, refuses existing destinations, and publishes with no overwrite
only after execution and input checks succeed. Errors leave no completed export.

The input/query deadline starts after MFA and AssumeRole complete, so
authentication keeps its own timeout and error category. Ctrl+C and timeout interrupt the engine, and the worker is joined before exit.
No query is automatically retried. A late fetch error is checked separately from
end-of-results. Driver error strings are not exposed because they can contain
SQL, cells or credentials.

JSON mode emits at most one stdout result and one stderr error document, with
no progress/log lines. Existing commands retain their text error format. Data
errors use `DATA_INVALID` (2), `DATA_FAILED` (1), `DATA_REJECTED` (4); existing
profile, configuration, authentication and AWS errors retain their existing
codes and exit categories. JSON errors include the cause chain, actionable
hint where available and a matching next action; no retry is automatic. `status --kind data` reads
configuration only and reports `connection_status: not_checked` / unknown auth.
While the engine runs, a person watching gets one line on stderr that is
rewritten once a second: `# scanning 12s`, ended before the result or the
error. Only the elapsed seconds; how far along a scan is cannot be observed,
so no percentage, estimate or byte count is invented. It appears only when
stderr is a terminal and the mode is not `--json` or `--jq`; a pipe, CI and
JSON mode get nothing, not even a carriage return. Credential acquisition has
its own output and is not counted here.

`meta.request_count` and `meta.bytes_transferred` are filled only for a run
that read S3, and only when both halves could be counted: the SDK's HEAD and
LIST calls (redirect retries and the final change check included) and the
engine's own reads, which are counted from the HTTP log it keeps in memory for
the session. A local run transfers nothing and leaves both `null` rather than
reporting zero. `bytes_transferred` is the object bytes the engine asked for --
a range `GET` by the range, a whole-object `GET` by its `Content-Length`; a
HEAD carries no body and counts only as a request, and the bodies of HEAD and
LIST responses are not measured. `input_bytes` is never copied into it: one is
the size of the objects selected, the other is what moved, and comparing them
is the point. If the engine's log cannot be read both stay `null`.

Accounting is not free: DuckDB keeps the HTTP log in an in-memory store that
`memory_limit` does not count and `allow_spill` cannot relieve, one row per
request holding the URL and every header. A scan issuing tens of thousands of
range reads therefore adds memory the configured limits do not see. It is only
enabled for an S3 run, and the session is dropped as soon as the numbers are
read.

A wait that ends early says what it was doing. The JSON error of a timeout or
an interrupt carries `elapsed_ms` and `columns`: the column names the statement
references, taken from the parsed statement, with qualifiers dropped. A query
that names none of them -- `SELECT *`, a full preview, a summary -- reports an
empty list, and so does a query that never reached validation. Column names are
schema, not content: no SQL text, literal or cell value is echoed.

`--dry-run` reads configuration and, for S3 region fallback, the AWS profile;
it does not enumerate/read input files, start DuckDB or acquire credentials.

Spill and the extracted extension live in a private `kurama-data-*` directory
under the process temporary directory (`TMPDIR` on macOS). Normal completion
and handled interruption remove it. After a crash, remove the particular
abandoned directory when no matching operation is running. The engine memory
limit is not a strict process RSS limit. Input size is not a transfer/cost
limit, and bounds only the operations and formats named under "Limits and
completion".

S3 reads use one explicit role credential set from the existing MFA/AssumeRole
path. There is no inherited AWS credential fallback, `aws` extension, persistent
secret, or automatic credential refresh/retry. Each bucket gets an in-memory
CONFIG secret; credentials never enter argv, result output or files. Temporary
secret SQL buffers on the Rust side are zeroized on drop; DuckDB may keep its
own in-memory copies. Temporary secret scope selects credentials; IAM remains the permission boundary.

Known keys use HEAD without LIST. In local and S3 globs, `*` stays within one
path component; use `**` to span directories. Resolved filenames containing
`*`, `?` or `[` are refused because DuckDB would expand them again.
Globs list the fixed prefix with a bound on
all candidates, even those that do not match. Exceeding a bound aborts before
any analysis. Input size/ETag/version (where observed) and timestamps are
reported; at most 100 input records are returned with an omitted count. A final
HEAD detects changes before output/export publication. This is `best_effort`,
not a transaction across objects or version-pinned reads. Transfer/request
measurements are null because they are not measured by the production adapter.

## Building and updating DuckDB/httpfs

`libduckdb-sys = 1.10503.0` bundles DuckDB 1.5.3 with CSV, Parquet and JSON.
`build.rs` fetches the official signed httpfs archive for that exact engine ABI
and target, verifies the pinned SHA-256, and embeds the archive in the binary.
Supported build targets are macOS arm64/x86_64 and Linux arm64/x86_64 GNU. Building
requires the normal C++ toolchain and curl. No extension download or lookup of
the user's extension directory happens at query time. DuckDB also verifies the
extension signature/version on loading; unsigned extensions remain disabled.

Downloads use HTTPS-only redirects and a staging file; only a verified archive
is renamed into the versioned cache. A bad cached hash triggers a fresh download.
The embedded extension version is tested against the linked engine.

For an offline build, supply the matching archive via `KURAMA_HTTPFS_ARCHIVE`.
The same hash verification applies. To update: use `cargo add` for the engine,
update the version URL and per-target hashes from official HTTPS artifacts,
and rerun the native and signed S3 tests from an empty extension environment
on every supported platform. Do not update the library without its extension.

The code uses DuckDB's C statement/chunk API through `libduckdb-sys`, with
owned handles and Arrow C interchange, because the higher-level API can execute
preceding statements when preparing a multi-statement string and does not expose
late fetch errors in the required way. Arrow interchange has a small C++
`noexcept` bridge because the bundled C API has allocations outside its catch
blocks. Reviewed entry points are enforced by an architecture test; owned
handles release schemas, options, arrays and errors. Tests cover late errors,
cancellation, scoped SQL, result precision and bounded row conversion.
The result byte limit bounds serialized row conversion before values are
materialized in Rust; it does not cap Arrow chunks already owned by DuckDB.

Validation here uses the native engine on macOS arm64 and signed S3/STS fakes.
It does not establish real AWS/SSE-KMS behavior, release compatibility on the
other targets, a 10-million-row performance budget, or real spill performance.
Those and the TUI remain follow-up acceptance items for the complete issue.

Primary references: [DuckDB Rust client](https://duckdb.org/docs/current/clients/rust/overview),
[JSON loading](https://duckdb.org/docs/current/data/json/loading_json),
[S3 support](https://duckdb.org/docs/current/core_extensions/httpfs/s3api),
[S3 URL forms](https://docs.aws.amazon.com/AmazonS3/latest/userguide/VirtualHosting.html),
[external access and secrets](https://duckdb.org/docs/current/operations_manual/securing_duckdb/overview).
