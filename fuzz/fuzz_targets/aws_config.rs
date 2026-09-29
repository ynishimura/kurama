//! `~/.aws/config` is read at every `status` and every completion; whatever
//! another tool wrote there must not stop kurama.
#![no_main]

use kurama::adapters::profile::parser::parse_aws_config;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(content) = std::str::from_utf8(data) {
        let _ = parse_aws_config(content);
    }
});
