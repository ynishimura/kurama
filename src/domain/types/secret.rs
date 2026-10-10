//! A secret value read from a store or issued elsewhere: redacted in `Debug` and `Display`, zeroized on drop, and read only through `expose`.
//!
//! There is no `Deref<Target = str>`: every place that hands the value to a
//! request, an export script or stdout says so with `expose()`, which is
//! what a reviewer greps for.

use zeroize::{Zeroize, ZeroizeOnDrop};

/// What `Debug` and `Display` print in place of the value.
const REDACTED: &str = "[REDACTED]";

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The value itself, for the one place that sends, exports or prints it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Secret({REDACTED})")
    }
}

impl std::fmt::Display for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(REDACTED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neither_debug_nor_display_prints_the_value() {
        let secret = Secret::new("s3cret-value");
        for printed in [
            format!("{secret:?}"),
            format!("{secret:#?}"),
            format!("{secret}"),
            format!("{:?}", Some(secret.clone())),
        ] {
            assert!(!printed.contains("s3cret-value"), "{printed}");
            assert!(printed.contains(REDACTED), "{printed}");
        }
        assert_eq!(format!("{secret:?}"), "Secret([REDACTED])");
        assert_eq!(format!("{secret}"), "[REDACTED]");
    }

    #[test]
    fn expose_is_the_value_whichever_way_it_was_made() {
        assert_eq!(Secret::new("a").expose(), "a");
        assert_eq!(Secret::from("b".to_owned()).expose(), "b");
        assert_eq!(Secret::from("c").expose(), "c");
    }

    #[test]
    fn zeroize_clears_the_value() {
        let mut secret = Secret::new("s3cret-value");
        secret.zeroize();
        assert_eq!(secret.expose(), "");
    }
}
