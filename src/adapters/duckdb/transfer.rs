//! What httpfs actually moved, read back from the engine's own HTTP log.
use super::{connection::Connection, result};
use crate::domain::types::dataset::DataLimits;

/// Requests httpfs made and the object bytes they carried.
///
/// A HEAD carries no body and counts only as a request. A range GET is
/// measured from the range it asked for; a whole-object GET from the
/// `Content-Length` it was answered with.
pub struct Transfer {
    pub requests: u64,
    pub bytes: u64,
}

/// Collect HTTP log records in memory. Nothing else is logged.
///
/// The log holds each request verbatim, headers included, so for S3 it also
/// holds the `Authorization` and `x-amz-security-token` of every signed
/// request, in buffers this crate does not zeroize. It is never written
/// anywhere: `lock_configuration` pins the storage to memory, the SQL
/// validator's allowlist keeps `duckdb_logs` out of user SQL, and the whole
/// session is dropped after `read`. One row per request is also memory DuckDB
/// does not count against `memory_limit`; see `docs/development/data.md`.
pub(super) fn start_logging(connection: &Connection) -> Result<(), super::DataError> {
    connection.execute(
        "SET logging_storage='memory'; \
         SET enabled_log_types='HTTP'; \
         SET logging_mode='ENABLE_SELECTED'; \
         SET logging_level='TRACE'; \
         SET enable_logging=true",
        "HTTP accounting",
    )
}

/// The aggregate, or `None` when it cannot be read whole.
///
/// The engine decides what its log looks like, and a number nobody measured is
/// worse than none: a request whose byte count cannot be determined -- no
/// closed range and no `Content-Length` -- makes the whole figure `None`
/// rather than counting as zero bytes moved.
pub(super) fn read(connection: &Connection, limits: &DataLimits) -> Option<Transfer> {
    const RANGE: &str = "^bytes=([0-9]+)-([0-9]+)$";
    let sql = format!(
        "WITH http AS (SELECT request.\"type\" AS method, \
           regexp_extract(coalesce(request.headers['Range'], ''), '{RANGE}', 1) AS range_from, \
           regexp_extract(coalesce(request.headers['Range'], ''), '{RANGE}', 2) AS range_to, \
           try_cast(response.headers['Content-Length'] AS BIGINT) AS length \
         FROM duckdb_logs_parsed('HTTP')) \
         SELECT count(*), \
           coalesce(sum(CASE \
             WHEN method = 'HEAD' THEN 0 \
             WHEN range_to <> '' THEN CAST(range_to AS BIGINT) - CAST(range_from AS BIGINT) + 1 \
             ELSE length END), 0), \
           count(*) FILTER (method <> 'HEAD' AND range_to = '' AND length IS NULL) \
         FROM http"
    );
    let rows = match result::collect_complete(connection, &sql, limits) {
        Ok(rows) => rows,
        Err(error) => {
            tracing::debug!(%error, "the engine's HTTP log could not be read");
            return None;
        }
    };
    let row = rows.first()?;
    let unmeasured = result::number(row.get(2))?;
    if unmeasured > 0 {
        tracing::debug!(unmeasured, "some HTTP requests carried no measurable size");
        return None;
    }
    Some(Transfer {
        requests: result::number(row.first())?,
        bytes: result::number(row.get(1))?,
    })
}
