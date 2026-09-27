//! PostgreSQL binary values, read from the bytes the protocol really sends.
use super::*;

fn text(type_name: &str, bytes: &[u8]) -> String {
    let (text, encoding) = decode(type_name, bytes).unwrap_or_else(|error| {
        panic!("{type_name}: {error:?}");
    });
    assert_eq!(encoding, DbEncoding::Text, "{type_name}");
    text
}

/// `numeric` on the wire: digit count, weight, sign, scale, then base-10000
/// digits.
fn numeric_bytes(weight: i16, sign: u16, scale: u16, digits: &[i16]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend((digits.len() as i16).to_be_bytes());
    bytes.extend(weight.to_be_bytes());
    bytes.extend(sign.to_be_bytes());
    bytes.extend(scale.to_be_bytes());
    for digit in digits {
        bytes.extend(digit.to_be_bytes());
    }
    bytes
}

/// One array dimension, its lower bound and its elements; `None` is NULL.
fn array_bytes(lengths: &[i32], elements: &[Option<Vec<u8>>]) -> Vec<u8> {
    let bounds: Vec<i32> = lengths.iter().map(|_| 1).collect();
    array_bytes_from(lengths, &bounds, elements)
}

/// The same, with the lower bound of each dimension written out: PostgreSQL
/// arrays do not all start at 1, and the literal has to say so.
fn array_bytes_from(
    lengths: &[i32],
    lower_bounds: &[i32],
    elements: &[Option<Vec<u8>>],
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend((lengths.len() as i32).to_be_bytes());
    bytes.extend(0i32.to_be_bytes());
    bytes.extend(23i32.to_be_bytes());
    for (length, lower) in lengths.iter().zip(lower_bounds) {
        bytes.extend(length.to_be_bytes());
        bytes.extend(lower.to_be_bytes());
    }
    for element in elements {
        match element {
            Some(value) => {
                bytes.extend((value.len() as i32).to_be_bytes());
                bytes.extend(value);
            }
            None => bytes.extend((-1i32).to_be_bytes()),
        }
    }
    bytes
}

#[test]
fn db_postgres_numbers_keep_their_own_width_and_their_digits() {
    assert_eq!(text("BOOL", &[1]), "true");
    assert_eq!(text("BOOL", &[0]), "false");
    assert_eq!(text("INT2", &i16::MIN.to_be_bytes()), "-32768");
    assert_eq!(text("INT4", &(-1i32).to_be_bytes()), "-1");
    assert_eq!(
        text("INT8", &i64::MAX.to_be_bytes()),
        "9223372036854775807",
        "a 64-bit integer is not passed through a float"
    );
    // 0.1 as f32 and as f64 are different numbers; neither gains digits.
    assert_eq!(text("FLOAT4", &0.1f32.to_be_bytes()), "0.1");
    assert_eq!(text("FLOAT8", &0.1f64.to_be_bytes()), "0.1");
    assert_eq!(text("FLOAT8", &f64::NAN.to_be_bytes()), "NaN");
}

#[test]
fn db_postgres_numeric_is_exact_and_keeps_the_values_that_are_not_numbers() {
    // 12345.6789 -> digits 1, 2345, 6789 with weight 1 and scale 4.
    assert_eq!(
        text("NUMERIC", &numeric_bytes(1, 0x0000, 4, &[1, 2345, 6789])),
        "12345.6789"
    );
    assert_eq!(
        text("NUMERIC", &numeric_bytes(1, 0x4000, 4, &[1, 2345, 6789])),
        "-12345.6789"
    );
    // 0.00001234: nothing before the point, and two groups of leading zeros.
    assert_eq!(
        text("NUMERIC", &numeric_bytes(-2, 0x0000, 8, &[1234])),
        "0.00001234"
    );
    // A whole number keeps the zeros its scale asks for.
    assert_eq!(text("NUMERIC", &numeric_bytes(0, 0x0000, 2, &[7])), "7.00");
    assert_eq!(text("NUMERIC", &numeric_bytes(0, 0x0000, 0, &[])), "0");
    // No decimal type represents these, which is why the bytes are read here.
    assert_eq!(text("NUMERIC", &numeric_bytes(0, 0xC000, 0, &[])), "NaN");
    assert_eq!(
        text("NUMERIC", &numeric_bytes(0, 0xD000, 0, &[])),
        "Infinity"
    );
    assert_eq!(
        text("NUMERIC", &numeric_bytes(0, 0xF000, 0, &[])),
        "-Infinity"
    );
}

#[test]
fn db_postgres_text_json_and_uuid_arrive_as_they_were_stored() {
    assert_eq!(text("TEXT", "日本語".as_bytes()), "日本語");
    assert_eq!(text("VARCHAR", b""), "");
    assert_eq!(text("JSON", br#"{"a":1}"#), r#"{"a":1}"#);
    // jsonb hides a format version in front of the document.
    assert_eq!(text("JSONB", b"\x01{\"a\":1}"), r#"{"a":1}"#);
    assert_eq!(
        decode("JSONB", b"\x02{}"),
        Err(DecodeError::Unreadable),
        "an unknown jsonb version is not guessed at"
    );
    assert_eq!(
        text(
            "UUID",
            &[
                0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc,
                0xde, 0xf0
            ]
        ),
        "12345678-9abc-def0-1234-56789abcdef0"
    );
    let (encoded, encoding) = decode("BYTEA", &[0, 1, 2]).unwrap();
    assert_eq!((encoded.as_str(), encoding), ("AAEC", DbEncoding::Base64));
}

#[test]
fn db_postgres_dates_and_times_count_from_2000_not_from_1970() {
    assert_eq!(text("DATE", &0i32.to_be_bytes()), "2000-01-01");
    assert_eq!(text("DATE", &(-1i32).to_be_bytes()), "1999-12-31");
    assert_eq!(text("DATE", &i32::MAX.to_be_bytes()), "infinity");
    assert_eq!(text("DATE", &i32::MIN.to_be_bytes()), "-infinity");
    // PostgreSQL dates reach year 5874897; chrono stops at 262143. Such a date
    // is a value kurama cannot read, and `epoch().date() + Duration` used to
    // panic on it, which is not one `error[CODE]:` line.
    assert_eq!(
        decode("DATE", &2_000_000_000i32.to_be_bytes()),
        Err(DecodeError::Unreadable)
    );
    assert_eq!(
        decode("DATE", &(-2_000_000_000i32).to_be_bytes()),
        Err(DecodeError::Unreadable)
    );
    assert_eq!(text("TIME", &0i64.to_be_bytes()), "00:00:00");
    assert_eq!(
        text("TIME", &(12 * 3600 * 1_000_000i64 + 500_000).to_be_bytes()),
        "12:00:00.5"
    );
    // `time` runs to 24:00:00 inclusive, which PostgreSQL stores and accepts.
    assert_eq!(text("TIME", &86_400_000_000i64.to_be_bytes()), "24:00:00");
    assert_eq!(
        decode("TIME", &86_400_000_001i64.to_be_bytes()),
        Err(DecodeError::Unreadable)
    );
    assert_eq!(
        text("TIMESTAMP", &0i64.to_be_bytes()),
        "2000-01-01 00:00:00"
    );
    assert_eq!(
        text("TIMESTAMPTZ", &1_000_000i64.to_be_bytes()),
        "2000-01-01 00:00:01+00"
    );
    assert_eq!(
        text("TIMESTAMP", &(-1_000_000i64).to_be_bytes()),
        "1999-12-31 23:59:59"
    );
    assert_eq!(text("TIMESTAMP", &i64::MAX.to_be_bytes()), "infinity");
}

#[test]
fn db_postgres_arrays_print_the_way_the_server_prints_them() {
    let cell = |value: i32| Some(value.to_be_bytes().to_vec());
    assert_eq!(
        text("INT4[]", &array_bytes(&[3], &[cell(1), None, cell(3)])),
        "{1,NULL,3}"
    );
    assert_eq!(
        text(
            "INT4[]",
            &array_bytes(&[2, 2], &[cell(1), cell(2), cell(3), cell(4)])
        ),
        "{{1,2},{3,4}}"
    );
    assert_eq!(text("INT4[]", &0i32.to_be_bytes()), "{}");
    // A value that could be read as punctuation, as nothing, or as NULL is
    // quoted, so the literal reads back as the same array.
    let word = |value: &str| Some(value.as_bytes().to_vec());
    assert_eq!(
        text(
            "TEXT[]",
            &array_bytes(&[4], &[word("a,b"), word(""), word("NULL"), word("plain")])
        ),
        r#"{"a,b","","NULL",plain}"#
    );
    // A lower bound that is not 1 is printed as the subscripts the server
    // prints, because `{a,b}` alone reads back as an array starting at 1.
    assert_eq!(
        text("INT4[]", &array_bytes_from(&[2], &[0], &[cell(1), cell(2)])),
        "[0:1]={1,2}"
    );
    assert_eq!(
        text(
            "INT4[]",
            &array_bytes_from(&[2, 2], &[1, 0], &[cell(1), cell(2), cell(3), cell(4)])
        ),
        "[1:2][0:1]={{1,2},{3,4}}"
    );
    // A binary element cannot be told from text inside a literal.
    assert_eq!(
        decode("BYTEA[]", &array_bytes(&[1], &[Some(vec![0, 1])])),
        Err(DecodeError::Unsupported)
    );
}

#[test]
fn db_postgres_says_which_values_it_cannot_read_instead_of_returning_nothing() {
    // A type with no reader is a cast away; the message names it.
    assert_eq!(decode("INTERVAL", &[0; 16]), Err(DecodeError::Unsupported));
    assert_eq!(decode("INET", &[0; 8]), Err(DecodeError::Unsupported));
    // A value of a type kurama reads, in a shape it does not have.
    assert_eq!(decode("INT4", &[0, 1]), Err(DecodeError::Unreadable));
    assert_eq!(decode("UUID", &[0; 4]), Err(DecodeError::Unreadable));
    assert_eq!(decode("TEXT", &[0xff, 0xfe]), Err(DecodeError::Unreadable));
    assert_eq!(decode("NUMERIC", &[0, 1]), Err(DecodeError::Unreadable));
    assert_eq!(
        decode("NUMERIC", &numeric_bytes(0, 0x1000, 0, &[])),
        Err(DecodeError::Unreadable),
        "an unknown sign is not read as positive"
    );
    assert_eq!(
        decode("TIME", &(-1i64).to_be_bytes()),
        Err(DecodeError::Unreadable)
    );
}
