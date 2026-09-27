//! The audit log on disk: `~/.local/state/kurama/audit.jsonl`, one JSON line per call, readable by its owner only, moved to `audit.jsonl.1` once it reaches its size bound.

use std::io::{self, BufRead, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::adapters::error::CoreResult;
use crate::adapters::utils::path::get_home_dir;
use crate::domain::types::audit::AuditEntry;

/// The current log; the previous generation is this path with `.1`.
pub fn audit_file() -> CoreResult<PathBuf> {
    Ok(get_home_dir()?
        .join(".local")
        .join("state")
        .join("kurama")
        .join("audit.jsonl"))
}

fn previous(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".1");
    PathBuf::from(name)
}

/// Append one entry, creating the file (mode 600) and its directory. A file
/// that has reached `max_bytes` first replaces the previous generation, so
/// the two together hold at most about twice the bound.
pub fn append_entry(path: &Path, entry: &AuditEntry, max_bytes: u64) -> io::Result<()> {
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)?;
    }
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.len() >= max_bytes => std::fs::rename(path, previous(path))?,
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut line = serde_json::to_string(entry).map_err(io::Error::other)?;
    line.push('\n');
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?
        .write_all(line.as_bytes())
}

/// What changes whenever an entry is appended or the log rotates: the
/// current file's length and modification time; `None` while there is none.
pub fn log_version(path: &Path) -> Option<(u64, std::time::SystemTime)> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.len(), metadata.modified().ok()?))
}

/// Every entry of the previous generation, then of the current one; no file
/// is no entry, and a line that is not an entry (cut by a crash, written by
/// a later version) is skipped.
pub fn read_entries(path: &Path) -> io::Result<Vec<AuditEntry>> {
    let mut entries = Vec::new();
    for file in [previous(path), path.to_owned()] {
        let file = match std::fs::File::open(&file) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for line in io::BufReader::new(file).lines() {
            if let Ok(entry) = serde_json::from_str(&line?) {
                entries.push(entry);
            }
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(target: &str) -> AuditEntry {
        AuditEntry {
            time: chrono::DateTime::from_timestamp(1_790_000_000, 0).unwrap(),
            command: "api".into(),
            target: target.into(),
            agent: true,
            method: Some("GET".into()),
            path: Some("/pets".into()),
            status: Some(200),
            program: None,
            sql_sha256: None,
            exit_code: Some(0),
            error_code: None,
            duration_ms: 3,
        }
    }

    #[test]
    fn entries_read_back_in_order_across_a_rotation_and_a_broken_line_is_skipped() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state").join("audit.jsonl");
        assert_eq!(read_entries(&path).unwrap(), Vec::new(), "no file yet");
        append_entry(&path, &entry("a"), 1 << 20).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"time\": \"cut\n")
            .unwrap();
        // The file is past a bound of 10 bytes, so `b` starts a new one.
        append_entry(&path, &entry("b"), 10).unwrap();
        append_entry(&path, &entry("c"), 1 << 20).unwrap();
        assert!(previous(&path).exists());
        let targets: Vec<String> = read_entries(&path)
            .unwrap()
            .into_iter()
            .map(|entry| entry.target)
            .collect();
        assert_eq!(targets, ["a", "b", "c"]);
        // One more rotation drops the oldest generation.
        append_entry(&path, &entry("d"), 10).unwrap();
        let targets: Vec<String> = read_entries(&path)
            .unwrap()
            .into_iter()
            .map(|entry| entry.target)
            .collect();
        assert_eq!(targets, ["b", "c", "d"]);
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn the_version_changes_with_every_append() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("audit.jsonl");
        assert_eq!(log_version(&path), None);
        append_entry(&path, &entry("a"), 1 << 20).unwrap();
        let first = log_version(&path).unwrap();
        append_entry(&path, &entry("b"), 1 << 20).unwrap();
        assert_ne!(log_version(&path), Some(first));
    }

    #[test]
    fn the_log_lives_under_the_state_directory() {
        assert!(
            audit_file()
                .unwrap()
                .ends_with(".local/state/kurama/audit.jsonl")
        );
    }
}
