//! Hand-off of the credential export script to the shell wrapper
//!
//! The wrapper printed by `kurama init zsh` sets `KURAMA_ENV_SCRIPT` to a
//! temporary file and sources it after the binary exits.

use crate::adapters::config::constants;
use anyhow::Result;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;

/// Output shell script either to a temporary file or stdout
///
/// - `KURAMA_ENV_SCRIPT` set: writes the script to that file with mode 600
/// - otherwise: prints to stdout (for `eval "$(command kurama ...)"`)
///
/// The mode applies only to a file `open` creates, so an existing file is
/// removed first and the new one created exclusively: a file somebody else
/// can read -- or a descriptor they opened on it earlier -- never sees the
/// script. A file that could not be removed makes the exclusive create fail,
/// so the removal's own result says nothing more.
pub fn output_shell_script(script: &str) -> Result<()> {
    if let Ok(script_path) = env::var("KURAMA_ENV_SCRIPT") {
        let _ = fs::remove_file(&script_path);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(constants::file::SECURE_FILE_MODE)
            .open(&script_path)?;

        file.write_all(script.as_bytes())?;
        file.sync_all()?;
    } else {
        print!("{}", script);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::utils::test_env;
    use serial_test::serial;
    use tempfile::NamedTempFile;

    #[test]
    #[serial]
    fn test_output_to_stdout_when_env_var_not_set() {
        test_env::remove("KURAMA_ENV_SCRIPT");

        let result = output_shell_script("export AWS_ACCESS_KEY_ID='test'\n");
        assert!(result.is_ok());
    }

    #[test]
    #[serial]
    fn test_output_to_file_when_env_var_set() -> Result<()> {
        // Absent until the script is written, as most callers hand it over.
        let temp_dir = tempfile::tempdir()?;
        let temp_path = temp_dir.path().join("kurama-env.sh");
        let temp_path = temp_path.to_str().unwrap();
        test_env::set("KURAMA_ENV_SCRIPT", temp_path);

        let script = "export AWS_ACCESS_KEY_ID='test'\nexport AWS_SECRET_ACCESS_KEY='secret'\n";
        let result = output_shell_script(script);
        test_env::remove("KURAMA_ENV_SCRIPT");

        assert!(result.is_ok());
        assert_eq!(fs::read_to_string(temp_path)?, script);
        Ok(())
    }

    #[test]
    #[serial]
    fn an_existing_readable_file_never_holds_the_script() -> Result<()> {
        use std::io::Read;
        use std::os::unix::fs::PermissionsExt;

        let temp_file = NamedTempFile::new()?;
        let temp_path = temp_file.path().to_str().unwrap();
        fs::write(temp_path, "stale\n")?;
        fs::set_permissions(temp_path, fs::Permissions::from_mode(0o644))?;
        // Opened while the file was readable: whoever holds this descriptor
        // must not see what kurama writes.
        let mut earlier_reader = fs::File::open(temp_path)?;
        test_env::set("KURAMA_ENV_SCRIPT", temp_path);

        let result = output_shell_script("export AWS_ACCESS_KEY_ID='test'\n");
        test_env::remove("KURAMA_ENV_SCRIPT");
        result?;

        let permissions = fs::metadata(temp_path)?.permissions();
        assert_eq!(permissions.mode() & 0o777, 0o600);
        assert_eq!(
            fs::read_to_string(temp_path)?,
            "export AWS_ACCESS_KEY_ID='test'\n"
        );
        let mut seen = String::new();
        earlier_reader.read_to_string(&mut seen)?;
        assert_eq!(seen, "stale\n");
        Ok(())
    }
}
