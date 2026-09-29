//! Every configured secret goes through `SecretRef::parse` at load, and its
//! error names the value's line, so it must answer for any string.
#![no_main]

use kurama::domain::types::SecretRef;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(value) = std::str::from_utf8(data) {
        let _ = SecretRef::parse(value);
    }
});
