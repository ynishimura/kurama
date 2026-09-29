//! A description fetched from a URL is whatever the server sent: any bytes,
//! read as OpenAPI or as a Discovery document, must end in a spec or an error.
#![no_main]

use kurama::adapters::openapi::parse_spec;
use kurama::domain::types::api_spec::SpecFormat;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&selector, bytes)) = data.split_first() else {
        return;
    };
    let format = if selector % 2 == 0 {
        SpecFormat::OpenApi
    } else {
        SpecFormat::Discovery
    };
    let _ = parse_spec(&format, bytes);
});
