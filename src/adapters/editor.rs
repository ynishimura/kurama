//! Edit a text in the person's editor (`$VISUAL`, then `$EDITOR`, then
//! `vi`) through a temporary file that only the owner can read and that
//! is removed afterwards. The caller suspends the TUI around the call.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::process::Command;

use crate::adapters::config::constants::file::SECURE_FILE_MODE;

/// The editor command line, as the shell would run it.
pub fn editor_command() -> String {
    ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "vi".to_string())
}

/// `initial` after the editor closed it; `suffix` names the file type
/// (`json`) so the editor picks its mode.
pub fn edit_text(initial: &str, suffix: &str) -> std::io::Result<String> {
    let path = temp_path(suffix);
    let result = edit_file(&path, initial);
    let _ = std::fs::remove_file(&path);
    result
}

fn edit_file(path: &PathBuf, initial: &str) -> std::io::Result<String> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(SECURE_FILE_MODE)
        .open(path)?
        .write_all(initial.as_bytes())?;
    let editor = editor_command();
    // `sh -c` lets $EDITOR carry arguments (`code --wait`); `$1` is the file.
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("kurama-editor")
        .arg(path)
        .status()
        .map_err(|error| std::io::Error::new(error.kind(), format!("{editor}: {error}")))?;
    if !status.success() {
        return Err(std::io::Error::other(format!(
            "{editor} exited with {status}"
        )));
    }
    std::fs::read_to_string(path)
}

fn temp_path(suffix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "kurama-body-{}-{nanos}.{suffix}",
        std::process::id()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::utils::test_env;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    #[serial_test::serial]
    fn the_editor_sees_the_text_in_a_private_file_that_is_removed_afterwards() {
        let previous = std::env::var_os("EDITOR");
        let visual = std::env::var_os("VISUAL");
        test_env::remove("VISUAL");
        // The "editor" records the file's mode and appends a line.
        let script = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            script.path(),
            "#!/bin/sh\nstat -f '%Lp' \"$1\" > \"$1.mode\" 2>/dev/null || stat -c '%a' \"$1\" > \"$1.mode\"\nprintf '\\n{\"edited\":true}' >> \"$1\"\n",
        )
        .unwrap();
        std::fs::set_permissions(script.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        // Linux refuses to execute a file while a writable descriptor is open.
        let script = script.into_temp_path();
        test_env::set("EDITOR", script.as_os_str());
        let edited = edit_text("{\"title\":\"x\"}", "json");
        test_env::set_or_remove("EDITOR", previous);
        if let Some(value) = visual {
            test_env::set("VISUAL", value);
        }
        assert_eq!(edited.unwrap(), "{\"title\":\"x\"}\n{\"edited\":true}");
        let modes: Vec<_> = std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .flatten()
            .filter(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.starts_with(&format!("kurama-body-{}-", std::process::id()))
            })
            .collect();
        assert_eq!(modes.len(), 1, "only the mode record remains: {modes:?}");
        let mode = std::fs::read_to_string(modes[0].path()).unwrap();
        assert_eq!(mode.trim(), "600");
        let _ = std::fs::remove_file(modes[0].path());
    }

    #[test]
    #[serial_test::serial]
    fn a_failing_editor_is_an_error_and_leaves_no_file() {
        let previous = std::env::var_os("EDITOR");
        test_env::set("EDITOR", "false");
        let result = edit_text("x", "txt");
        test_env::set_or_remove("EDITOR", previous);
        assert!(result.unwrap_err().to_string().contains("exited with"));
        let leftovers = std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(&format!("kurama-body-{}-", std::process::id()))
            })
            .count();
        assert_eq!(leftovers, 0);
    }
}
