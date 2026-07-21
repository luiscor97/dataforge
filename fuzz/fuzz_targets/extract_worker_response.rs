#![no_main]
//! The extract worker protocol parses bytes from an isolated sidecar; a
//! hostile or corrupt frame must fail closed, never panic (ADR-0031).
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = df_extract::worker_protocol::parse_response(data, 1024 * 1024);
});
