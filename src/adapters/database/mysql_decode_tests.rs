//! MySQL binary values, read from the bytes the protocol really sends.
use super::*;

fn text(type_name: &str, bytes: &[u8]) -> String {
    let (text, encoding) =
        decode(type_name, bytes).unwrap_or_else(|error| panic!("{type_name}: {error:?}"));
    assert_eq!(encoding, DbEncoding::Text, "{type_name}");
    text
}

/// A temporal value as the protocol sends it: a length byte, then the payload.
fn packed(payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![payload.len() as u8];
    bytes.extend(payload);
    bytes
}

fn date_payload(year: u16, month: u8, day: u8) -> Vec<u8> {
    let mut bytes = year.to_le_bytes().to_vec();
    bytes.extend([month, day]);
    bytes
}

#[test]
fn db_mysql_integers_are_little_endian_and_keep_their_sign() {
    assert_eq!(text("TINYINT", &[0xff]), "-1");
    assert_eq!(text("TINYINT UNSIGNED", &[0xff]), "255");
    assert_eq!(text("SMALLINT", &(-2i16).to_le_bytes()), "-2");
    assert_eq!(text("INT", &(-3i32).to_le_bytes()), "-3");
    assert_eq!(
        text("BIGINT", &i64::MIN.to_le_bytes()),
        "-9223372036854775808"
    );
    assert_eq!(
        text("BIGINT UNSIGNED", &u64::MAX.to_le_bytes()),
        "18446744073709551615",
        "the largest unsigned integer survives as digits"
    );
    // sqlx names a TINYINT(1) BOOLEAN; MySQL stores and prints 0 and 1.
    assert_eq!(text("BOOLEAN", &[1]), "1");
    assert_eq!(text("YEAR", &2026u16.to_le_bytes()), "2026");
    assert_eq!(text("BIT", &[0x01, 0x00]), "256", "BIT is big-endian");
}

#[test]
fn db_mysql_decimal_json_and_text_arrive_as_the_server_printed_them() {
    assert_eq!(
        text("DECIMAL", b"12345678901234567890.123456789"),
        "12345678901234567890.123456789",
        "a DECIMAL is never routed through a float"
    );
    assert_eq!(text("JSON", br#"{"a": 1}"#), r#"{"a": 1}"#);
    assert_eq!(text("VARCHAR", "日本語".as_bytes()), "日本語");
    assert_eq!(text("TEXT", b""), "");
    assert_eq!(text("ENUM", b"open"), "open");
    assert_eq!(text("FLOAT", &0.1f32.to_le_bytes()), "0.1");
    assert_eq!(text("DOUBLE", &0.1f64.to_le_bytes()), "0.1");
    let (encoded, encoding) = decode("BLOB", &[0, 1, 2]).unwrap();
    assert_eq!((encoded.as_str(), encoding), ("AAEC", DbEncoding::Base64));
}

#[test]
fn db_mysql_temporal_values_say_as_much_as_was_stored() {
    assert_eq!(
        text("DATE", &packed(&date_payload(2026, 9, 21))),
        "2026-09-21"
    );
    let mut seconds = date_payload(2026, 9, 21);
    seconds.extend([12, 34, 56]);
    assert_eq!(text("DATETIME", &packed(&seconds)), "2026-09-21 12:34:56");
    let mut micros = seconds.clone();
    micros.extend(500_000u32.to_le_bytes());
    assert_eq!(text("TIMESTAMP", &packed(&micros)), "2026-09-21 12:34:56.5");
    // The zero date is a value MySQL stores, not a missing one.
    assert_eq!(text("DATE", &packed(&[])), "0000-00-00 00:00:00");
    // A payload the driver already unwrapped is read the same way.
    assert_eq!(text("DATE", &date_payload(2026, 9, 21)), "2026-09-21");
}

#[test]
fn db_mysql_time_is_a_span_that_can_be_negative_and_pass_a_day() {
    let span = |negative: u8, days: u32, hour: u8, minute: u8, second: u8| {
        let mut bytes = vec![negative];
        bytes.extend(days.to_le_bytes());
        bytes.extend([hour, minute, second]);
        bytes
    };
    assert_eq!(text("TIME", &packed(&span(0, 0, 12, 0, 0))), "12:00:00");
    assert_eq!(
        text("TIME", &packed(&span(0, 2, 3, 0, 0))),
        "51:00:00",
        "two days and three hours is 51 hours, not a clock time"
    );
    assert_eq!(text("TIME", &packed(&span(1, 0, 1, 2, 3))), "-01:02:03");
    assert_eq!(text("TIME", &packed(&[])), "00:00:00");
}

#[test]
fn db_mysql_says_which_values_it_cannot_read_instead_of_returning_nothing() {
    assert_eq!(decode("GEOMETRY", &[0; 4]), Err(DecodeError::Unsupported));
    assert_eq!(decode("INT", &[0, 1]), Err(DecodeError::Unreadable));
    assert_eq!(decode("DOUBLE", &[0; 4]), Err(DecodeError::Unreadable));
    assert_eq!(decode("TEXT", &[0xff, 0xfe]), Err(DecodeError::Unreadable));
    assert_eq!(decode("DATE", &[9, 9, 9]), Err(DecodeError::Unreadable));
    assert_eq!(decode("TIME", &[1, 2, 3]), Err(DecodeError::Unreadable));
}
