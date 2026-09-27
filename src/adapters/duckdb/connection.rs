//! Owned DuckDB C handles: single statements, true chunk streaming, checked fetch errors and interrupt.
use super::{handles::*, sql};
use crate::domain::types::dataset::{DataError, InvalidInput, ScanContext};
use arrow::{
    array::StructArray,
    ffi::{FFI_ArrowArray, FFI_ArrowSchema, from_ffi},
};
use libduckdb_sys as ffi;
use std::{
    ffi::CStr,
    ptr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

struct InterruptPointer(ffi::duckdb_connection);
// DuckDB explicitly permits interrupt from another thread. The mutex ensures
// the owning worker cannot disconnect while the pointer is being used.
unsafe impl Send for InterruptPointer {}

#[derive(Default)]
pub struct Cancellation {
    connection: Mutex<Option<InterruptPointer>>,
    cancelled: AtomicBool,
}
impl Cancellation {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(pointer) = self.connection.lock().unwrap().as_ref() {
            // SAFETY: a live connection is registered until its owner drops.
            unsafe { ffi::duckdb_interrupt(pointer.0) };
        }
    }
    pub fn check(&self) -> Result<(), DataError> {
        if self.cancelled.load(Ordering::SeqCst) {
            Err(DataError::Interrupted(ScanContext::default()))
        } else {
            Ok(())
        }
    }
}

pub struct Connection {
    database: ffi::duckdb_database,
    connection: ffi::duckdb_connection,
    cancel: Arc<Cancellation>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        let mut pointer = self.cancel.connection.lock().unwrap();
        *pointer = None;
        // SAFETY: handles belong to this object; every result is already dropped.
        unsafe {
            ffi::duckdb_disconnect(&mut self.connection);
            ffi::duckdb_close(&mut self.database);
        }
    }
}

impl Connection {
    pub fn open(settings: &[(&str, String)], cancel: Arc<Cancellation>) -> Result<Self, DataError> {
        cancel.check()?;
        let mut config = Config(ptr::null_mut());
        // SAFETY: C out-pointers are initialized and all owned handles have Drop.
        unsafe {
            if ffi::duckdb_create_config(&mut config.0) != ffi::DuckDBSuccess {
                return Err(DataError::Engine("configuration"));
            }
            for (key, value) in settings {
                if ffi::duckdb_set_config(
                    config.0,
                    cstring(key)?.as_ptr().cast(),
                    cstring(value)?.as_ptr().cast(),
                ) != ffi::DuckDBSuccess
                {
                    return Err(InvalidInput::EngineSetting {
                        name: (*key).into(),
                    }
                    .into());
                }
            }
            let mut db = Self {
                database: ptr::null_mut(),
                connection: ptr::null_mut(),
                cancel,
            };
            let mut error = ptr::null_mut();
            let status = ffi::duckdb_open_ext(ptr::null(), &mut db.database, config.0, &mut error);
            ffi::duckdb_free(error.cast());
            if status != ffi::DuckDBSuccess {
                return Err(DataError::Engine("engine startup"));
            }
            if ffi::duckdb_connect(db.database, &mut db.connection) != ffi::DuckDBSuccess {
                return Err(DataError::Engine("connection"));
            }
            *db.cancel.connection.lock().unwrap() = Some(InterruptPointer(db.connection));
            db.cancel.check()?;
            Ok(db)
        }
    }

    pub fn version() -> String {
        // SAFETY: library_version returns a static, NUL-terminated string.
        unsafe {
            CStr::from_ptr(ffi::duckdb_library_version())
                .to_string_lossy()
                .into_owned()
        }
    }

    pub fn execute(&self, sql: &str, stage: &'static str) -> Result<(), DataError> {
        self.cancel.check()?;
        let mut result = ResultSet::new();
        let status = unsafe {
            ffi::duckdb_query(
                self.connection,
                cstring(sql)?.as_ptr().cast(),
                &mut result.0,
            )
        };
        if status != ffi::DuckDBSuccess {
            return Err(result_error(&mut result, stage));
        }
        Ok(())
    }

    fn prepare(&self, sql: &str) -> Result<Statement, DataError> {
        self.cancel.check()?;
        let mut extracted = Extracted(ptr::null_mut());
        // Unlike duckdb-rs prepare, do not execute preceding statements.
        let count = unsafe {
            ffi::duckdb_extract_statements(
                self.connection,
                cstring(sql)?.as_ptr().cast(),
                &mut extracted.0,
            )
        };
        if count != 1 {
            return Err(DataError::Sql);
        }
        let mut statement = Statement(ptr::null_mut());
        let status = unsafe {
            ffi::duckdb_prepare_extracted_statement(
                self.connection,
                extracted.0,
                0,
                &mut statement.0,
            )
        };
        if status != ffi::DuckDBSuccess {
            // Binding failures are invalid SQL. Never read driver diagnostics:
            // the pointer can be null, and the text can contain secrets or cells.
            return Err(DataError::Sql);
        }
        Ok(statement)
    }

    /// The parsed statement, for the caller that wants what it references.
    pub fn validate_select(
        &self,
        sql: &str,
        tables: &[String],
    ) -> Result<serde_json::Value, DataError> {
        // Inspect DuckDB's parsed tree before binding user SQL. Only registered
        // views/CTEs and pure table generators are accepted; secret introspection,
        // query('...') and direct file readers are not analysis inputs.
        let query = format!(
            "SELECT json_serialize_sql({})",
            crate::domain::types::dataset::sql_string(sql)
        );
        let mut result = ResultSet::new();
        if unsafe {
            ffi::duckdb_query(
                self.connection,
                cstring(&query)?.as_ptr().cast(),
                &mut result.0,
            )
        } != ffi::DuckDBSuccess
        {
            return Err(DataError::Sql);
        }
        let tree = unsafe {
            let text = ffi::duckdb_value_varchar(&mut result.0, 0, 0);
            if text.is_null() {
                return Err(DataError::Sql);
            }
            let tree = serde_json::from_slice::<serde_json::Value>(CStr::from_ptr(text).to_bytes());
            ffi::duckdb_free(text.cast());
            tree.map_err(|_| DataError::Sql)?
        };
        if tree["error"] != false || tree["statements"].as_array().is_none_or(|s| s.len() != 1) {
            return Err(DataError::Sql);
        }
        let names: std::collections::BTreeSet<String> =
            tables.iter().map(|s| s.to_lowercase()).collect();
        if !sql::allowed_tables(&tree, &names) {
            return Err(DataError::Sql);
        }
        let statement = self.prepare(sql)?;
        if unsafe { ffi::duckdb_prepared_statement_type(statement.0) }
            != ffi::duckdb_statement_type_DUCKDB_STATEMENT_TYPE_SELECT
        {
            return Err(DataError::Sql);
        }
        Ok(tree)
    }

    /// Visit a chunk at a time; false stops fetching. An empty final fetch is
    /// success only if the engine has no stored error (including late CSV errors).
    pub fn stream(
        &self,
        sql: &str,
        mut visit: impl FnMut(&StructArray) -> Result<bool, DataError>,
    ) -> Result<FFI_ArrowSchema, DataError> {
        let statement = self.prepare(sql)?;
        let mut result = ResultSet::new();
        if unsafe { ffi::duckdb_execute_prepared_streaming(statement.0, &mut result.0) }
            != ffi::DuckDBSuccess
        {
            return Err(result_error(&mut result, "query execution"));
        }
        let (schema, options) = result_schema(&mut result)?;
        loop {
            self.cancel.check()?;
            let chunk = Chunk(unsafe { ffi::duckdb_fetch_chunk(result.0) });
            if chunk.0.is_null() {
                if !unsafe { ffi::duckdb_result_error(&mut result.0) }.is_null() {
                    return Err(result_error(&mut result, "result fetch"));
                }
                break;
            }
            let mut array = FFI_ArrowArray::empty();
            if !unsafe {
                kurama_arrow_array(
                    options.0,
                    chunk.0,
                    (&mut array as *mut FFI_ArrowArray).cast(),
                )
            } {
                return Err(DataError::Engine("Arrow conversion"));
            }
            // SAFETY: DuckDB exports the Arrow C interface with its release callback;
            // from_ffi takes ownership and preserves the buffers' lifetime.
            let data = unsafe { from_ffi(array, &schema) }
                .map_err(|_| DataError::Engine("Arrow conversion"))?;
            if !visit(&StructArray::from(data))? {
                break;
            }
        }
        Ok(schema)
    }
}

fn result_error(result: &mut ResultSet, stage: &'static str) -> DataError {
    match unsafe { ffi::duckdb_result_error_type(&mut result.0) } {
        ffi::duckdb_error_type_DUCKDB_ERROR_PARSER | ffi::duckdb_error_type_DUCKDB_ERROR_BINDER => {
            DataError::Sql
        }
        ffi::duckdb_error_type_DUCKDB_ERROR_INTERRUPT => {
            DataError::Interrupted(ScanContext::default())
        }
        // A remote HTTP error could be a transport failure or a service rejection;
        // classify a known status without returning any driver text.
        ffi::duckdb_error_type_DUCKDB_ERROR_HTTP => {
            let text = unsafe { ffi::duckdb_result_error(&mut result.0) };
            if text.is_null() {
                return DataError::S3Failed;
            }
            let text = unsafe { CStr::from_ptr(text) }.to_string_lossy();
            if contains_http_status(&text, "403") {
                DataError::S3Rejected("AccessDenied or expired credentials".into())
            } else if contains_http_status(&text, "404") {
                DataError::S3Rejected("NoSuchKey".into())
            } else {
                DataError::S3Failed
            }
        }
        _ => DataError::Engine(stage),
    }
}

fn contains_http_status(text: &str, status: &str) -> bool {
    text.contains(&format!("HTTP {status}")) || text.contains(&format!("({status})"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn data_engine_http_status_does_not_classify_object_names() {
        assert!(!contains_http_status(
            "connection failed for s3://bucket/2024/403/data.parquet",
            "403"
        ));
        assert!(contains_http_status("request failed with HTTP 403", "403"));
        assert!(contains_http_status("Forbidden (403)", "403"));
    }
}
