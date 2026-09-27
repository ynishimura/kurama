//! OpenAPI documents on disk and on the wire: JSON or YAML bytes parsed
//! into a `serde_json::Value` (the domain normalizes it), and the cache of
//! documents fetched from a URL (`cache`).

pub mod cache;
mod loader;

pub use cache::{CachedSpec, SpecCache};
pub use loader::{
    FileReadError, default_cache_dir, parse_spec, read_file, read_offline, read_offline_spec,
};

/// The document as JSON: parsed as JSON first, as YAML when that fails.
/// When neither parses, the error of the format the text looks like is
/// reported.
pub fn parse_document(bytes: &[u8]) -> Result<serde_json::Value, String> {
    let json_error = match serde_json::from_slice::<serde_json::Value>(bytes) {
        Ok(document) => return Ok(document),
        Err(error) => error,
    };
    // Trusted API descriptions may be large: no node or event budget.
    let mut options = serde_saphyr::Options::default();
    options.budget = None;
    let yaml_error =
        match serde_saphyr::from_slice_with_options::<serde_json::Value>(bytes, options) {
            Ok(document) => return Ok(document),
            Err(error) => error,
        };
    let looks_like_json = bytes
        .iter()
        .find(|byte| !byte.is_ascii_whitespace())
        .is_some_and(|byte| *byte == b'{' || *byte == b'[');
    if looks_like_json {
        Err(format!("JSON: {json_error}"))
    } else {
        Err(format!("YAML: {}", one_line(&yaml_error.to_string())))
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_and_yaml_documents_parse_to_the_same_value() {
        let json = br#"{"openapi": "3.0.0", "paths": {"/x": {"get": {"responses": {"200": {"description": "ok"}}}}}}"#;
        let yaml = b"openapi: \"3.0.0\"\npaths:\n  /x:\n    get:\n      responses:\n        200:\n          description: ok\n";
        let from_json = parse_document(json).unwrap();
        let from_yaml = parse_document(yaml).unwrap();
        assert_eq!(
            from_json, from_yaml,
            "a YAML integer key becomes a string key"
        );
        assert_eq!(
            from_yaml["paths"]["/x"]["get"]["responses"]["200"]["description"],
            "ok"
        );
    }

    #[test]
    fn yaml_scalars_keep_their_types_where_json_has_them() {
        let document =
            parse_document(b"required: true\nlimit: 20\nratio: 0.5\nname: 'x'\n").unwrap();
        assert_eq!(document["required"], true);
        assert_eq!(document["limit"], 20);
        assert_eq!(document["ratio"], 0.5);
        assert_eq!(document["name"], "x");
    }

    #[test]
    fn errors_name_the_format_the_text_looks_like() {
        let error = parse_document(b"{\"openapi\": ").unwrap_err();
        assert!(error.starts_with("JSON: "), "{error}");
        let error = parse_document(b"openapi: [unclosed\n  - x: {\n").unwrap_err();
        assert!(error.starts_with("YAML: "), "{error}");
        assert!(!error.contains('\n'));
    }
}
