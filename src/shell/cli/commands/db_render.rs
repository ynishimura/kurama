//! Render a typed database result as JSON or as terminal-safe text.
use crate::domain::types::database::DbOutput;
use crate::shell::cli::client::{json_line, tab_separated};

/// One document on stdout: JSON, or the rows as text.
pub(super) fn print_result(output: &DbOutput, json: bool) {
    if json {
        println!("{}", json_line(output));
        return;
    }
    print!("{}", render_text_result(output));
    if let Some(cursor) = &output.meta.next_cursor {
        crate::console::progress!("# More rows remain; continue with --cursor {cursor}");
    }
}

fn render_text_result(output: &DbOutput) -> String {
    let Some(result) = &output.result else {
        // A plan has no rows; it is the document it describes.
        return format!(
            "{}\n",
            serde_json::to_string_pretty(output).expect("typed database plan serializes")
        );
    };
    tab_separated(
        result.columns.iter().map(|column| column.name.as_str()),
        &result.rows,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::database::{
        DbColumn, DbEncoding, DbEngineInfo, DbKind, DbLimits, DbMeta, DbOperation, DbResult,
    };
    use serde_json::{Value, json};

    fn output(result: Option<DbResult>) -> DbOutput {
        DbOutput {
            schema_version: 1,
            kind: DbKind::Db,
            operation: DbOperation::Query,
            target: "app".into(),
            meta: DbMeta::planned(
                DbOperation::Query,
                DbLimits::default(),
                DbEngineInfo {
                    name: "sqlite".into(),
                    server_version: None,
                },
                "/tmp/app.sqlite3".into(),
                None,
            ),
            dry_run: None,
            result,
            statements: None,
            elapsed_ms: None,
        }
    }

    #[test]
    fn db_text_escapes_controls_in_names_and_cells() {
        let rendered = render_text_result(&output(Some(DbResult::new(
            vec![
                DbColumn {
                    name: "na\tme".into(),
                    data_type: Some("TEXT".into()),
                    encoding: DbEncoding::Text,
                },
                DbColumn {
                    name: "note".into(),
                    data_type: None,
                    encoding: DbEncoding::Text,
                },
            ],
            vec![
                vec![json!("a\u{1b}[31m"), Value::Null],
                vec![json!("日本語"), json!("")],
            ],
            None,
            0,
        ))));
        assert_eq!(rendered, "na\\tme\tnote\na\\u{1b}[31m\tnull\n日本語\t\n");
    }

    #[test]
    fn db_text_plan_is_the_document_it_describes() {
        let mut plan = output(None);
        plan.dry_run = Some(true);
        let rendered = render_text_result(&plan);
        assert!(rendered.contains("\"dry_run\": true"), "{rendered}");
        assert!(rendered.ends_with('\n'));
    }
}
