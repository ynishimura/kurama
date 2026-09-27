//! The home, config and cache directories kurama uses.

use std::path::PathBuf;

use crate::adapters::error::{CoreError, CoreResult};

/// Returns the user's home directory.
///
/// # Errors
///
/// Returns an error if the home directory cannot be determined.
pub fn get_home_dir() -> CoreResult<PathBuf> {
    std::env::home_dir().ok_or_else(|| CoreError::other("Home directory not found"))
}

/// Get the kurama config directory (~/.config/kurama)
pub fn get_kurama_config_dir() -> CoreResult<PathBuf> {
    let home = get_home_dir()?;
    Ok(home.join(".config").join("kurama"))
}

/// Get the AWS config directory (~/.aws)
pub fn get_aws_config_dir() -> CoreResult<PathBuf> {
    let home = get_home_dir()?;
    Ok(home.join(".aws"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::adapters::utils::test_env;

    #[test]
    #[serial_test::serial]
    fn resolves_kurama_config_under_home_override() {
        let previous = std::env::var_os("HOME");
        test_env::set("HOME", "/tmp/kurama home/日本語");
        let home = get_home_dir();
        let config = get_kurama_config_dir();
        let aws = get_aws_config_dir();
        test_env::set_or_remove("HOME", previous);

        assert_eq!(home.unwrap(), PathBuf::from("/tmp/kurama home/日本語"));
        assert_eq!(
            config.unwrap(),
            PathBuf::from("/tmp/kurama home/日本語/.config/kurama")
        );
        assert_eq!(aws.unwrap(), PathBuf::from("/tmp/kurama home/日本語/.aws"));
    }
}
