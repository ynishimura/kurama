//! The audit entry of this process: opened when `api`, `exec`, `db` or `data` starts, filled in as the call learns its request and status, and appended once when it ends.
//!
//! `[audit] enabled` decides whether a run is recorded: absent, only a run
//! under `KURAMA_AGENT` is. The log is a record, never a requirement: an
//! entry that cannot be written is one warning on stderr, and the command's
//! own exit code stands. What an entry may hold is `AuditEntry`: the URL is
//! cut to its path before it is noted, and nothing else of a request is.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

use crate::adapters::audit_log::{append_entry, audit_file};
use crate::adapters::config::Config;
use crate::console::progress;
use crate::domain::functions::audit::{path_of, sql_fingerprint};
use crate::domain::types::audit::AuditEntry;

/// What `begin` knows, completed by the `note_*` calls.
struct Pending {
    entry: AuditEntry,
    started: Instant,
    file: PathBuf,
    max_bytes: u64,
}

static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

fn with_pending(update: impl FnOnce(&mut AuditEntry)) {
    if let Some(pending) = PENDING.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        update(&mut pending.entry);
    }
}

/// What one command is, for its entry.
pub struct Call<'a> {
    pub command: &'static str,
    pub target: &'a str,
    pub program: Option<&'a str>,
}

/// Open the entry of this run when `[audit]` records it.
pub fn begin(call: Call<'_>, config: &Config, agent_run: bool) {
    if !config.audit.enabled.unwrap_or(agent_run) {
        return;
    }
    let file = match audit_file() {
        Ok(file) => file,
        Err(error) => return warn(&error),
    };
    let entry = AuditEntry {
        time: chrono::Utc::now(),
        command: call.command.to_owned(),
        target: call.target.to_owned(),
        agent: agent_run,
        method: None,
        path: None,
        status: None,
        program: call.program.map(str::to_owned),
        sql_sha256: None,
        exit_code: None,
        error_code: None,
        duration_ms: 0,
    };
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending {
        entry,
        started: Instant::now(),
        file,
        max_bytes: config.audit.max_bytes,
    });
}

/// The request `api` is about to send (or refuse): its method and its path,
/// the query cut off here.
pub fn note_request(method: &str, url: &str) {
    with_pending(|entry| {
        entry.method = Some(method.to_owned());
        entry.path = Some(path_of(url));
    });
}

/// The SQL `db` or `data` runs, kept as its fingerprint only.
pub fn note_sql(sql: &str) {
    with_pending(|entry| entry.sql_sha256 = Some(sql_fingerprint(sql)));
}

/// The status of the last response.
pub fn note_status(status: u16) {
    with_pending(|entry| entry.status = Some(status));
}

/// Append the entry with the exit code the process ends with (`None` when
/// kurama is about to replace itself with an `exec` command) and the code of
/// the error it failed with.
pub fn finish(exit_code: Option<u8>, error_code: Option<&str>) {
    let Some(mut pending) = PENDING.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    pending.entry.exit_code = exit_code;
    pending.entry.error_code = error_code.map(str::to_owned);
    pending.entry.duration_ms = pending.started.elapsed().as_millis() as u64;
    if let Err(error) = append_entry(&pending.file, &pending.entry, pending.max_bytes) {
        warn(&error);
    }
}

fn warn(error: &dyn std::fmt::Display) {
    progress!("# warning: the audit log could not be written: {error}");
}
