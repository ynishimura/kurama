//! S3 explorer state and its pure transitions.
//!
//! One request runs at a time: a key that would start another while one
//! runs says so and does nothing, and `Esc` stops the running one -- the
//! rows that arrived stay and the list says it was stopped. A key or content
//! search starts only from a form that names its target and bound. No I/O,
//! no clock: the runtime runs the effects and tells the update how long a
//! request has taken.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::effects::{S3Ask, S3Effect};
use super::messages::{Answer, Answered, Failure, Found, S3Message};
use super::model::{
    End, Focus, Listing, PreviewPane, Row, Running, S3Modal, S3Model, SearchForm, View, parent,
};
use crate::domain::functions::s3_handoff::{Observed, data_handoff, handoff_command};
use crate::domain::types::limits::S3_READ;
use crate::domain::types::s3_browse::{
    DEFAULT_SEARCH_OBJECTS, MAX_OBJECTS, S3Location, S3Position, S3Result,
};
use crate::domain::types::s3_object::S3PreviewKind;
use crate::shell::cli::client::safe_text;
use crate::shell::tui::components::line_input::typed;
use crate::shell::tui::components::list_navigation::{PAGE, moved_selection};

/// What the screen says when a key would start a second request.
pub const BUSY: &str = "a request is running; Esc stops it";

pub fn update(model: &mut S3Model, message: S3Message) -> Vec<S3Effect> {
    match message {
        S3Message::Key(key) => on_key(model, key),
        S3Message::Resize => Vec::new(),
        S3Message::Elapsed(seconds) => {
            if let Some(running) = &mut model.running {
                running.elapsed_secs = seconds;
            }
            Vec::new()
        }
        S3Message::Found(found) => {
            on_found(model, found);
            Vec::new()
        }
        S3Message::Answered(answer) => on_answer(model, answer),
        S3Message::Copied(result) => {
            model.notice = Some(match result {
                Ok(()) => "copied to the clipboard".into(),
                Err(message) => format!("copy failed: {message}"),
            });
            Vec::new()
        }
    }
}

/// The first request of the screen: the start level, or the bucket list.
pub fn start(model: &mut S3Model) -> Vec<S3Effect> {
    let location = model.listing.location.clone();
    open(model, location)
}

/// Replace the list with a level, or the bucket list, and ask for its first
/// rows.
fn open(model: &mut S3Model, location: Option<S3Location>) -> Vec<S3Effect> {
    let (view, request) = match &location {
        None => (View::Buckets, S3Ask::Buckets),
        Some(location) => (
            View::Level,
            S3Ask::Level {
                location: location.clone(),
                start: S3Position::default(),
            },
        ),
    };
    let effects = ask(model, request);
    if !effects.is_empty() {
        reset(model, Listing::new(view, location));
    }
    effects
}

fn reset(model: &mut S3Model, listing: Listing) {
    model.listing = listing;
    model.filter.clear();
    model.filtering = false;
    model.filtered.clear();
    model.selected = 0;
    // A preview belongs to the rows it was opened from.
    model.preview = None;
    model.focus = Focus::List;
}

/// Ask for a request, unless one is running.
fn ask(model: &mut S3Model, request: S3Ask) -> Vec<S3Effect> {
    if model.running.is_some() {
        model.notice = Some(BUSY.into());
        return Vec::new();
    }
    model.running = Some(Running {
        ask: request.clone(),
        elapsed_secs: 0,
        stopping: false,
    });
    vec![S3Effect::Ask(request)]
}

fn on_found(model: &mut S3Model, found: Found) {
    let listing = &mut model.listing;
    listing
        .rows
        .extend(found.objects.into_iter().map(Row::Object));
    listing
        .rows
        .extend(found.matches.into_iter().map(Row::Match));
    listing.scanned = found.scanned;
    listing.read = found.read;
    model.apply_filter();
}

fn on_answer(model: &mut S3Model, answer: Answer) -> Vec<S3Effect> {
    model.running = None;
    let Answer { ask, outcome } = answer;
    let lists = !matches!(ask, S3Ask::Preview { .. });
    match outcome {
        Err(Failure { message, stopped }) => {
            if stopped {
                if lists {
                    model.listing.end = Some(End::Stopped);
                } else {
                    model.notice = Some("the read was stopped".into());
                }
            } else {
                if lists {
                    model.listing.end = Some(End::Failed);
                }
                model.modal = Some(S3Modal::Error(message));
            }
            Vec::new()
        }
        Ok(Answered::Buckets(buckets)) => {
            let listing = &mut model.listing;
            listing.scanned = buckets.len() as u64;
            listing.rows = buckets.into_iter().map(Row::Bucket).collect();
            listing.end = Some(End::Complete);
            model.apply_filter();
            Vec::new()
        }
        Ok(Answered::Level {
            prefixes,
            objects,
            scanned,
            next,
        }) => {
            let listing = &mut model.listing;
            listing.rows.extend(prefixes.into_iter().map(Row::Prefix));
            listing.rows.extend(objects.into_iter().map(Row::Object));
            listing.scanned += scanned;
            listing.end = Some(if next.is_some() {
                End::More
            } else {
                End::Complete
            });
            listing.next = next;
            model.apply_filter();
            more_rows(model)
        }
        Ok(Answered::Searched {
            complete,
            stop_reason,
            skipped,
        }) => {
            let listing = &mut model.listing;
            listing.skipped = skipped;
            listing.end = Some(match stop_reason {
                Some(reason) if !complete => End::Bound(reason),
                _ => End::Complete,
            });
            Vec::new()
        }
        Ok(Answered::Preview(result)) => {
            if let S3Ask::Preview { bucket, key, .. } = ask {
                model.preview = Some(PreviewPane {
                    lines: preview_lines(&result),
                    bucket,
                    key,
                    result: *result,
                    scroll: 0,
                });
                model.focus = Focus::Preview;
            }
            Vec::new()
        }
    }
}

/// The lines a preview shows, terminal-safe: the text, a hex head, or why
/// nothing was read.
pub fn preview_lines(result: &S3Result) -> Vec<String> {
    let mut lines = Vec::new();
    if result.skipped.as_ref().is_some_and(|s| !s.is_empty()) {
        lines.push("archived: restore the object before it can be read".to_owned());
        return lines;
    }
    let Some(preview) = &result.preview else {
        return lines;
    };
    match preview.kind {
        S3PreviewKind::Empty => lines.push("(the object is empty)".to_owned()),
        S3PreviewKind::Binary => {
            lines.push("binary: the first bytes as hex".to_owned());
            // Sixteen bytes a line.
            let hex = preview.hex.as_deref().unwrap_or_default();
            lines.extend(
                hex.as_bytes()
                    .chunks(32)
                    .map(|chunk| safe_text(&String::from_utf8_lossy(chunk))),
            );
        }
        S3PreviewKind::Text => lines.extend(
            preview
                .text
                .iter()
                .flat_map(|text| text.lines())
                .map(safe_text),
        ),
    }
    lines
}

/// The next page of a level, once the selection reaches the last row.
fn more_rows(model: &mut S3Model) -> Vec<S3Effect> {
    let at_the_end = model.selected + 1 >= model.filtered.len();
    let (Some(start), Some(location)) = (&model.listing.next, &model.listing.location) else {
        return Vec::new();
    };
    if !at_the_end || !model.filter.is_empty() || model.running.is_some() {
        return Vec::new();
    }
    let request = S3Ask::Level {
        location: location.clone(),
        start: start.clone(),
    };
    let effects = ask(model, request);
    if !effects.is_empty() {
        model.listing.end = None;
    }
    effects
}

fn on_key(model: &mut S3Model, key: KeyEvent) -> Vec<S3Effect> {
    model.notice = None;
    if key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL {
        model.should_exit = true;
        return Vec::new();
    }
    if let Some(modal) = &model.modal {
        if let (S3Modal::Handoff(action), KeyCode::Char('y')) = (modal, key.code) {
            let text = handoff_command(action);
            model.modal = None;
            return vec![S3Effect::CopyToClipboard { text }];
        }
        if matches!(
            key.code,
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char('?')
        ) {
            model.modal = None;
        }
        return Vec::new();
    }
    if key.code == KeyCode::Esc
        && let Some(running) = &mut model.running
    {
        if running.stopping {
            return Vec::new();
        }
        running.stopping = true;
        return vec![S3Effect::Stop];
    }
    if model.form.is_some() {
        return on_form_key(model, key);
    }
    if model.filtering {
        on_filter_key(model, key);
        return Vec::new();
    }
    match key.code {
        KeyCode::Char('q') => {
            model.should_exit = true;
            return Vec::new();
        }
        KeyCode::Char('?') => {
            model.modal = Some(S3Modal::Help);
            return Vec::new();
        }
        KeyCode::Char('d') => return open_as_data(model),
        _ => {}
    }
    match model.focus {
        Focus::List => on_list_key(model, key),
        Focus::Preview => on_preview_key(model, key),
    }
}

fn on_list_key(model: &mut S3Model, key: KeyEvent) -> Vec<S3Effect> {
    match key.code {
        KeyCode::Enter => enter(model),
        KeyCode::Backspace | KeyCode::Left | KeyCode::Char('h') => up(model),
        KeyCode::Esc => {
            if !model.filter.is_empty() {
                model.filter.clear();
                model.apply_filter();
                Vec::new()
            } else if matches!(
                model.listing.view,
                View::KeySearch(_) | View::ContentSearch(_)
            ) {
                let location = model.listing.location.clone();
                open(model, location)
            } else {
                Vec::new()
            }
        }
        KeyCode::Char('/') => {
            model.filtering = true;
            Vec::new()
        }
        KeyCode::Char('s') => search_form(model, false),
        KeyCode::Char('g') => search_form(model, true),
        KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') if model.preview.is_some() => {
            model.focus = Focus::Preview;
            Vec::new()
        }
        code => match moved_selection(code, model.selected, model.filtered.len()) {
            Some(index) => {
                model.selected = index;
                more_rows(model)
            }
            None => Vec::new(),
        },
    }
}

fn on_filter_key(model: &mut S3Model, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            model.filtering = false;
            model.filter.clear();
            model.apply_filter();
        }
        KeyCode::Enter => model.filtering = false,
        KeyCode::Up | KeyCode::Down => {
            if let Some(index) = moved_selection(key.code, model.selected, model.filtered.len()) {
                model.selected = index;
            }
        }
        _ if (typed(key) || !matches!(key.code, KeyCode::Char(_)))
            && model.filter.handle_key(key) =>
        {
            model.selected = 0;
            model.apply_filter();
        }
        _ => {}
    }
}

fn on_preview_key(model: &mut S3Model, key: KeyEvent) -> Vec<S3Effect> {
    let Some(preview) = &mut model.preview else {
        model.focus = Focus::List;
        return Vec::new();
    };
    let last = preview.lines.len().saturating_sub(1);
    match key.code {
        KeyCode::Esc | KeyCode::Tab | KeyCode::Left | KeyCode::Char('h') | KeyCode::Backspace => {
            model.focus = Focus::List;
        }
        KeyCode::Up | KeyCode::Char('k') => preview.scroll = preview.scroll.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => preview.scroll = (preview.scroll + 1).min(last),
        KeyCode::PageUp => preview.scroll = preview.scroll.saturating_sub(PAGE),
        KeyCode::PageDown => preview.scroll = (preview.scroll + PAGE).min(last),
        KeyCode::Home => preview.scroll = 0,
        KeyCode::End => preview.scroll = last,
        KeyCode::Char('n') => {
            let next = preview.result.preview.as_ref().and_then(|p| p.next_offset);
            let Some(offset) = next else {
                model.notice = Some("the whole object, or all a preview reads, is shown".into());
                return Vec::new();
            };
            let request = S3Ask::Preview {
                bucket: preview.bucket.clone(),
                key: preview.key.clone(),
                offset,
                // The next range is read only while the object is the one
                // the first range came from.
                if_match: preview.result.object.as_ref().and_then(|o| o.etag.clone()),
            };
            return ask(model, request);
        }
        _ => {}
    }
    Vec::new()
}

fn on_form_key(model: &mut S3Model, key: KeyEvent) -> Vec<S3Effect> {
    let Some(form) = &mut model.form else {
        return Vec::new();
    };
    match key.code {
        KeyCode::Esc => {
            model.form = None;
            Vec::new()
        }
        KeyCode::Tab | KeyCode::BackTab | KeyCode::Up | KeyCode::Down => {
            form.field = 1 - form.field;
            Vec::new()
        }
        KeyCode::Enter => confirm_search(model),
        _ => {
            let input = if form.field == 0 {
                &mut form.text
            } else {
                &mut form.max_objects
            };
            if typed(key) || !matches!(key.code, KeyCode::Char(_)) {
                input.handle_key(key);
            }
            Vec::new()
        }
    }
}

fn search_form(model: &mut S3Model, content: bool) -> Vec<S3Effect> {
    let Some(location) = model.listing.location.clone() else {
        model.notice = Some("open a bucket first: a search reads one bucket's prefix".into());
        return Vec::new();
    };
    let bound = if content {
        S3_READ.search_objects
    } else {
        DEFAULT_SEARCH_OBJECTS
    };
    model.form = Some(SearchForm {
        content,
        location,
        text: Default::default(),
        max_objects: bound.to_string().into(),
        field: 0,
        error: None,
    });
    Vec::new()
}

fn confirm_search(model: &mut S3Model) -> Vec<S3Effect> {
    let Some(form) = &mut model.form else {
        return Vec::new();
    };
    let text = form.text.as_str().to_owned();
    if text.is_empty() {
        form.error = Some("type the text a key or a line has to contain".into());
        return Vec::new();
    }
    let Some(max_objects) = form
        .max_objects
        .as_str()
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|n| (1..=MAX_OBJECTS).contains(n))
    else {
        form.error = Some(format!("max objects is a number from 1 to {MAX_OBJECTS}"));
        return Vec::new();
    };
    let location = form.location.clone();
    let (view, request) = if form.content {
        (
            View::ContentSearch(text.clone()),
            S3Ask::ContentSearch {
                location: location.clone(),
                text,
                max_objects,
            },
        )
    } else {
        (
            View::KeySearch(text.clone()),
            S3Ask::KeySearch {
                location: location.clone(),
                text,
                max_objects,
            },
        )
    };
    let effects = ask(model, request);
    if !effects.is_empty() {
        model.form = None;
        reset(model, Listing::new(view, Some(location)));
    }
    effects
}

/// Enter on a row: a bucket or a prefix opens that level, an object or a
/// match is previewed.
fn enter(model: &mut S3Model) -> Vec<S3Effect> {
    let Some(row) = model.selected_row().cloned() else {
        return Vec::new();
    };
    let bucket = model.listing.location.as_ref().map(|l| l.bucket.clone());
    match row {
        Row::Bucket(found) => open(
            model,
            Some(S3Location {
                bucket: found.name,
                prefix: String::new(),
            }),
        ),
        Row::Prefix(prefix) => {
            let bucket = bucket.expect("a prefix is listed in a bucket");
            open(model, Some(S3Location { bucket, prefix }))
        }
        Row::Object(object) => preview(model, bucket, object.key),
        Row::Match(found) => preview(model, Some(found.bucket), found.key),
    }
}

fn preview(model: &mut S3Model, bucket: Option<String>, key: String) -> Vec<S3Effect> {
    let bucket = bucket.expect("an object is listed in a bucket");
    ask(
        model,
        S3Ask::Preview {
            bucket,
            key,
            offset: 0,
            if_match: None,
        },
    )
}

/// The level above, or the bucket list above a bucket's root; a search goes
/// back to the level it searched.
fn up(model: &mut S3Model) -> Vec<S3Effect> {
    let Some(location) = model.listing.location.clone() else {
        return Vec::new();
    };
    if model.listing.view != View::Level {
        return open(model, Some(location));
    }
    match parent(&location) {
        Some(above) => open(model, Some(above)),
        None => open(model, None),
    }
}

/// The `kurama data` request for the previewed object or the selected one,
/// in a modal; nothing runs it.
fn open_as_data(model: &mut S3Model) -> Vec<S3Effect> {
    let observed = match (model.focus, &model.preview, model.selected_row()) {
        (Focus::Preview, Some(preview), _) => {
            preview.result.object.as_ref().map(|object| Observed {
                bucket: &preview.bucket,
                key: &object.key,
                etag: object.etag.as_deref(),
                size: object.size,
                last_modified: object.last_modified.as_deref(),
            })
        }
        (Focus::List, _, Some(Row::Object(object))) => {
            model.listing.location.as_ref().map(|l| Observed {
                bucket: &l.bucket,
                key: &object.key,
                etag: object.etag.as_deref(),
                size: object.size,
                last_modified: object.last_modified.as_deref(),
            })
        }
        _ => None,
    };
    let Some(observed) = observed else {
        model.notice = Some("d opens an object as data: select or preview one".into());
        return Vec::new();
    };
    match data_handoff(&model.summary.name, &observed) {
        Some(action) => model.modal = Some(S3Modal::Handoff(action)),
        None => {
            model.notice = Some(format!(
                "kurama data reads CSV, JSONL and Parquet; {} is none of them",
                safe_text(observed.key)
            ));
        }
    }
    Vec::new()
}

#[cfg(test)]
#[path = "update_tests.rs"]
mod tests;
