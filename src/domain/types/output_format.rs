//! Output format for credential export

/// Format for outputting credentials
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    /// Shell export script (default)
    #[default]
    Shell,
    /// AWS credential_process compatible JSON
    Json,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_is_shell() {
        assert_eq!(OutputFormat::default(), OutputFormat::Shell);
    }
}
