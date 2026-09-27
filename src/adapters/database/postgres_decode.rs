//! PostgreSQL binary values, as the strings the server stored.
//!
//! A prepared statement always answers in the binary format, so every type
//! needs its own reader. Nothing is converted to a JSON number and nothing is
//! rounded: a `numeric` keeps its digits, and its `NaN`, which no decimal
//! crate can represent, stays `NaN`.
use super::decode::{DecodeError, Decoded};
use crate::domain::types::database::DbEncoding;
use base64::Engine as _;
use chrono::{Duration, NaiveDate, NaiveDateTime};

/// PostgreSQL counts days and microseconds from this instant, not from 1970.
fn epoch() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2000, 1, 1)
        .expect("2000-01-01 is a date")
        .and_hms_opt(0, 0, 0)
        .expect("midnight is a time")
}

pub fn decode(type_name: &str, bytes: &[u8]) -> Result<Decoded, DecodeError> {
    if let Some(element) = type_name.strip_suffix("[]") {
        return array(element, bytes);
    }
    let text = match type_name {
        "BOOL" => match one(bytes)? {
            0 => "false".to_owned(),
            _ => "true".to_owned(),
        },
        "INT2" => i16::from_be_bytes(fixed(bytes)?).to_string(),
        "INT4" => i32::from_be_bytes(fixed(bytes)?).to_string(),
        "INT8" => i64::from_be_bytes(fixed(bytes)?).to_string(),
        // A float keeps its own width: widening f32 to f64 invents digits.
        "FLOAT4" => format!("{:?}", f32::from_be_bytes(fixed(bytes)?)),
        "FLOAT8" => format!("{:?}", f64::from_be_bytes(fixed(bytes)?)),
        "NUMERIC" => numeric(bytes)?,
        "TEXT" | "VARCHAR" | "BPCHAR" | "NAME" | "CHAR" | "JSON" => utf8(bytes)?,
        // jsonb carries a format version byte before the document.
        "JSONB" => match bytes.split_first() {
            Some((1, rest)) => utf8(rest)?,
            _ => return Err(DecodeError::Unreadable),
        },
        "UUID" => uuid(bytes)?,
        "DATE" => date(i32::from_be_bytes(fixed(bytes)?))?,
        "TIME" => time(i64::from_be_bytes(fixed(bytes)?))?,
        "TIMESTAMP" => timestamp(i64::from_be_bytes(fixed(bytes)?), "")?,
        "TIMESTAMPTZ" => timestamp(i64::from_be_bytes(fixed(bytes)?), "+00")?,
        "BYTEA" => {
            return Ok((
                base64::engine::general_purpose::STANDARD.encode(bytes),
                DbEncoding::Base64,
            ));
        }
        _ => return Err(DecodeError::Unsupported),
    };
    Ok((text, DbEncoding::Text))
}

fn one(bytes: &[u8]) -> Result<u8, DecodeError> {
    bytes.first().copied().ok_or(DecodeError::Unreadable)
}

fn fixed<const N: usize>(bytes: &[u8]) -> Result<[u8; N], DecodeError> {
    bytes.try_into().map_err(|_| DecodeError::Unreadable)
}

fn utf8(bytes: &[u8]) -> Result<String, DecodeError> {
    String::from_utf8(bytes.to_vec()).map_err(|_| DecodeError::Unreadable)
}

fn uuid(bytes: &[u8]) -> Result<String, DecodeError> {
    let bytes: [u8; 16] = fixed(bytes)?;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

fn date(days: i32) -> Result<String, DecodeError> {
    match days {
        i32::MAX => Ok("infinity".into()),
        i32::MIN => Ok("-infinity".into()),
        // PostgreSQL dates reach year 5874897 and chrono stops at 262143, so
        // the addition is checked here exactly as it is in `timestamp`: `+`
        // panics, and a panic is not one `error[CODE]:` line.
        days => epoch()
            .date()
            .checked_add_signed(Duration::days(days as i64))
            .ok_or(DecodeError::Unreadable)
            .map(|date| date.format("%Y-%m-%d").to_string()),
    }
}

fn time(micros: i64) -> Result<String, DecodeError> {
    // `time` runs from 00:00:00 to 24:00:00, and the upper end is a value
    // PostgreSQL stores; the formatting below already prints it as 24:00:00.
    if !(0..=86_400_000_000).contains(&micros) {
        return Err(DecodeError::Unreadable);
    }
    let seconds = micros / 1_000_000;
    Ok(format!(
        "{:02}:{:02}:{:02}{}",
        seconds / 3600,
        (seconds / 60) % 60,
        seconds % 60,
        fraction(micros % 1_000_000)
    ))
}

fn timestamp(micros: i64, zone: &str) -> Result<String, DecodeError> {
    match micros {
        i64::MAX => return Ok("infinity".into()),
        i64::MIN => return Ok("-infinity".into()),
        _ => {}
    }
    let moment = epoch()
        .checked_add_signed(Duration::microseconds(micros))
        .ok_or(DecodeError::Unreadable)?;
    Ok(format!(
        "{}{}{zone}",
        moment.format("%Y-%m-%d %H:%M:%S"),
        fraction(micros.rem_euclid(1_000_000))
    ))
}

/// PostgreSQL prints no decimal point when there is nothing after it, and
/// drops the trailing zeros when there is.
fn fraction(micros: i64) -> String {
    if micros == 0 {
        return String::new();
    }
    format!(".{:06}", micros).trim_end_matches('0').to_owned()
}

/// `numeric` is a sign, a scale and base-10000 digits, so its value is exact
/// and its special values are not numbers at all.
fn numeric(bytes: &[u8]) -> Result<String, DecodeError> {
    let read = |offset: usize| -> Result<i16, DecodeError> {
        bytes
            .get(offset..offset + 2)
            .and_then(|slice| slice.try_into().ok())
            .map(i16::from_be_bytes)
            .ok_or(DecodeError::Unreadable)
    };
    let count = read(0)?;
    let weight = read(2)? as i32;
    let sign = read(4)? as u16;
    let scale = read(6)? as usize;
    match sign {
        0xC000 => return Ok("NaN".into()),
        0xD000 => return Ok("Infinity".into()),
        0xF000 => return Ok("-Infinity".into()),
        0x0000 | 0x4000 => {}
        _ => return Err(DecodeError::Unreadable),
    }
    let count = usize::try_from(count).map_err(|_| DecodeError::Unreadable)?;
    let digits: Vec<i16> = (0..count)
        .map(|index| read(8 + index * 2))
        .collect::<Result<_, _>>()?;
    let digit = |index: i32| -> i16 {
        usize::try_from(index)
            .ok()
            .and_then(|index| digits.get(index).copied())
            .unwrap_or(0)
    };
    let mut text = String::new();
    if sign == 0x4000 {
        text.push('-');
    }
    if weight < 0 {
        text.push('0');
    } else {
        for index in 0..=weight {
            if index == 0 {
                text.push_str(&digit(index).to_string());
            } else {
                text.push_str(&format!("{:04}", digit(index)));
            }
        }
    }
    if scale > 0 {
        text.push('.');
        let mut written = 0;
        let mut index = weight + 1;
        while written < scale {
            let group = format!("{:04}", digit(index));
            let take = (scale - written).min(4);
            text.push_str(&group[..take]);
            written += take;
            index += 1;
        }
    }
    Ok(text)
}

/// An array is its dimensions and its elements; it is printed the way the
/// server prints it, so it can be read back as the same array.
fn array(element_type: &str, bytes: &[u8]) -> Result<Decoded, DecodeError> {
    let read_i32 = |offset: usize| -> Result<i32, DecodeError> {
        bytes
            .get(offset..offset + 4)
            .and_then(|slice| slice.try_into().ok())
            .map(i32::from_be_bytes)
            .ok_or(DecodeError::Unreadable)
    };
    let dimensions = read_i32(0)?;
    if dimensions == 0 {
        return Ok(("{}".to_owned(), DbEncoding::Text));
    }
    let dimensions = usize::try_from(dimensions).map_err(|_| DecodeError::Unreadable)?;
    let mut lengths = Vec::with_capacity(dimensions);
    let mut lower_bounds = Vec::with_capacity(dimensions);
    for index in 0..dimensions {
        let length = read_i32(12 + index * 8)?;
        lengths.push(usize::try_from(length).map_err(|_| DecodeError::Unreadable)?);
        lower_bounds.push(read_i32(16 + index * 8)?);
    }
    let mut offset = 12 + dimensions * 8;
    let total: usize = lengths.iter().product();
    let mut cells = Vec::with_capacity(total);
    for _ in 0..total {
        let length = read_i32(offset)?;
        offset += 4;
        if length < 0 {
            cells.push("NULL".to_owned());
            continue;
        }
        let length = usize::try_from(length).map_err(|_| DecodeError::Unreadable)?;
        let slice = bytes
            .get(offset..offset + length)
            .ok_or(DecodeError::Unreadable)?;
        offset += length;
        let (text, encoding) = decode(element_type, slice)?;
        if encoding == DbEncoding::Base64 {
            // A binary element inside a literal cannot be told from text.
            return Err(DecodeError::Unsupported);
        }
        cells.push(quote_element(&text));
    }
    // PostgreSQL prints the subscripts when a lower bound is not 1, and reads
    // them back; without them `[0:1]={a,b}` returns as a different array.
    let subscripts = if lower_bounds.iter().all(|lower| *lower == 1) {
        String::new()
    } else {
        let ranges: Vec<String> = lower_bounds
            .iter()
            .zip(&lengths)
            .map(|(lower, length)| format!("[{lower}:{}]", i64::from(*lower) + *length as i64 - 1))
            .collect();
        format!("{}=", ranges.concat())
    };
    Ok((
        format!("{subscripts}{}", nest(&cells, &lengths)),
        DbEncoding::Text,
    ))
}

/// The literal form: `{a,b}`, with quotes where a value could be read as
/// punctuation, as nothing, or as the word NULL.
fn quote_element(text: &str) -> String {
    let needs_quotes = text.is_empty()
        || text.eq_ignore_ascii_case("null")
        || text
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '{' | '}' | ',' | '"' | '\\'));
    if !needs_quotes {
        return text.to_owned();
    }
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn nest(cells: &[String], lengths: &[usize]) -> String {
    let Some((length, rest)) = lengths.split_first() else {
        return cells.first().cloned().unwrap_or_default();
    };
    if rest.is_empty() {
        return format!("{{{}}}", cells[..*length.min(&cells.len())].join(","));
    }
    let stride = rest.iter().product::<usize>().max(1);
    let inner: Vec<String> = (0..*length)
        .filter_map(|index| {
            cells
                .get(index * stride..(index + 1) * stride)
                .map(|chunk| nest(chunk, rest))
        })
        .collect();
    format!("{{{}}}", inner.join(","))
}

#[cfg(test)]
#[path = "postgres_decode_tests.rs"]
mod tests;
