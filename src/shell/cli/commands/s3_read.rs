//! `kurama s3 --head / --preview / --search-content`: one object's metadata, a bounded range of its bytes, or the lines of the listed objects that contain a text.
use crate::{
    adapters::aws::{s3_data::BucketClients, s3_object},
    domain::{
        functions::s3_text::{
            Decoded, GunzipEnd, decode_text, find_lines, gunzip, text_format, well_formed,
        },
        types::{
            limits::S3_READ,
            s3_browse::{S3Error, S3Invalid, S3Object, S3Result, S3StopReason},
            s3_object::{
                S3Match, S3Preview, S3PreviewKind, S3Searched, S3SkipReason, S3Skipped,
                is_archive_class, is_gzip,
            },
        },
    },
    shell::cli::client::{safe_text, tab_separated},
};
use serde_json::Value;

pub async fn head(
    clients: &mut BucketClients,
    bucket: &str,
    key: &str,
) -> anyhow::Result<S3Result> {
    let object = s3_object::head_object(clients, bucket, key).await?;
    Ok(S3Result {
        scanned_objects: 1,
        complete: true,
        object: Some(object),
        ..S3Result::default()
    })
}

/// What one `--preview` asks for.
pub struct PreviewRequest<'a> {
    pub bucket: &'a str,
    pub key: &'a str,
    pub bytes: u64,
    pub offset: u64,
    pub if_match: Option<&'a str>,
}

/// HEAD the object, then read one range of it while its ETag is still the
/// one HEAD (or `--if-match`) named.
pub async fn preview(
    clients: &mut BucketClients,
    request: PreviewRequest<'_>,
) -> anyhow::Result<S3Result> {
    let PreviewRequest {
        bucket,
        key,
        bytes,
        offset,
        if_match,
    } = request;
    let head = s3_object::head_object(clients, bucket, key).await?;
    if let Some(expected) = if_match
        && head.etag.as_deref().map(unquoted) != Some(unquoted(expected))
    {
        return Err(S3Error::Changed.into());
    }
    let etag = head.etag.clone();
    let format = text_format(key, head.content_type.as_deref());
    let size = head.size;
    let mut result = S3Result {
        scanned_objects: 1,
        bytes_read: Some(0),
        bytes_decoded: Some(0),
        ..S3Result::default()
    };
    if offset > 0 && offset >= size {
        return Err(S3Invalid::OffsetPastEnd { offset, size }.into());
    }
    if head.archived() {
        result.skipped = Some(vec![S3Skipped {
            key: key.to_owned(),
            reason: S3SkipReason::Archived,
        }]);
        result.object = Some(head);
        return Ok(result);
    }
    let mut preview = S3Preview {
        kind: S3PreviewKind::Empty,
        format,
        encoding: "identity",
        range_start: offset,
        range_end: offset,
        next_offset: None,
        utf8_head_skipped: 0,
        utf8_tail_cut: 0,
        well_formed: None,
        text: None,
        hex: None,
    };
    if size == 0 {
        result.complete = true;
    } else if head.gzip() {
        if offset > 0 {
            return Err(S3Invalid::GzipOffset.into());
        }
        let end = size.min(S3_READ.gzip_transfer_bytes);
        let compressed =
            s3_object::get_range(clients, bucket, key, (0, end), etag.as_deref()).await?;
        let (plain, stop) = gunzip(&compressed, bytes as usize);
        let finished = stop == GunzipEnd::Finished;
        result.stop_reason = match stop {
            GunzipEnd::Finished => None,
            GunzipEnd::Limit => Some(S3StopReason::MaxBytes),
            GunzipEnd::Input if end < size => Some(S3StopReason::MaxTransfer),
            GunzipEnd::Input | GunzipEnd::Invalid => Some(S3StopReason::InvalidGzip),
        };
        result.complete = finished;
        result.bytes_read = Some(compressed.len() as u64);
        result.bytes_decoded = Some(plain.len() as u64);
        preview.encoding = "gzip";
        preview.range_end = compressed.len() as u64;
        show(&mut preview, decode_text(&plain, true, finished), finished);
    } else {
        let end = size.min(offset.saturating_add(bytes));
        let data =
            s3_object::get_range(clients, bucket, key, (offset, end), etag.as_deref()).await?;
        let read_to = offset + data.len() as u64;
        let decoded = decode_text(&data, offset == 0, read_to == size);
        let next = match &decoded {
            Decoded::Text { tail_cut, .. } => read_to - *tail_cut as u64,
            Decoded::Binary { .. } => read_to,
        };
        preview.range_end = read_to;
        preview.next_offset = (next < size).then_some(next);
        result.complete = preview.next_offset.is_none();
        result.stop_reason = (!result.complete).then_some(S3StopReason::MaxBytes);
        result.bytes_read = Some(data.len() as u64);
        result.bytes_decoded = Some(data.len() as u64);
        show(&mut preview, decoded, offset == 0 && result.complete);
    }
    result.object = Some(head);
    result.preview = Some(preview);
    Ok(result)
}

fn show(preview: &mut S3Preview, decoded: Decoded, whole: bool) {
    match decoded {
        Decoded::Text {
            text,
            head_skipped,
            tail_cut,
        } => {
            preview.kind = S3PreviewKind::Text;
            preview.utf8_head_skipped = head_skipped;
            preview.utf8_tail_cut = tail_cut;
            preview.well_formed = well_formed(preview.format, &text, whole);
            preview.text = Some(text);
        }
        Decoded::Binary { hex } => {
            preview.kind = S3PreviewKind::Binary;
            preview.hex = Some(hex);
        }
    }
}

/// An ETag as S3 writes it is quoted; one passed on a command line may not be.
fn unquoted(etag: &str) -> &str {
    etag.trim_matches('"')
}

/// What a content search read and found in the objects a listing named.
#[derive(Debug, Default)]
pub struct ContentSearch {
    pub matches: Vec<S3Match>,
    pub searched: Vec<S3Searched>,
    pub skipped: Vec<S3Skipped>,
    pub bytes_read: u64,
    pub bytes_decoded: u64,
    pub stop: Option<S3StopReason>,
}

/// Read each listed object in turn, while the bounds allow, and keep every
/// line that contains `needle`. A refusal or a network failure ends the run
/// with an error whatever was found before it; an object that changed since
/// it was listed is skipped and named.
pub async fn search_contents(
    clients: &mut BucketClients,
    bucket: &str,
    objects: &[S3Object],
    needle: &str,
    now: impl Fn() -> String,
) -> Result<ContentSearch, S3Error> {
    let mut run = ContentSearch::default();
    for object in objects {
        run.read(clients, bucket, object, needle, &now).await?;
    }
    Ok(run)
}

impl ContentSearch {
    /// Search the next listed object, within what is left of the run's
    /// bounds. The explorer calls this one object at a time so that a stop
    /// starts no further GET.
    pub async fn read(
        &mut self,
        clients: &mut BucketClients,
        bucket: &str,
        object: &S3Object,
        needle: &str,
        now: impl Fn() -> String,
    ) -> Result<(), S3Error> {
        let run = self;
        let skip = |reason| S3Skipped {
            key: object.key.clone(),
            reason,
        };
        let reason = if run.stop.is_some() {
            Some(S3SkipReason::NotReached)
        } else if object
            .storage_class
            .as_deref()
            .is_some_and(is_archive_class)
        {
            Some(S3SkipReason::Archived)
        } else if object.size > S3_READ.object_bytes {
            Some(S3SkipReason::TooLarge)
        } else if run.bytes_read + object.size > S3_READ.total_bytes
            || run.bytes_decoded >= S3_READ.total_bytes
        {
            run.stop = Some(S3StopReason::MaxBytes);
            Some(S3SkipReason::NotReached)
        } else {
            None
        };
        if let Some(reason) = reason {
            run.skipped.push(skip(reason));
            return Ok(());
        }
        let data = if object.size == 0 {
            vec![]
        } else {
            match s3_object::get_range(
                clients,
                bucket,
                &object.key,
                (0, object.size),
                object.etag.as_deref(),
            )
            .await
            {
                Err(S3Error::Changed) => {
                    run.skipped.push(skip(S3SkipReason::Changed));
                    return Ok(());
                }
                read => read?,
            }
        };
        let fetched_at = now();
        let data_len = data.len() as u64;
        run.bytes_read += data_len;
        let budget = S3_READ.total_bytes - run.bytes_decoded;
        let limit = S3_READ.object_bytes.min(budget) as usize;
        let (plain, full) = if is_gzip(&object.key, None, None) || data.starts_with(&[0x1f, 0x8b]) {
            match gunzip(&data, limit) {
                (plain, GunzipEnd::Finished) => (plain, true),
                (plain, GunzipEnd::Limit) => (plain, false),
                _ => {
                    run.skipped.push(skip(S3SkipReason::InvalidGzip));
                    return Ok(());
                }
            }
        } else if data.len() > limit {
            (data[..limit].to_vec(), false)
        } else {
            (data, true)
        };
        run.bytes_decoded += plain.len() as u64;
        if !full && (limit as u64) < S3_READ.object_bytes {
            run.stop = Some(S3StopReason::MaxBytes);
        }
        let Decoded::Text { text, .. } = decode_text(&plain, true, full) else {
            run.skipped.push(skip(S3SkipReason::Binary));
            return Ok(());
        };
        let (found, capped) = find_lines(&text, needle, S3_READ.matches - run.matches.len());
        if capped {
            run.stop = Some(S3StopReason::MaxMatches);
        }
        run.searched.push(S3Searched {
            key: object.key.clone(),
            etag: object.etag.clone(),
            full: full && !capped,
            bytes_read: data_len,
            bytes_decoded: plain.len() as u64,
            matches: found.len(),
        });
        run.matches.extend(found.into_iter().map(|found| S3Match {
            bucket: bucket.to_owned(),
            key: object.key.clone(),
            line: found.line,
            byte_offset: found.byte_offset,
            excerpt: found.excerpt,
            fetched_at: fetched_at.clone(),
        }));
        Ok(())
    }
}

/// The text form of a head, a preview or a content search: data on stdout,
/// what was not shown on stderr.
pub fn print_text(result: &S3Result) {
    if let Some(preview) = &result.preview {
        if let Some(text) = &preview.text {
            for line in text.split_inclusive('\n') {
                println!("{}", safe_text(line.trim_end_matches('\n')));
            }
        }
        if let Some(hex) = &preview.hex {
            println!("{hex}");
            crate::console::progress!("# binary: the first bytes as hex");
        }
        match (preview.next_offset, result.stop_reason) {
            (Some(next), _) => crate::console::progress!(
                "# bytes {}-{} shown; go on with --offset-bytes {next}",
                preview.range_start,
                preview.range_end
            ),
            (None, Some(reason)) => crate::console::progress!("# stopped: {}", name(reason)),
            (None, None) => {}
        }
    } else if let Some(object) = &result.object {
        let value = serde_json::to_value(object).expect("an object head serializes");
        let rows: Vec<Vec<Value>> = value
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(_, v)| !v.is_null())
            .map(|(k, v)| vec![k.clone().into(), v.clone()])
            .collect();
        print!("{}", tab_separated(["field", "value"].into_iter(), &rows));
    }
    if let Some(matches) = &result.matches {
        let rows: Vec<Vec<Value>> = matches
            .iter()
            .map(|m| {
                vec![
                    m.key.clone().into(),
                    m.line.into(),
                    m.excerpt.clone().into(),
                ]
            })
            .collect();
        print!(
            "{}",
            tab_separated(["key", "line", "excerpt"].into_iter(), &rows)
        );
        if !result.complete {
            crate::console::progress!(
                "# incomplete: {} objects listed, {} not searched{}{}",
                result.scanned_objects,
                result.skipped.as_ref().map_or(0, Vec::len),
                result
                    .stop_reason
                    .map(|r| format!(", stopped: {}", name(r)))
                    .unwrap_or_default(),
                result
                    .cursor
                    .as_ref()
                    .map(|c| format!("; go on with --cursor {c}"))
                    .unwrap_or_default()
            );
        }
    }
    for skipped in result.skipped.iter().flatten() {
        crate::console::progress!(
            "# skipped {}: {}",
            safe_text(&skipped.key),
            name(skipped.reason)
        );
    }
}

/// The name a reason has in the JSON result.
fn name(reason: impl serde::Serialize) -> String {
    match serde_json::to_value(reason).expect("a reason serializes") {
        Value::String(name) => name,
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_http_client::test_util::infallible_client_fn;
    use std::sync::{Arc, Mutex};

    fn object(key: &str, size: u64) -> S3Object {
        S3Object {
            key: key.into(),
            size,
            last_modified: None,
            etag: Some(format!("\"{key}\"")),
            storage_class: None,
        }
    }

    /// S3 whose every object holds `body`, recording the keys it was asked
    /// for; a key named `changed` answers 412.
    fn clients(body: &'static [u8], asked: Arc<Mutex<Vec<String>>>) -> BucketClients {
        BucketClients::for_tests(
            infallible_client_fn(move |request| {
                let path = request.uri().to_string();
                asked.lock().unwrap().push(path.clone());
                if path.contains("changed") {
                    return http::Response::builder()
                        .status(412)
                        .body("<Error><Code>PreconditionFailed</Code></Error>".as_bytes())
                        .unwrap();
                }
                http::Response::builder().status(206).body(body).unwrap()
            }),
            "ap-northeast-1",
            true,
        )
    }

    #[tokio::test]
    async fn s3_content_search_reads_what_the_bounds_allow_and_names_the_rest() {
        let asked = Arc::new(Mutex::new(vec![]));
        let mut clients = clients(b"one\nid-1 here\n", asked.clone());
        let archived = S3Object {
            storage_class: Some("DEEP_ARCHIVE".into()),
            ..object("cold.txt", 5)
        };
        let objects = [
            object("a.txt", 14),
            object("empty.txt", 0),
            archived,
            object("huge.txt", S3_READ.object_bytes + 1),
            object("changed.txt", 14),
            object("b.txt", 14),
        ];
        let run = search_contents(&mut clients, "bucket", &objects, "id-1", || "t".into())
            .await
            .unwrap();
        assert_eq!(
            run.matches
                .iter()
                .map(|m| (m.key.as_str(), m.line, m.byte_offset))
                .collect::<Vec<_>>(),
            [("a.txt", 2, 4), ("b.txt", 2, 4)]
        );
        assert_eq!(
            run.skipped
                .iter()
                .map(|s| (s.key.as_str(), s.reason))
                .collect::<Vec<_>>(),
            [
                ("cold.txt", S3SkipReason::Archived),
                ("huge.txt", S3SkipReason::TooLarge),
                ("changed.txt", S3SkipReason::Changed),
            ]
        );
        assert_eq!(run.searched.len(), 3);
        assert!(run.searched.iter().all(|s| s.full));
        assert_eq!((run.bytes_read, run.stop), (28, None));
        // Neither the empty, the archived nor the huge object was asked for.
        assert_eq!(asked.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn s3_content_search_stops_at_the_match_bound_and_names_what_it_did_not_reach() {
        let body: &'static [u8] =
            Box::leak("x\n".repeat(S3_READ.matches + 1).into_boxed_str()).as_bytes();
        let size = body.len() as u64;
        let mut clients = clients(body, Arc::default());
        let objects = [object("a.txt", size), object("b.txt", size)];
        let run = search_contents(&mut clients, "bucket", &objects, "x", || "t".into())
            .await
            .unwrap();
        assert_eq!(run.matches.len(), S3_READ.matches);
        assert_eq!(run.stop, Some(S3StopReason::MaxMatches));
        assert!(!run.searched[0].full);
        assert_eq!(
            run.skipped,
            [S3Skipped {
                key: "b.txt".into(),
                reason: S3SkipReason::NotReached
            }]
        );
    }

    #[tokio::test]
    async fn s3_content_search_stops_before_the_transfer_bound() {
        let body: &'static [u8] = vec![b'x'; S3_READ.object_bytes as usize].leak();
        let mut clients = clients(body, Arc::default());
        let objects: Vec<S3Object> = (0..7)
            .map(|i| object(&format!("{i}.txt"), S3_READ.object_bytes))
            .collect();
        let run = search_contents(&mut clients, "bucket", &objects, "y", || "t".into())
            .await
            .unwrap();
        // Six objects of 8 MiB fit in 50 MiB; the seventh would not.
        assert_eq!(run.searched.len(), 6);
        assert_eq!(run.stop, Some(S3StopReason::MaxBytes));
        assert_eq!(run.skipped[0].reason, S3SkipReason::NotReached);
    }

    #[tokio::test]
    async fn s3_content_search_fails_instead_of_reporting_no_match() {
        let mut clients = BucketClients::for_tests(
            infallible_client_fn(|_| {
                http::Response::builder()
                    .status(403)
                    .body("<Error><Code>AccessDenied</Code></Error>")
                    .unwrap()
            }),
            "ap-northeast-1",
            true,
        );
        let error = search_contents(&mut clients, "b", &[object("a", 3)], "x", || "t".into())
            .await
            .unwrap_err();
        assert!(matches!(error, S3Error::ReadRejected(ref c) if c == "AccessDenied"));
    }
}
