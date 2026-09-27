## `kurama data`

Start with `kurama agent --kind data --json` for the versioned capability
document and request schema. `kurama status --kind data --json` lists configured
workspaces with `connection_status: "not_checked"`; it never reads data or
acquires credentials. A workspace's `sources` register named SQL views.

```bash
kurama data ./events.jsonl --jq '.rows'
kurama data --from ./orders.csv --describe data --json
kurama data --from ./events.jsonl --query 'SELECT count(*) FROM data' --json
kurama data --from ./orders.csv --type 'customer_id=VARCHAR' --preview data --json
kurama data --from 's3://bucket/orders.parquet' --aws-profile ops --region ap-northeast-1 --query 'SELECT sum(amount) FROM data' --json
kurama data lake --request ./query.json --json
kurama data lake --file ./monthly.sql --export ./monthly.parquet --json
```

Choose a configured `[data.<name>]` or `--from PATH_OR_S3_URI` (view name
`data`). Operations: `--tables`, `--describe TABLE`, `--preview TABLE`,
`--summary TABLE`, `--query SQL` or `--file FILE`. Query may add `--explain`
or `--export LOCAL.csv|LOCAL.parquet`. `--request FILE` accepts a strict JSON
request such as `{"operation":"query","args":{"sql":"SELECT count(*) FROM orders"}}`;
only `--request -` reads stdin. `--dry-run` resolves configuration but does
not enumerate or read inputs, acquire credentials, or start DuckDB.

Paths/globs and S3 URIs/HTTPS object URLs also work as the positional input.
Ad-hoc input without an operation defaults to preview (at most 100 rows).
Workspace names still need an operation. `--describe`, `--preview` and
`--summary` may omit TABLE when exactly one source exists. `--jq FILTER`
implies JSON mode and projects the envelope to one JSON value; always check
the exit code because truncation still fails. Use `[.rows[] | ...]` for multiple
projected rows, and `.rows` for the complete returned rows array.

S3 HTTPS inputs are normalized to `s3://`; URL query credentials are discarded.
Use an explicit AWS profile/S3 source. Bucket names and URL credentials never
select authentication. `--region` is optional: without it each bucket is read in
the region S3 reports for it, and `meta.region` says where that was; with it the
named region is used as it is, and a bucket elsewhere is `DATA_REJECTED` naming its region. `meta.region_source` says which: `named`, `profile` (only where the first request is signed, as a dry run reports it) or `bucket`. An S3 301 is a region mismatch, never a permission: the error names the region the request was signed for and the hint the fix. AssumeRole keeps the AWS profile's region. Versioned/partial object URLs and URL keys containing
glob characters are unsupported. Pass a signed URL as `args.from` via
`--request -` to keep it out of argv and shell history; do not persist it.

Inputs support CSV, JSONL/NDJSON (also gzip) and Parquet. JSONL schema is
inferred, nested fields are accessible in SQL, and missing fields become NULL.
Malformed rows fail the query; CSV reader options apply only to CSV.

Results keep ordered `columns` and positional `rows`, including duplicate column
names. Numbers are strings to retain integer/decimal precision. `meta` contains
effective limits and bounded input provenance; consistency is `best_effort`.
`meta.request_count` and `meta.bytes_transferred` are filled for an S3 read
that could be counted end to end and are `null` otherwise, including every
local read; `bytes_transferred` is what moved, never a copy of `input_bytes`.
Naming fewer columns moves fewer bytes on Parquet over S3 as well as locally:
compare the two fields to see how much a projection saved.
A timeout or interrupt adds `elapsed_ms` and `columns` to the JSON error: the
columns the statement names, empty when it names none (`SELECT *`, a full
preview, a summary).
On Parquet, `--describe` adds a `parquet` block next to the logical schema:
files read, row groups (total and the largest per file), rows, and per column
the physical type, compressed and uncompressed bytes, and whether min/max
statistics exist. Read it before writing the query: name the columns you need
rather than selecting every one, and expect a filter on a column without
statistics to read every row group. Other formats have no `parquet` block.
Only one SELECT over the registered inputs/CTEs is accepted. Data SQL does not
accept arbitrary file readers, secret introspection, writes or extensions.

Default query limits: 60 seconds, 1 GB engine memory, two threads, 1,000 result
rows (preview: 100), 8 MiB serialized rows, 1,000 examined input candidates,
10 GiB input bytes, and 10 GB spill. The candidate limit stops every operation.
The input byte limit is object size, not transfer, so it stops only what is
read end to end: a CSV or JSONL scan. `--tables`, `--describe` and `--explain`
read a catalog or a footer, and a local Parquet read takes its footer and the
projected column ranges, so neither is refused for size; the timeout, the
memory limit and the result limits bound those instead. A request that reads
every column anyway -- `--summary`, and `--preview` without `--columns` -- is
bounded on Parquet too. Result limits never reduce aggregation input.
`truncated: true` is partial output and exits 1; do not treat it as complete or
infer a continuation cursor. Explicit export writes the full result atomically,
refuses existing files, and removes incomplete output on failure. JSON mode
emits one structured error on stderr; use its code, exit_code, retry and
next_actions. Engine diagnostics, SQL and cell contents are not echoed in errors.
The data TUI and S3 explorer are not available in this version.

