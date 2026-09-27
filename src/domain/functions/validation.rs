//! Validation helper functions
//!
//! Pure validation functions that can be used across the domain layer.

/// Check if a string is a valid AWS account ID (12 digits)
///
/// # Example
///
/// ```
/// use kurama::domain::functions::validation::is_valid_account_id;
///
/// assert!(is_valid_account_id("123456789012"));
/// assert!(!is_valid_account_id("12345"));
/// assert!(!is_valid_account_id("12345678901a"));
/// ```
pub fn is_valid_account_id(account_id: &str) -> bool {
    account_id.len() == 12 && account_id.chars().all(|c| c.is_ascii_digit())
}

/// Extract account ID from a role ARN
///
/// # Example
///
/// ```
/// use kurama::domain::functions::validation::extract_account_id;
///
/// let account = extract_account_id("arn:aws:iam::123456789012:role/MyRole");
/// assert_eq!(account, Some("123456789012"));
/// ```
pub fn extract_account_id(arn: &str) -> Option<&str> {
    let parts: Vec<&str> = arn.split(':').collect();
    if parts.len() >= 5 {
        let account = parts[4];
        if is_valid_account_id(account) {
            return Some(account);
        }
    }
    None
}

/// Extract role name from a role ARN
///
/// # Example
///
/// ```
/// use kurama::domain::functions::validation::extract_role_name;
///
/// let name = extract_role_name("arn:aws:iam::123456789012:role/MyRole");
/// assert_eq!(name, Some("MyRole"));
///
/// let name = extract_role_name("arn:aws:iam::123456789012:role/path/to/MyRole");
/// assert_eq!(name, Some("MyRole"));
/// ```
pub fn extract_role_name(arn: &str) -> Option<&str> {
    arn.split(':')
        .next_back()
        .and_then(|resource| resource.strip_prefix("role/"))
        .and_then(|path| path.split('/').next_back())
}

/// Extract username from MFA serial ARN
///
/// # Example
///
/// ```
/// use kurama::domain::functions::validation::extract_mfa_username;
///
/// let username = extract_mfa_username("arn:aws:iam::123456789012:mfa/john.doe");
/// assert_eq!(username, Some("john.doe"));
/// ```
pub fn extract_mfa_username(mfa_serial: &str) -> Option<&str> {
    let parts: Vec<&str> = mfa_serial.split(':').collect();
    if parts.len() >= 6
        && let Some(mfa_part) = parts.get(5)
    {
        return mfa_part.strip_prefix("mfa/");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_valid_account_id() {
        assert!(is_valid_account_id("123456789012"));
        assert!(!is_valid_account_id("12345678901")); // Too short
        assert!(!is_valid_account_id("1234567890123")); // Too long
        assert!(!is_valid_account_id("12345678901a")); // Contains letter
    }

    #[test]
    fn test_extract_account_id() {
        assert_eq!(
            extract_account_id("arn:aws:iam::123456789012:role/MyRole"),
            Some("123456789012")
        );
        assert_eq!(extract_account_id("invalid"), None);
    }

    #[test]
    fn test_extract_role_name() {
        assert_eq!(
            extract_role_name("arn:aws:iam::123456789012:role/MyRole"),
            Some("MyRole")
        );
        assert_eq!(
            extract_role_name("arn:aws:iam::123456789012:role/path/to/MyRole"),
            Some("MyRole")
        );
        assert_eq!(extract_role_name("invalid"), None);
    }

    #[test]
    fn test_extract_mfa_username() {
        assert_eq!(
            extract_mfa_username("arn:aws:iam::123456789012:mfa/john.doe"),
            Some("john.doe")
        );
        assert_eq!(extract_mfa_username("invalid"), None);
    }
}
