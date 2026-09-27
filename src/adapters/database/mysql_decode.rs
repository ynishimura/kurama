//! MySQL binary values, as the strings the server stored.
//!
//! A prepared statement answers in the binary protocol: integers are
//! little-endian, temporal values are packed and length-prefixed, and DECIMAL
//! and JSON arrive as the text MySQL itself would print. Nothing is turned
//! into a JSON number, so a `BIGINT UNSIGNED` and a `DECIMAL(40,9)` keep every
//! digit.
use super::decode::{DecodeError, Decoded};
use crate::domain::types::database::DbEncoding;
use base64::Engine as _;

pub fn decode(type_name: &str, bytes: &[u8]) -> Result<Decoded, DecodeError> {
    let text = match type_name {
        // `BOOLEAN` is what sqlx calls a TINYINT(1); MySQL stores and prints 0
        // and 1, so that is what comes back.
        "BOOLEAN" | "TINYINT" => signed(bytes, 1)?,
        "SMALLINT" => signed(bytes, 2)?,
        "MEDIUMINT" | "INT" => signed(bytes, 4)?,
        "BIGINT" => signed(bytes, 8)?,
        "TINYINT UNSIGNED" => unsigned(bytes, 1)?,
        "SMALLINT UNSIGNED" | "YEAR" => unsigned(bytes, 2)?,
        "MEDIUMINT UNSIGNED" | "INT UNSIGNED" => unsigned(bytes, 4)?,
        "BIGINT UNSIGNED" => unsigned(bytes, 8)?,
        // A float keeps its own width: widening f32 to f64 invents digits.
        "FLOAT" => format!("{:?}", f32::from_le_bytes(fixed(bytes)?)),
        "DOUBLE" => format!("{:?}", f64::from_le_bytes(fixed(bytes)?)),
        // MySQL sends DECIMAL as the digits it stored, not as a float.
        "DECIMAL" | "VARCHAR" | "CHAR" | "TEXT" | "JSON" | "ENUM" | "SET" => utf8(bytes)?,
        "DATE" | "DATETIME" | "TIMESTAMP" => date_time(bytes)?,
        "TIME" => time(bytes)?,
        // BIT is a big-endian bit string; its value is the number it holds.
        "BIT" => bit(bytes),
        "BLOB" | "BINARY" | "VARBINARY" => {
            return Ok((
                base64::engine::general_purpose::STANDARD.encode(bytes),
                DbEncoding::Base64,
            ));
        }
        _ => return Err(DecodeError::Unsupported),
    };
    Ok((text, DbEncoding::Text))
}

fn fixed<const N: usize>(bytes: &[u8]) -> Result<[u8; N], DecodeError> {
    bytes.try_into().map_err(|_| DecodeError::Unreadable)
}

fn utf8(bytes: &[u8]) -> Result<String, DecodeError> {
    String::from_utf8(bytes.to_vec()).map_err(|_| DecodeError::Unreadable)
}

/// A signed integer of `width` bytes, sign-extended to 64 bits.
fn signed(bytes: &[u8], width: usize) -> Result<String, DecodeError> {
    let value = little_endian(bytes, width)?;
    let shift = 64 - width * 8;
    Ok((((value << shift) as i64) >> shift).to_string())
}

fn unsigned(bytes: &[u8], width: usize) -> Result<String, DecodeError> {
    little_endian(bytes, width).map(|value| value.to_string())
}

fn little_endian(bytes: &[u8], width: usize) -> Result<u64, DecodeError> {
    if bytes.len() != width {
        return Err(DecodeError::Unreadable);
    }
    Ok(bytes.iter().enumerate().fold(0u64, |value, (index, byte)| {
        value | ((*byte as u64) << (index * 8))
    }))
}

fn bit(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(0u128, |value, byte| (value << 8) | *byte as u128)
        .to_string()
}

/// The packed payload of a temporal value: a length byte and that many bytes,
/// or the payload on its own when the driver already removed the prefix.
fn payload<'a>(bytes: &'a [u8], valid: &[usize]) -> Option<&'a [u8]> {
    if let Some((length, rest)) = bytes.split_first()
        && *length as usize == rest.len()
        && valid.contains(&rest.len())
    {
        return Some(rest);
    }
    valid.contains(&bytes.len()).then_some(bytes)
}

/// `DATE`, `DATETIME` and `TIMESTAMP` share one packed layout; how much of it
/// is present says how much of the value was stored.
fn date_time(bytes: &[u8]) -> Result<String, DecodeError> {
    let packed = payload(bytes, &[0, 4, 7, 11]).ok_or(DecodeError::Unreadable)?;
    if packed.is_empty() {
        // MySQL's zero date is a value, not a missing one.
        return Ok("0000-00-00 00:00:00".into());
    }
    let year = u16::from_le_bytes([packed[0], packed[1]]);
    let date = format!("{year:04}-{:02}-{:02}", packed[2], packed[3]);
    if packed.len() == 4 {
        return Ok(date);
    }
    let micros = if packed.len() == 11 {
        u32::from_le_bytes([packed[7], packed[8], packed[9], packed[10]])
    } else {
        0
    };
    Ok(format!(
        "{date} {:02}:{:02}:{:02}{}",
        packed[4],
        packed[5],
        packed[6],
        fraction(micros)
    ))
}

/// `TIME` is a signed span, so it counts days and may pass 24 hours.
fn time(bytes: &[u8]) -> Result<String, DecodeError> {
    let packed = payload(bytes, &[0, 8, 12]).ok_or(DecodeError::Unreadable)?;
    if packed.is_empty() {
        return Ok("00:00:00".into());
    }
    let sign = if packed[0] == 1 { "-" } else { "" };
    let days = u32::from_le_bytes([packed[1], packed[2], packed[3], packed[4]]);
    let hours = days * 24 + packed[5] as u32;
    let micros = if packed.len() == 12 {
        u32::from_le_bytes([packed[8], packed[9], packed[10], packed[11]])
    } else {
        0
    };
    Ok(format!(
        "{sign}{hours:02}:{:02}:{:02}{}",
        packed[6],
        packed[7],
        fraction(micros)
    ))
}

/// MySQL prints no decimal point when there is nothing after it.
fn fraction(micros: u32) -> String {
    if micros == 0 {
        return String::new();
    }
    format!(".{micros:06}").trim_end_matches('0').to_owned()
}

#[cfg(test)]
#[path = "mysql_decode_tests.rs"]
mod tests;
