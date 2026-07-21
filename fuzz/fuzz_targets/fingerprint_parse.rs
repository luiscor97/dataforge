#![no_main]
//! Parsing a stored fingerprint token must never panic: the token can come
//! from a database an attacker may have edited (ADR-0019).
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(token) = std::str::from_utf8(data) {
        let _ = df_domain::FileFingerprint::parse(token);
    }
});
