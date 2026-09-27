//! Fixtures for every S3 explorer state and the explorer's UI contract, for the render regression (`view_snapshot_tests.rs`) and the update tests.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;

use super::effects::{S3Ask, S3Effect};
use super::messages::{Answer, Answered, Failure, Found, S3Message};
use super::model::{Focus, Row, S3Modal, S3Model, S3Summary};
use super::update::update;
use super::view::{APP_TITLE, READ_ONLY_BADGE, hints, render};
use crate::domain::types::s3_browse::{
    S3Bucket, S3Location, S3Object, S3Position, S3Result, S3StopReason,
};
use crate::domain::types::s3_object::{
    S3Match, S3ObjectHead, S3Preview, S3PreviewKind, S3TextFormat,
};
use crate::shell::tui::layout::Breakpoint;
use crate::shell::tui::testing::{
    buffer_lines, cell, column_of, has_style, modal_violations, render_frame, selection_violations,
};
use crate::shell::tui::theme::{self, Tone};

pub fn render_buffer(model: &S3Model, width: u16, height: u16) -> Buffer {
    render_frame(width, height, |frame| render(frame, model))
}

/// Violations of the S3 explorer's contract: the title and the read-only
/// badge in the header, the primary hint in the footer, a panel ending
/// above the footer, the selected row marked in the selection style while
/// the list is on screen, a modal inside the viewport in its tone.
pub fn check_contract(model: &S3Model, buffer: &Buffer) -> Vec<String> {
    let lines = buffer_lines(buffer);
    let height = lines.len();
    let mut violations = Vec::new();
    if !lines[0].contains(APP_TITLE) {
        violations.push(format!(
            "header row does not show the title: {:?}",
            lines[0]
        ));
    }
    match column_of(&lines[0], READ_ONLY_BADGE) {
        Some(x) => {
            if !has_style(cell(buffer, x, 0), theme::badge_on()) {
                violations.push("the READ ONLY badge is not drawn in its on style".into());
            }
        }
        None => violations.push(format!("header lacks the READ ONLY badge: {:?}", lines[0])),
    }
    let footer = &lines[height - 1];
    if let Some((key, action)) = hints(model).first()
        && !footer.contains(&format!("{key} {action}"))
    {
        violations.push(format!(
            "footer lacks the primary hint {key} {action}: {footer:?}"
        ));
    }
    if height >= 4 && !lines[height - 2].contains(theme::BORDER.bottom_left) {
        violations.push(format!(
            "row above the footer is not a panel bottom border: {:?}",
            lines[height - 2]
        ));
    }
    let modal = match (&model.modal, &model.form) {
        (Some(S3Modal::Help), _) => Some(("Help", Tone::Info)),
        (Some(S3Modal::Error(_)), _) => Some(("Error", Tone::Danger)),
        (Some(S3Modal::Handoff(_)), _) => Some(("Open as data", Tone::Info)),
        (None, Some(form)) if form.content => Some(("Content search", Tone::Info)),
        (None, Some(_)) => Some(("Key search", Tone::Info)),
        (None, None) => None,
    };
    let list_shown =
        model.focus == Focus::List || Breakpoint::of(buffer.area.width) != Breakpoint::Compact;
    if let Some((title, tone)) = modal {
        violations.extend(modal_violations(buffer, &lines, title, tone));
    } else if list_shown && let Some(row) = model.selected_row() {
        violations.extend(selection_violations(
            buffer,
            &lines,
            &shown_start(model, row),
        ));
    }
    violations
}

/// The first characters the list shows of a row: its name under the level.
fn shown_start(model: &S3Model, row: &Row) -> String {
    let prefix = model
        .listing
        .location
        .as_ref()
        .map_or("", |location| location.prefix.as_str());
    let name = row.name();
    name.strip_prefix(prefix)
        .unwrap_or(name)
        .chars()
        .take(6)
        .collect()
}

/// A key without modifiers.
pub fn key(code: KeyCode) -> S3Message {
    S3Message::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

/// The effects of one message.
pub fn send(model: &mut S3Model, message: S3Message) -> Vec<S3Effect> {
    update(model, message)
}

/// The request the effects ask for, if any.
pub fn asked(effects: &[S3Effect]) -> Option<S3Ask> {
    effects.iter().find_map(|effect| match effect {
        S3Effect::Ask(ask) => Some(ask.clone()),
        _ => None,
    })
}

/// Answer the running request.
pub fn answer(model: &mut S3Model, outcome: Result<Answered, Failure>) -> Vec<S3Effect> {
    let ask = model.running.as_ref().expect("a request runs").ask.clone();
    update(model, S3Message::Answered(Answer { ask, outcome }))
}

pub mod fixtures {
    use super::*;

    pub fn object(key: &str, size: u64) -> S3Object {
        S3Object {
            key: key.into(),
            size,
            last_modified: Some("2026-09-01T00:00:00Z".into()),
            etag: Some("\"9b2cf535f27731c974343645a3985328\"".into()),
            storage_class: Some("STANDARD".into()),
        }
    }

    fn summary() -> S3Summary {
        S3Summary {
            name: "assets".into(),
            aws_profile: "dev".into(),
            region: "ap-northeast-1".into(),
            page_size: 200,
        }
    }

    pub fn at(prefix: &str) -> S3Location {
        S3Location {
            bucket: "example-assets".into(),
            prefix: prefix.into(),
        }
    }

    fn typed(model: &mut S3Model, text: &str) {
        for c in text.chars() {
            send(model, key(KeyCode::Char(c)));
        }
    }

    /// The first page asked for; nothing arrived yet.
    pub fn loading() -> S3Model {
        let mut model = S3Model::new(summary(), Some(at("reports/")));
        super::super::update::start(&mut model);
        model
    }

    /// A level with prefixes and objects, and a next page.
    pub fn loaded() -> S3Model {
        let mut model = loading();
        answer(
            &mut model,
            Ok(Answered::Level {
                prefixes: vec!["reports/2024/".into(), "reports/2025/".into()],
                objects: vec![
                    object("reports/orders.csv", 20),
                    object("reports/summary.json", 33),
                    object("reports/events.parquet", 1_048_576),
                ],
                scanned: 5,
                next: Some(S3Position {
                    token: Some("t".into()),
                    skip: 0,
                }),
            }),
        );
        model
    }

    /// The CSV selected; the next page is not asked for above the last row.
    pub fn selected() -> S3Model {
        let mut model = loaded();
        send(&mut model, key(KeyCode::Down));
        send(&mut model, key(KeyCode::Down));
        model
    }

    /// The bucket list, a region per bucket.
    pub fn buckets() -> S3Model {
        let mut model = S3Model::new(summary(), None);
        super::super::update::start(&mut model);
        answer(
            &mut model,
            Ok(Answered::Buckets(vec![
                S3Bucket {
                    name: "example-assets".into(),
                    created: Some("2024-01-01T00:00:00Z".into()),
                    region: Some("ap-northeast-1".into()),
                },
                S3Bucket {
                    name: "overturemaps-us-west-2".into(),
                    created: None,
                    region: Some("us-west-2".into()),
                },
            ])),
        );
        model
    }

    /// `/` typed `csv`: only the rows here are read.
    pub fn filtering() -> S3Model {
        let mut model = loaded();
        send(&mut model, key(KeyCode::Char('/')));
        typed(&mut model, "csv");
        model
    }

    /// A level with nothing under it.
    pub fn empty() -> S3Model {
        let mut model = loading();
        answer(
            &mut model,
            Ok(Answered::Level {
                prefixes: vec![],
                objects: vec![],
                scanned: 0,
                next: None,
            }),
        );
        model
    }

    /// The content search form, a bound refused.
    pub fn form() -> S3Model {
        let mut model = loaded();
        send(&mut model, key(KeyCode::Char('g')));
        typed(&mut model, "request-id-123");
        send(&mut model, key(KeyCode::Tab));
        send(&mut model, key(KeyCode::Backspace));
        send(&mut model, key(KeyCode::Backspace));
        send(&mut model, key(KeyCode::Backspace));
        typed(&mut model, "0");
        send(&mut model, key(KeyCode::Enter));
        model
    }

    /// A key search, the first page found, the second on its way.
    pub fn key_search_running() -> S3Model {
        let mut model = loaded();
        send(&mut model, key(KeyCode::Char('s')));
        typed(&mut model, "invoice");
        send(&mut model, key(KeyCode::Enter));
        send(
            &mut model,
            S3Message::Found(Found {
                objects: vec![
                    object("reports/invoice-001.pdf", 7),
                    object("reports/invoice-002.pdf", 7),
                ],
                matches: vec![],
                scanned: 200,
                read: 0,
            }),
        );
        send(&mut model, S3Message::Elapsed(3));
        model
    }

    /// `Esc` stopped the key search: the rows found stay.
    pub fn stopped() -> S3Model {
        let mut model = key_search_running();
        send(&mut model, key(KeyCode::Esc));
        answer(
            &mut model,
            Err(Failure {
                message: "stopped".into(),
                stopped: true,
            }),
        );
        model
    }

    fn found(key: &str, line: u64, excerpt: &str) -> S3Match {
        S3Match {
            bucket: "example-assets".into(),
            key: key.into(),
            line,
            byte_offset: 0,
            excerpt: excerpt.into(),
            fetched_at: "2026-09-27T00:00:00Z".into(),
        }
    }

    /// A content search that stopped at its match bound; an excerpt holds
    /// an escape sequence, drawn escaped.
    pub fn content_search() -> S3Model {
        let mut model = loaded();
        send(&mut model, key(KeyCode::Char('g')));
        typed(&mut model, "request-id-123");
        send(&mut model, key(KeyCode::Enter));
        send(
            &mut model,
            S3Message::Found(Found {
                objects: vec![],
                matches: vec![
                    found("reports/a.log", 2, "GET /x request-id-123 200"),
                    found(
                        "reports/b.log.gz",
                        2,
                        "POST /z request-id-123 500\u{1b}[31m",
                    ),
                ],
                scanned: 5,
                read: 2,
            }),
        );
        answer(
            &mut model,
            Ok(Answered::Searched {
                complete: false,
                stop_reason: Some(S3StopReason::MaxMatches),
                skipped: 1,
            }),
        );
        model
    }

    fn head(key: &str, size: u64, content_type: &str) -> S3ObjectHead {
        S3ObjectHead {
            key: key.into(),
            size,
            content_type: Some(content_type.into()),
            content_encoding: None,
            last_modified: Some("2026-09-01T00:00:00Z".into()),
            etag: Some("\"9b2cf535f27731c974343645a3985328\"".into()),
            storage_class: "STANDARD".into(),
            restore: None,
            archive_status: None,
        }
    }

    /// The CSV previewed, a range short of the end.
    pub fn preview() -> S3Model {
        let mut model = selected();
        send(&mut model, key(KeyCode::Enter));
        let result = S3Result {
            object: Some(head("reports/orders.csv", 120_000, "text/csv")),
            preview: Some(S3Preview {
                kind: S3PreviewKind::Text,
                format: S3TextFormat::Csv,
                encoding: "identity",
                range_start: 0,
                range_end: 65_536,
                next_offset: Some(65_536),
                utf8_head_skipped: 0,
                utf8_tail_cut: 0,
                well_formed: None,
                text: Some("id,amount,note\n1,10,日本語\n2,20,tab\there\n".into()),
                hex: None,
            }),
            stop_reason: Some(S3StopReason::MaxBytes),
            ..S3Result::default()
        };
        answer(&mut model, Ok(Answered::Preview(Box::new(result))));
        model
    }

    /// A binary object: only the hex head.
    pub fn preview_binary() -> S3Model {
        let mut model = loaded();
        for _ in 0..4 {
            send(&mut model, key(KeyCode::Down));
        }
        // The last row asked for the next page; it arrives empty.
        answer(
            &mut model,
            Ok(Answered::Level {
                prefixes: vec![],
                objects: vec![],
                scanned: 0,
                next: None,
            }),
        );
        send(&mut model, key(KeyCode::Enter));
        let result = S3Result {
            object: Some(head(
                "reports/events.parquet",
                1_048_576,
                "application/octet-stream",
            )),
            preview: Some(S3Preview {
                kind: S3PreviewKind::Binary,
                format: S3TextFormat::Text,
                encoding: "identity",
                range_start: 0,
                range_end: 65_536,
                next_offset: Some(65_536),
                utf8_head_skipped: 0,
                utf8_tail_cut: 0,
                well_formed: None,
                text: None,
                hex: Some(
                    "50415231150415800000000000000000000000000000000000000000000000000102".into(),
                ),
            }),
            ..S3Result::default()
        };
        answer(&mut model, Ok(Answered::Preview(Box::new(result))));
        model
    }

    /// `d` on the previewed CSV: the request, not run.
    pub fn handoff() -> S3Model {
        let mut model = preview();
        send(&mut model, key(KeyCode::Char('d')));
        model
    }

    /// S3 refused the level.
    pub fn error() -> S3Model {
        let mut model = loading();
        answer(
            &mut model,
            Err(Failure {
                message: "S3_REJECTED: S3 rejected the request: AccessDenied\nhint: check the role's s3:ListBucket on the bucket".into(),
                stopped: false,
            }),
        );
        model
    }

    pub fn help() -> S3Model {
        let mut model = loaded();
        send(&mut model, key(KeyCode::Char('?')));
        model
    }

    /// Long and full-width keys under a long prefix.
    pub fn long_text() -> S3Model {
        let mut model = S3Model::new(
            summary(),
            Some(at("a-very-long-prefix-name-that-goes-on/and-on-and-on/")),
        );
        super::super::update::start(&mut model);
        answer(
            &mut model,
            Ok(Answered::Level {
                prefixes: vec![
                    "a-very-long-prefix-name-that-goes-on/and-on-and-on/日本語のプレフィックス/"
                        .into(),
                ],
                objects: vec![object(
                    "a-very-long-prefix-name-that-goes-on/and-on-and-on/an object key with spaces, 全角文字 and a name far longer than any pane.csv",
                    123_456_789,
                )],
                scanned: 2,
                next: None,
            }),
        );
        model
    }

    pub fn all() -> Vec<(&'static str, S3Model)> {
        vec![
            ("loading", loading()),
            ("loaded", loaded()),
            ("selected", selected()),
            ("buckets", buckets()),
            ("filtering", filtering()),
            ("empty", empty()),
            ("form", form()),
            ("key_search_running", key_search_running()),
            ("stopped", stopped()),
            ("content_search", content_search()),
            ("preview", preview()),
            ("preview_binary", preview_binary()),
            ("handoff", handoff()),
            ("error", error()),
            ("help", help()),
            ("long_text", long_text()),
        ]
    }
}
