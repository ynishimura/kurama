//! The `kurama data` operation an S3 object can be opened with: the exact `from` URI, the `[s3.*]` that signs it, and what was observed of the object as provenance; derived once for `kurama s3 --json` and the S3 explorer.
use super::operation_command::shell_quote;
use crate::domain::types::dataset::DataFormat;
use crate::domain::types::s3_object::{DataHandoffArgs, S3NextAction, S3Provenance};

/// What was observed of one object.
pub struct Observed<'a> {
    pub bucket: &'a str,
    pub key: &'a str,
    pub etag: Option<&'a str>,
    pub size: u64,
    pub last_modified: Option<&'a str>,
}

/// The `kurama data --describe` of an object whose name is a format `data`
/// reads (CSV, JSONL, Parquet, gzip included); `None` for any other.
pub fn data_handoff(s3: &str, object: &Observed<'_>) -> Option<S3NextAction> {
    DataFormat::from_path(object.key)?;
    let from = format!("s3://{}/{}", object.bucket, object.key);
    Some(S3NextAction {
        kind: "data",
        operation: "describe",
        message: format!(
            "describe its columns with `{}`; data reads the object again",
            command_line(&from, s3)
        ),
        args: DataHandoffArgs {
            from,
            s3_source: s3.to_owned(),
        },
        provenance: S3Provenance {
            etag: object.etag.map(str::to_owned),
            size: object.size,
            last_modified: object.last_modified.map(str::to_owned),
        },
    })
}

/// The command line that runs a next action, each argument one shell word.
pub fn handoff_command(action: &S3NextAction) -> String {
    command_line(&action.args.from, &action.args.s3_source)
}

fn command_line(from: &str, s3: &str) -> String {
    format!(
        "kurama data --from {} --s3-source {} --describe",
        shell_quote(from),
        shell_quote(s3)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observed(key: &str) -> Observed<'_> {
        Observed {
            bucket: "lake",
            key,
            etag: Some("\"e1\""),
            size: 42,
            last_modified: Some("2026-09-01T00:00:00Z"),
        }
    }

    #[test]
    fn s3_handoff_names_the_exact_uri_the_source_and_what_was_seen() {
        let action = data_handoff("assets", &observed("a b/'x'.PARQUET")).unwrap();
        assert_eq!((action.kind, action.operation), ("data", "describe"));
        assert_eq!(action.args.from, "s3://lake/a b/'x'.PARQUET");
        assert_eq!(action.args.s3_source, "assets");
        assert_eq!(
            action.provenance,
            S3Provenance {
                etag: Some("\"e1\"".into()),
                size: 42,
                last_modified: Some("2026-09-01T00:00:00Z".into()),
            }
        );
        assert_eq!(
            handoff_command(&action),
            "kurama data --from 's3://lake/a b/'\\''x'\\''.PARQUET' --s3-source assets --describe"
        );
        assert!(action.message.contains(&handoff_command(&action)));
    }

    #[test]
    fn s3_handoff_offers_only_what_data_reads() {
        for key in ["r.csv", "r.csv.gz", "e.jsonl", "e.ndjson.gz", "t.parquet"] {
            assert!(data_handoff("s", &observed(key)).is_some(), "{key}");
        }
        for key in ["r.json", "notes.txt", "image.png", "dir/", "parquet"] {
            assert!(data_handoff("s", &observed(key)).is_none(), "{key}");
        }
    }
}
