//! Resource-isolated PDF parser for DataForge.
//!
//! This executable is intentionally single-shot: it accepts one bounded frame
//! on stdin, emits one bounded frame on stdout, and exits. The parent must
//! place it in an OS sandbox before it sends untrusted bytes.

#![forbid(unsafe_code)]

use std::io::{self, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::process::ExitCode;

use df_extract::worker_protocol::{
    read_request_header, read_request_payload, write_response, WorkerStatus, MAX_ERROR_BYTES,
};

fn main() -> ExitCode {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let header = match read_request_header(&mut input) {
        Ok(header) => header,
        Err(error) => return emit_internal(&error),
    };
    let bytes = match read_request_payload(&mut input, header.input_bytes) {
        Ok(bytes) => bytes,
        Err(error) => return emit_internal(&error),
    };

    let result = if !bytes.starts_with(b"%PDF-") {
        WorkerResult::Rejected("PDF header signature is missing".to_string())
    } else {
        match catch_unwind(AssertUnwindSafe(|| {
            pdf_extract::extract_text_from_mem(&bytes)
        })) {
            Ok(Ok(text)) => {
                if u64::try_from(text.len()).unwrap_or(u64::MAX) > header.max_output_bytes {
                    WorkerResult::OutputLimit
                } else {
                    WorkerResult::Text(text)
                }
            }
            Ok(Err(error)) => {
                WorkerResult::Rejected(format!("PDF backend rejected input: {error}"))
            }
            Err(_) => {
                WorkerResult::Internal("PDF backend panicked while parsing input".to_string())
            }
        }
    };

    let stdout = io::stdout();
    let mut output = stdout.lock();
    let write = match result {
        WorkerResult::Text(text) => write_response(&mut output, WorkerStatus::Ok, text.as_bytes()),
        WorkerResult::Rejected(error) => write_response(
            &mut output,
            WorkerStatus::Rejected,
            bounded_error(&error).as_bytes(),
        ),
        WorkerResult::OutputLimit => write_response(&mut output, WorkerStatus::OutputLimit, &[]),
        WorkerResult::Internal(error) => write_response(
            &mut output,
            WorkerStatus::Internal,
            bounded_error(&error).as_bytes(),
        ),
    };
    match write.and_then(|()| output.flush().map_err(|error| error.to_string())) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

fn emit_internal(error: &str) -> ExitCode {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    let message = bounded_error(error);
    match write_response(&mut output, WorkerStatus::Internal, message.as_bytes())
        .and_then(|()| output.flush().map_err(|error| error.to_string()))
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

fn bounded_error(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars().filter(|character| !character.is_control()) {
        if output.len().saturating_add(character.len_utf8()) > MAX_ERROR_BYTES {
            break;
        }
        output.push(character);
    }
    output
}

enum WorkerResult {
    Text(String),
    Rejected(String),
    OutputLimit,
    Internal(String),
}
