//! What an authorization server answers, and the redirect a browser brings
//! back, are read before anything about them is trusted.
#![no_main]

use chrono::{TimeZone, Utc};
use kurama::domain::functions::oauth::{parse_callback_query, parse_token_response};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((status, body)) = data.split_first_chunk::<2>() else {
        return;
    };
    let received_at = Utc.timestamp_opt(1_790_000_000, 0).unwrap();
    let _ = parse_token_response(u16::from_le_bytes(*status), body, received_at, None);
    if let Ok(query) = std::str::from_utf8(body) {
        let _ = parse_callback_query(query, "state");
    }
});
