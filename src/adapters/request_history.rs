//! The explorer's request history on disk:
//! `~/.local/state/kurama/history/<api>.jsonl`, one JSON line per sent or
//! saved request, appended and never rewritten, readable by its owner only.

use std::io::{self, BufRead, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::adapters::error::CoreResult;
use crate::adapters::utils::path::get_home_dir;
use crate::domain::types::request_history::HistoryEntry;

/// The history file of the `[api.*]` named `api`; a character a file name
/// should not hold becomes `_`.
pub fn history_file(api: &str) -> CoreResult<PathBuf> {
    let name: String = api
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Ok(get_home_dir()?
        .join(".local")
        .join("state")
        .join("kurama")
        .join("history")
        .join(format!("{name}.jsonl")))
}

/// Every line of the file, oldest first; no file is no history, and a line
/// that is not an entry (cut by a crash, written by a later version) is
/// skipped.
pub fn read_history(path: &Path) -> io::Result<Vec<HistoryEntry>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut entries = Vec::new();
    for line in io::BufReader::new(file).lines() {
        if let Ok(entry) = serde_json::from_str(&line?) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

/// Append one entry, creating the file (mode 600) and its directory.
pub fn append_history(path: &Path, entry: &HistoryEntry) -> io::Result<()> {
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(operation: &str) -> HistoryEntry {
        HistoryEntry {
            operation: operation.into(),
            params: vec![("petId".into(), "p1".into())],
            body: None,
            name: None,
        }
    }

    #[test]
    fn appended_entries_read_back_in_order_and_a_broken_line_is_skipped() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state").join("pets.jsonl");
        assert_eq!(read_history(&path).unwrap(), Vec::new(), "no file yet");
        append_history(&path, &entry("pets/list")).unwrap();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(b"{\"operation\": \"cut\n").unwrap();
        append_history(&path, &entry("pets/get")).unwrap();
        assert_eq!(
            read_history(&path).unwrap(),
            vec![entry("pets/list"), entry("pets/get")]
        );
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn the_file_is_named_after_the_api_without_path_characters() {
        let path = history_file("../a b").unwrap();
        assert_eq!(path.file_name().unwrap(), "___a_b.jsonl");
        assert!(path.ends_with(".local/state/kurama/history/___a_b.jsonl"));
    }
}
