#![no_main]
//! Parsing an untrusted relative path must never panic; it either yields a
//! safe path or a typed error (ADR-0017).
use libfuzzer_sys::fuzz_target;
use std::path::Path;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = df_fs_safety::SafeRelativePath::parse(Path::new(text));
    }
});
