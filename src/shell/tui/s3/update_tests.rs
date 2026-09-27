use super::*;
use crate::domain::types::s3_browse::S3StopReason;
use crate::shell::tui::s3::model::End;
use crate::shell::tui::s3::testing::{answer, asked, fixtures, key, send};

#[test]
fn s3_enter_walks_from_a_bucket_to_a_prefix_to_an_object() {
    let mut model = fixtures::buckets();
    let effects = send(&mut model, key(KeyCode::Enter));
    assert_eq!(
        asked(&effects),
        Some(S3Ask::Level {
            location: S3Location {
                bucket: "example-assets".into(),
                prefix: String::new(),
            },
            start: S3Position::default(),
        })
    );
    assert_eq!(model.listing.view, View::Level);
    let mut model = fixtures::loaded();
    let effects = send(&mut model, key(KeyCode::Enter));
    assert_eq!(
        asked(&effects),
        Some(S3Ask::Level {
            location: fixtures::at("reports/2024/"),
            start: S3Position::default(),
        })
    );
    let mut model = fixtures::selected();
    let effects = send(&mut model, key(KeyCode::Enter));
    assert_eq!(
        asked(&effects),
        Some(S3Ask::Preview {
            bucket: "example-assets".into(),
            key: "reports/orders.csv".into(),
            offset: 0,
            if_match: None,
        })
    );
}

#[test]
fn s3_left_goes_up_a_level_then_to_the_bucket_list() {
    let mut model = fixtures::loaded();
    let effects = send(&mut model, key(KeyCode::Left));
    assert_eq!(
        asked(&effects),
        Some(S3Ask::Level {
            location: fixtures::at(""),
            start: S3Position::default(),
        })
    );
    answer(
        &mut model,
        Ok(Answered::Level {
            prefixes: vec![],
            objects: vec![],
            scanned: 0,
            next: None,
        }),
    );
    let effects = send(&mut model, key(KeyCode::Backspace));
    assert_eq!(asked(&effects), Some(S3Ask::Buckets));
    assert_eq!(model.listing.view, View::Buckets);
}

#[test]
fn s3_the_next_page_is_asked_for_at_the_last_row_only() {
    let mut model = fixtures::loaded();
    for _ in 0..3 {
        assert!(asked(&send(&mut model, key(KeyCode::Down))).is_none());
    }
    let effects = send(&mut model, key(KeyCode::Down));
    assert!(matches!(
        asked(&effects),
        Some(S3Ask::Level { start, .. }) if start.token.as_deref() == Some("t")
    ));
    // The filter reads only what is here, so it fetches nothing.
    let mut model = fixtures::filtering();
    assert!(asked(&send(&mut model, key(KeyCode::End))).is_none());
    assert_eq!(model.filtered.len(), 1);
}

#[test]
fn s3_a_search_starts_only_from_a_confirmed_form() {
    let mut model = fixtures::loaded();
    assert!(send(&mut model, key(KeyCode::Char('g'))).is_empty());
    for c in "id".chars() {
        assert!(send(&mut model, key(KeyCode::Char(c))).is_empty());
    }
    // Esc drops the form and reads nothing.
    assert!(send(&mut model, key(KeyCode::Esc)).is_empty());
    assert!(model.form.is_none() && model.running.is_none());
    // An empty text or a bound out of range is refused in the form.
    send(&mut model, key(KeyCode::Char('g')));
    assert!(send(&mut model, key(KeyCode::Enter)).is_empty());
    assert!(model.form.as_ref().unwrap().error.is_some());
    let model = fixtures::form();
    assert!(model.running.is_none());
    assert!(
        model
            .form
            .as_ref()
            .unwrap()
            .error
            .as_deref()
            .unwrap()
            .contains("1 to 100000"),
        "{:?}",
        model.form.as_ref().unwrap().error
    );
    let mut model = fixtures::loaded();
    send(&mut model, key(KeyCode::Char('g')));
    for c in "id".chars() {
        send(&mut model, key(KeyCode::Char(c)));
    }
    let effects = send(&mut model, key(KeyCode::Enter));
    assert_eq!(
        asked(&effects),
        Some(S3Ask::ContentSearch {
            location: fixtures::at("reports/"),
            text: "id".into(),
            max_objects: S3_READ.search_objects,
        })
    );
    assert_eq!(model.listing.view, View::ContentSearch("id".into()));
    let mut model = fixtures::loaded();
    send(&mut model, key(KeyCode::Char('s')));
    send(&mut model, key(KeyCode::Char('x')));
    assert!(matches!(
        asked(&send(&mut model, key(KeyCode::Enter))),
        Some(S3Ask::KeySearch { max_objects, .. }) if max_objects == DEFAULT_SEARCH_OBJECTS
    ));
}

#[test]
fn s3_esc_stops_the_search_keeps_what_arrived_and_moving_still_works() {
    let mut model = fixtures::key_search_running();
    // Moving while it runs asks for nothing.
    assert!(asked(&send(&mut model, key(KeyCode::Down))).is_none());
    assert_eq!(model.selected, 1);
    // Enter while it runs starts nothing either.
    assert!(send(&mut model, key(KeyCode::Enter)).is_empty());
    assert_eq!(model.notice.as_deref(), Some(BUSY));
    let effects = send(&mut model, key(KeyCode::Esc));
    assert!(matches!(effects.as_slice(), [S3Effect::Stop]));
    // A second Esc sends no second stop.
    assert!(send(&mut model, key(KeyCode::Esc)).is_empty());
    answer(
        &mut model,
        Err(Failure {
            message: "stopped".into(),
            stopped: true,
        }),
    );
    assert_eq!(model.listing.end, Some(End::Stopped));
    assert_eq!(model.listing.rows.len(), 2);
    assert!(model.modal.is_none() && model.running.is_none());
}

#[test]
fn s3_a_search_that_hits_its_bound_says_so() {
    let model = fixtures::content_search();
    assert_eq!(
        model.listing.end,
        Some(End::Bound(S3StopReason::MaxMatches))
    );
    assert_eq!((model.listing.read, model.listing.skipped), (2, 1));
    assert!(matches!(model.listing.rows[0], Row::Match(_)));
}

#[test]
fn s3_open_as_data_names_the_request_and_runs_nothing() {
    let mut model = fixtures::preview();
    let effects = send(&mut model, key(KeyCode::Char('d')));
    assert!(effects.is_empty());
    let Some(S3Modal::Handoff(action)) = &model.modal else {
        panic!("no handoff modal");
    };
    assert_eq!(action.args.from, "s3://example-assets/reports/orders.csv");
    assert_eq!(action.args.s3_source, "assets");
    // The size HEAD answered, not the listing's.
    assert_eq!(action.provenance.size, 120_000);
    let effects = send(&mut model, key(KeyCode::Char('y')));
    assert!(matches!(
        effects.as_slice(),
        [S3Effect::CopyToClipboard { text }]
            if text == "kurama data --from s3://example-assets/reports/orders.csv --s3-source assets --describe"
    ));
    // From the list, the listed object; a format data does not read is
    // refused in words.
    let mut model = fixtures::selected();
    send(&mut model, key(KeyCode::Char('d')));
    assert!(matches!(&model.modal, Some(S3Modal::Handoff(a)) if a.provenance.size == 20));
    let mut model = fixtures::selected();
    send(&mut model, key(KeyCode::Down));
    send(&mut model, key(KeyCode::Char('d')));
    assert!(model.modal.is_none());
    assert!(model.notice.as_deref().unwrap().contains("summary.json"));
}

#[test]
fn s3_n_reads_the_next_range_of_the_same_version() {
    let mut model = fixtures::preview();
    let effects = send(&mut model, key(KeyCode::Char('n')));
    assert_eq!(
        asked(&effects),
        Some(S3Ask::Preview {
            bucket: "example-assets".into(),
            key: "reports/orders.csv".into(),
            offset: 65_536,
            if_match: Some("\"9b2cf535f27731c974343645a3985328\"".into()),
        })
    );
}

#[test]
fn s3_preview_lines_are_terminal_safe() {
    let model = fixtures::preview();
    let lines = &model.preview.as_ref().unwrap().lines;
    assert_eq!(lines[2], "2,20,tab\\there");
}
