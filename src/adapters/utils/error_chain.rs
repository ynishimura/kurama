//! An error and the causes under it: where an SDK, reqwest and aws-sigv4 all
//! keep what really happened, because their outermost `Display` does not.

/// The error's own text, then every cause under it, joined with `": "`.
///
/// ```
/// # use kurama::adapters::utils::error_chain::causes;
/// let error = std::io::Error::other("connection refused");
/// assert_eq!(causes(&error), "connection refused");
/// ```
pub fn causes(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("{text}")]
    struct Layer {
        text: &'static str,
        #[source]
        source: Option<Box<Layer>>,
    }

    fn layer(text: &'static str, source: Option<Layer>) -> Layer {
        Layer {
            text,
            source: source.map(Box::new),
        }
    }

    /// Every layer, not the outermost one: an SDK's own `Display` says
    /// "dispatch failure" and the reason is three sources down.
    #[test]
    fn an_error_carries_its_own_text_and_every_cause_under_it() {
        let deepest = layer("connection refused", None);
        let middle = layer("tcp connect error", Some(deepest));
        let outermost = layer("dispatch failure", Some(middle));

        assert_eq!(
            causes(&outermost),
            "dispatch failure: tcp connect error: connection refused"
        );
        assert_eq!(causes(&layer("no source", None)), "no source");
    }
}
