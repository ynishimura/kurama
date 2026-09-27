//! Read an object's bytes as text within bounds: UTF-8 boundaries, binary detection, bounded gzip decoding and literal line search; no I/O.
use crate::domain::types::{limits::S3_READ, s3_object::S3TextFormat};
use std::io::Read;

/// The layout the key's extension or the content type names.
pub fn text_format(key: &str, content_type: Option<&str>) -> S3TextFormat {
    let name = key.strip_suffix(".gz").unwrap_or(key);
    let extension = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    match (extension.as_deref(), content_type.unwrap_or("")) {
        (Some("json"), _) | (_, "application/json") => S3TextFormat::Json,
        (Some("jsonl" | "ndjson"), _) | (_, "application/x-ndjson") => S3TextFormat::Jsonl,
        (Some("yaml" | "yml"), _) | (_, "application/yaml") => S3TextFormat::Yaml,
        (Some("csv"), _) | (_, "text/csv") => S3TextFormat::Csv,
        _ => S3TextFormat::Text,
    }
}

/// Bytes read from somewhere in an object, as text or as binary.
#[derive(Debug, PartialEq, Eq)]
pub enum Decoded {
    Text {
        text: String,
        /// Continuation bytes skipped at the start: the range began inside a
        /// character.
        head_skipped: usize,
        /// Bytes of a character the range cut at the end, held back.
        tail_cut: usize,
    },
    /// Not UTF-8 or holding a NUL: the first bytes as hex.
    Binary { hex: String },
}

/// Read `bytes` as UTF-8. `at_start` and `at_end` say whether they begin and
/// end where the object does; a range that begins or ends inside a character
/// is trimmed to whole characters instead of being taken for binary.
pub fn decode_text(bytes: &[u8], at_start: bool, at_end: bool) -> Decoded {
    let head_skipped = if at_start {
        0
    } else {
        bytes
            .iter()
            .take(3)
            .take_while(|b| (**b & 0b1100_0000) == 0b1000_0000)
            .count()
    };
    let body = &bytes[head_skipped..];
    let (valid, tail_cut) = match std::str::from_utf8(body) {
        Ok(text) => (text, 0),
        // An incomplete character at the very end, of a range that stops
        // before the object does, is the next range's first character.
        Err(error) if error.error_len().is_none() && !at_end => (
            std::str::from_utf8(&body[..error.valid_up_to()]).expect("valid up to here"),
            body.len() - error.valid_up_to(),
        ),
        Err(_) => return binary(bytes),
    };
    if valid.contains('\0') {
        return binary(bytes);
    }
    Decoded::Text {
        text: valid.to_owned(),
        head_skipped,
        tail_cut,
    }
}

fn binary(bytes: &[u8]) -> Decoded {
    Decoded::Binary {
        hex: bytes
            .iter()
            .take(S3_READ.hex_bytes)
            .map(|b| format!("{b:02x}"))
            .collect(),
    }
}

/// Where decoding a gzip stream stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GunzipEnd {
    /// The stream ended: every byte of it was decoded.
    Finished,
    /// The decoded bytes reached the limit.
    Limit,
    /// The compressed bytes ran out before the stream ended.
    Input,
    /// The bytes are not a gzip stream, or one that is broken.
    Invalid,
}

/// Decode `compressed` from its first byte to at most `limit` bytes. What
/// decoded before a stop is kept; only `Finished` means the whole stream was
/// read and checked.
pub fn gunzip(compressed: &[u8], limit: usize) -> (Vec<u8>, GunzipEnd) {
    let mut out = Vec::new();
    let read = flate2::read::MultiGzDecoder::new(compressed)
        .take(limit as u64 + 1)
        .read_to_end(&mut out);
    let end = match read {
        Ok(_) if out.len() > limit => {
            out.truncate(limit);
            GunzipEnd::Limit
        }
        Ok(_) => GunzipEnd::Finished,
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => GunzipEnd::Input,
        Err(_) => GunzipEnd::Invalid,
    };
    (out, end)
}

/// Whether the text shown is well-formed for its format: one JSON document,
/// or one JSON value on every whole line. `None` for the other formats.
pub fn well_formed(format: S3TextFormat, text: &str, whole: bool) -> Option<bool> {
    match format {
        S3TextFormat::Json => Some(serde_json::from_str::<serde_json::Value>(text).is_ok()),
        S3TextFormat::Jsonl => {
            let mut lines: Vec<&str> = text.lines().collect();
            // The last line of a range that stops early is only its start.
            if !whole && !text.ends_with('\n') {
                lines.pop();
            }
            Some(
                lines
                    .iter()
                    .filter(|line| !line.trim().is_empty())
                    .all(|line| serde_json::from_str::<serde_json::Value>(line).is_ok()),
            )
        }
        _ => None,
    }
}

/// One line that contains the text.
#[derive(Debug, PartialEq, Eq)]
pub struct LineMatch {
    pub line: u64,
    pub byte_offset: u64,
    pub excerpt: String,
}

/// Every line of `text` that contains `needle` byte for byte, up to `max`,
/// each with an excerpt of at most `S3_READ.excerpt_chars` characters around
/// the first occurrence. The second value says whether `max` stopped it.
pub fn find_lines(text: &str, needle: &str, max: usize) -> (Vec<LineMatch>, bool) {
    let mut found = vec![];
    let mut offset = 0u64;
    for (index, raw) in text.split_inclusive('\n').enumerate() {
        let line = raw.trim_end_matches(['\n', '\r']);
        if let Some(at) = line.find(needle) {
            if found.len() == max {
                return (found, true);
            }
            found.push(LineMatch {
                line: index as u64 + 1,
                byte_offset: offset,
                excerpt: excerpt(line, at),
            });
        }
        offset += raw.len() as u64;
    }
    (found, false)
}

/// At most `excerpt_chars` characters of `line`, starting a quarter of that
/// before the match so the match is in it.
fn excerpt(line: &str, at: usize) -> String {
    let width = S3_READ.excerpt_chars;
    let before = line[..at].chars().count();
    let start = before.saturating_sub(width / 4);
    line.chars().skip(start).take(width).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(vec![], flate2::Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn s3_text_format_comes_from_the_name_then_the_type() {
        for (key, content_type, format) in [
            ("a/r.json", None, S3TextFormat::Json),
            ("a/r.JSONL.gz", None, S3TextFormat::Jsonl),
            ("a/r.ndjson", None, S3TextFormat::Jsonl),
            ("a/r.yml", None, S3TextFormat::Yaml),
            ("a/r.csv.gz", None, S3TextFormat::Csv),
            ("a/r", Some("application/json"), S3TextFormat::Json),
            ("a/r.log", Some("text/plain"), S3TextFormat::Text),
            ("a/json", None, S3TextFormat::Text),
        ] {
            assert_eq!(text_format(key, content_type), format, "{key}");
        }
    }

    #[test]
    fn s3_decode_text_trims_a_range_to_whole_characters() {
        let text = "aβc".as_bytes(); // β is 0xCE 0xB2
        assert_eq!(
            decode_text(&text[..2], true, false),
            Decoded::Text {
                text: "a".into(),
                head_skipped: 0,
                tail_cut: 1
            }
        );
        assert_eq!(
            decode_text(&text[2..], false, true),
            Decoded::Text {
                text: "c".into(),
                head_skipped: 1,
                tail_cut: 0
            }
        );
        // At the end of the object a cut character is not the next range's:
        // the bytes are not UTF-8.
        assert!(matches!(
            decode_text(&text[..2], true, true),
            Decoded::Binary { .. }
        ));
        // At the start of the object a continuation byte is not skipped.
        assert!(matches!(
            decode_text(&text[2..], true, true),
            Decoded::Binary { .. }
        ));
    }

    #[test]
    fn s3_decode_text_shows_binary_as_a_hex_head() {
        let bytes: Vec<u8> = (0..=255).collect();
        let Decoded::Binary { hex } = decode_text(&bytes, true, true) else {
            panic!("binary");
        };
        assert_eq!(hex.len(), S3_READ.hex_bytes * 2);
        assert!(hex.starts_with("000102"));
        assert!(matches!(
            decode_text(b"text\0more", true, true),
            Decoded::Binary { .. }
        ));
        assert_eq!(
            decode_text(b"", true, true),
            Decoded::Text {
                text: String::new(),
                head_skipped: 0,
                tail_cut: 0
            }
        );
    }

    #[test]
    fn s3_gunzip_tells_a_finished_stream_from_a_stopped_one() {
        let plain = b"line one\nline two\n".repeat(50);
        let compressed = gzip(&plain);
        assert_eq!(
            gunzip(&compressed, plain.len()),
            (plain.clone(), GunzipEnd::Finished)
        );
        assert_eq!(
            gunzip(&compressed, 10),
            (plain[..10].to_vec(), GunzipEnd::Limit)
        );
        let (partial, end) = gunzip(&compressed[..compressed.len() / 2], plain.len());
        assert_eq!(end, GunzipEnd::Input);
        assert!(plain.starts_with(&partial));
        assert_eq!(
            gunzip(b"this is not a gzip stream at all", 100).1,
            GunzipEnd::Invalid
        );
        let mut two = gzip(b"a\n");
        two.extend(gzip(b"b\n"));
        assert_eq!(gunzip(&two, 100), (b"a\nb\n".to_vec(), GunzipEnd::Finished));
    }

    #[test]
    fn s3_well_formed_tells_whole_json_from_a_cut_one() {
        assert_eq!(
            well_formed(S3TextFormat::Json, "{\"a\":1}", true),
            Some(true)
        );
        assert_eq!(
            well_formed(S3TextFormat::Json, "{\"a\":", false),
            Some(false)
        );
        assert_eq!(
            well_formed(S3TextFormat::Jsonl, "{\"a\":1}\n{\"b\":", false),
            Some(true)
        );
        assert_eq!(
            well_formed(S3TextFormat::Jsonl, "{\"a\":1}\n{\"b\":", true),
            Some(false)
        );
        assert_eq!(well_formed(S3TextFormat::Csv, "a,b", true), None);
    }

    #[test]
    fn s3_find_lines_numbers_each_matching_line_and_stops_at_the_bound() {
        let text = "alpha\r\nbeta id-1\ngamma\nid-1 and id-1\n";
        let (found, stopped) = find_lines(text, "id-1", 10);
        assert!(!stopped);
        assert_eq!(
            found,
            [
                LineMatch {
                    line: 2,
                    byte_offset: 7,
                    excerpt: "beta id-1".into()
                },
                LineMatch {
                    line: 4,
                    byte_offset: 23,
                    excerpt: "id-1 and id-1".into()
                }
            ]
        );
        let (found, stopped) = find_lines(text, "id-1", 1);
        assert_eq!((found.len(), stopped), (1, true));
        // At the bound exactly, nothing is left over: not stopped.
        assert!(!find_lines(text, "id-1", 2).1);
        assert_eq!(find_lines(text, "ID-1", 10).0, []);
    }

    #[test]
    fn s3_an_excerpt_is_cut_around_the_match() {
        let line = format!("{}needle{}", "é".repeat(1000), "x".repeat(1000));
        let (found, _) = find_lines(&line, "needle", 1);
        let excerpt = &found[0].excerpt;
        assert_eq!(excerpt.chars().count(), S3_READ.excerpt_chars);
        assert!(excerpt.contains("needle"), "{excerpt}");
        assert!(excerpt.starts_with('é'));
    }
}
