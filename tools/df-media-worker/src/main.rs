#![forbid(unsafe_code)]

use std::io::{Read, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};

use df_media::worker_protocol::MAX_IMAGE_WORKER_STDIN_BYTES;

mod worker;

use worker::process_framed_request;

fn main() {
    let mut input = Vec::new();
    let read = std::io::stdin()
        .lock()
        .take(MAX_IMAGE_WORKER_STDIN_BYTES.saturating_add(1))
        .read_to_end(&mut input);
    let response = match read {
        Ok(_) => catch_unwind(AssertUnwindSafe(|| process_framed_request(&input)))
            .unwrap_or_else(|_| process_framed_request(&[])),
        Err(_) => process_framed_request(&[]),
    };
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(&response).and_then(|()| stdout.flush());
}
