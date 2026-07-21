#![no_main]
//! Decoding a stored raw-path blob must never panic (ADR-0020).
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = df_domain::RawPath::from_blob(data);
});
