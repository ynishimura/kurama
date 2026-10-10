# Data analysis flow

How `kurama data` plans, enumerates and runs a query over local or S3 files.

Data analysis: `dispatch.rs` -> `commands/data.rs`, with typed plans and
output in `data_plan.rs` / `data_render.rs`, bounded local/S3 enumeration
in `adapters/data_inputs.rs` / `adapters/aws/s3_data.rs`, and ephemeral
execution in `adapters/duckdb/`. SQL validation preserves CTE scope and
declaration order; Arrow C interchange uses reviewed entry points and a
small exception-catching bridge. Each bucket is read where it lives: a
region the request names (`--region`, the workspace, `[s3.*]`, a regional
S3 URL host) is used as it is, and otherwise the AWS profile's region only
signs the first request, which S3 corrects once through
`x-amz-bucket-region`; the later HEAD/LIST calls and that bucket's httpfs
secret follow it, while AssumeRole stays in the profile's region.
`max_source_bytes` bounds only what an operation reads end to end, so a
metadata read and a Parquet scan are not refused for size; `--describe` on
Parquet adds the footer facts (`duckdb/parquet_summary.rs`), an S3 run
reports the requests and bytes it moved (SDK counters plus the engine's own
HTTP log, `duckdb/transfer.rs`), a terminal gets the elapsed seconds, and a
cancelled wait names the columns the statement referenced.
Files: `cargo xtask map local-analytics`.
