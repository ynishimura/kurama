//! `agent --kind s3 --json` and `status --kind s3`: the S3 operations and the bounds a run takes when it names none, and the configured connections; no configuration is read for the contract, and nothing is assumed or reached.
use serde_json::{Value, json};
use strum::VariantArray;

use crate::adapters::config::Config;
use crate::domain::types::limits::S3_READ;
use crate::domain::types::s3_browse::{
    DEFAULT_PAGE_SIZE, DEFAULT_REQUEST_TIMEOUT_SECS, DEFAULT_SEARCH_OBJECTS, MAX_OBJECTS,
    MAX_PAGE_SIZE, S3Invalid, S3Operation,
};

/// The flag that selects `operation`, and what running it touches.
fn operation(operation: S3Operation) -> (&'static str, &'static str) {
    match operation {
        S3Operation::Buckets => ("--buckets", "ListBuckets, every page"),
        S3Operation::List => (
            "--list",
            "ListObjectsV2: one level, or every key with --recursive",
        ),
        S3Operation::Search => (
            "--search TEXT",
            "ListObjectsV2: the keys under the prefix that contain TEXT",
        ),
        S3Operation::Head => ("--head", "HeadObject of one key"),
        S3Operation::Preview => (
            "--preview",
            "HeadObject, then one ranged GetObject conditional on the ETag",
        ),
        S3Operation::ContentSearch => (
            "--search-content TEXT",
            "ListObjectsV2, then a bounded GetObject of each listed object",
        ),
    }
}

pub fn capabilities() -> Value {
    let name = |op: S3Operation| serde_json::to_value(op).expect("an operation serializes");
    let flags: serde_json::Map<String, Value> = S3Operation::VARIANTS
        .iter()
        .copied()
        .map(|op| (text(name(op)), operation(op).0.into()))
        .collect();
    let mut side_effects: serde_json::Map<String, Value> = S3Operation::VARIANTS
        .iter()
        .copied()
        .map(|op| (text(name(op)), operation(op).1.into()))
        .collect();
    side_effects.insert("dry_run".into(), "none".into());
    json!({
        "schema_version": 1,
        "kind": "s3",
        "operations": S3Operation::VARIANTS.iter().copied().map(name).collect::<Vec<_>>(),
        "flags": flags,
        "defaults": {
            "page_size": DEFAULT_PAGE_SIZE,
            "max_page_size": MAX_PAGE_SIZE,
            "list_max_objects": "page_size",
            "search_max_objects": DEFAULT_SEARCH_OBJECTS,
            "content_search_max_objects": S3_READ.search_objects,
            "max_objects": MAX_OBJECTS,
            "request_timeout_secs": DEFAULT_REQUEST_TIMEOUT_SECS,
            "preview_bytes": S3_READ.preview_bytes,
            "max_preview_bytes": S3_READ.max_preview_bytes,
            "gzip_transfer_bytes": S3_READ.gzip_transfer_bytes,
            "hex_bytes": S3_READ.hex_bytes,
            "object_bytes": S3_READ.object_bytes,
            "total_bytes": S3_READ.total_bytes,
            "matches": S3_READ.matches,
            "excerpt_chars": S3_READ.excerpt_chars,
        },
        "error_schema": crate::shell::cli::client_error::schema(),
        // `kurama s3 <S3>` with no operation, on a terminal: a person's
        // explorer. Without a terminal the same call is S3_INVALID.
        "capabilities": {"tui": true, "write": false, "data_handoff": true},
        "side_effects": side_effects,
        "credentials": "the [s3.*] aws_profile is assumed through the same path as `kurama env`, once per run; the explorer assumes it again when it is about to expire",
        "region": "--region, then the [s3.*] region, then the AWS profile's; a bucket in another region answers with its own, which is followed",
        "paging": "a run examines at most --max-objects entries; complete: false with a cursor goes on through --cursor with the same TARGET, operation, --recursive and --search",
        "handoff": "--head and --preview of a key kurama data reads carry meta.next_actions: kind data, operation describe, the exact from URI and the [s3.*] that signs it",
        "examples": [
            ["s3", "assets", "--list", "--json"],
            ["s3", "assets", "s3://example-assets/reports/", "--search", "invoice", "--json"],
            ["s3", "assets", "s3://example-assets/reports/result.json", "--preview", "--bytes", "65536", "--json"],
            ["s3", "assets", "s3://example-assets/logs/", "--search-content", "request-id-123", "--json"]
        ],
    })
}

fn text(value: Value) -> String {
    value.as_str().expect("an operation is a string").to_owned()
}

pub fn print_status(config: &Config, name: Option<&str>, json: bool) -> anyhow::Result<()> {
    let rows: Vec<_> = super::s3_status::status_rows(config, name)
        .into_iter()
        .map(super::status::StatusRow::S3)
        .collect();
    if let (Some(name), true) = (name, rows.is_empty()) {
        return Err(S3Invalid::UnknownConnection(name.to_owned()).into());
    }
    super::status::print_rows(&rows, chrono::Utc::now(), json);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every operation the CLI parses is listed, under the name its result
    /// envelope carries, with the flag that selects it.
    #[test]
    fn s3_contract_lists_every_operation_with_its_flag() {
        let doc = capabilities();
        assert_eq!(
            doc["operations"],
            json!([
                "buckets",
                "list",
                "search",
                "head",
                "preview",
                "content_search"
            ])
        );
        for op in doc["operations"].as_array().unwrap() {
            let parsed: S3Operation = serde_json::from_value(op.clone()).unwrap();
            let flag = doc["flags"][op.as_str().unwrap()].as_str().unwrap();
            let long = flag.split(' ').next().unwrap().trim_start_matches("--");
            assert!(
                crate::build_command()
                    .find_subcommand("s3")
                    .unwrap()
                    .get_arguments()
                    .any(|arg| arg.get_long() == Some(long)),
                "{flag}"
            );
            assert_eq!(operation(parsed).0, flag);
        }
        assert_eq!(doc["side_effects"]["dry_run"], "none");
    }

    /// The defaults are the values a run uses, not copies of them.
    #[test]
    fn s3_contract_defaults_are_the_bounds_a_run_takes() {
        let defaults = &capabilities()["defaults"];
        assert_eq!(defaults["page_size"], 200);
        assert_eq!(defaults["search_max_objects"], 1000);
        assert_eq!(defaults["content_search_max_objects"], 100);
        assert_eq!(defaults["max_objects"], 100_000);
        assert_eq!(defaults["request_timeout_secs"], 30);
        assert_eq!(defaults["preview_bytes"], 65536);
        assert_eq!(defaults["max_preview_bytes"], 1_048_576);
        assert_eq!(defaults["gzip_transfer_bytes"], S3_READ.gzip_transfer_bytes);
        assert_eq!(defaults["total_bytes"], S3_READ.total_bytes);
    }

    #[test]
    fn s3_status_of_an_unknown_connection_is_refused() {
        let config = Config::parse("[s3.assets]\naws_profile = \"dev\"\n").unwrap();
        let error = print_status(&config, Some("nope"), true).unwrap_err();
        assert!(matches!(
            error.downcast_ref::<S3Invalid>(),
            Some(S3Invalid::UnknownConnection(name)) if name == "nope"
        ));
    }
}
