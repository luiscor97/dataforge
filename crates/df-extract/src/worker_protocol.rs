//! Versioned, bounded wire protocol shared with `df-extract-worker`.
//!
//! This module is public only so the separately linked worker executable can
//! share the exact framing contract. It is not a general application API.

use std::io::{Read, Write};

pub const REQUEST_MAGIC: [u8; 8] = *b"DFPDFQ01";
pub const RESPONSE_MAGIC: [u8; 8] = *b"DFPDFR01";
pub const PROTOCOL_VERSION: u16 = 1;
pub const REQUEST_HEADER_BYTES: usize = 26;
pub const RESPONSE_HEADER_BYTES: usize = 19;
pub const HARD_MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024;
pub const HARD_MAX_OUTPUT_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_ERROR_BYTES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WorkerStatus {
    Ok = 0,
    Rejected = 1,
    OutputLimit = 2,
    Internal = 3,
}

impl WorkerStatus {
    fn from_byte(value: u8) -> Result<Self, String> {
        match value {
            0 => Ok(Self::Ok),
            1 => Ok(Self::Rejected),
            2 => Ok(Self::OutputLimit),
            3 => Ok(Self::Internal),
            _ => Err(format!("unknown PDF worker status {value}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestHeader {
    pub input_bytes: u64,
    pub max_output_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: WorkerStatus,
    pub payload: Vec<u8>,
}

pub fn write_request(
    mut writer: impl Write,
    input: &[u8],
    max_output_bytes: u64,
) -> Result<(), String> {
    let input_bytes = u64::try_from(input.len())
        .map_err(|_| "PDF worker input length does not fit u64".to_string())?;
    validate_request_lengths(input_bytes, max_output_bytes)?;
    writer
        .write_all(&REQUEST_MAGIC)
        .and_then(|()| writer.write_all(&PROTOCOL_VERSION.to_le_bytes()))
        .and_then(|()| writer.write_all(&input_bytes.to_le_bytes()))
        .and_then(|()| writer.write_all(&max_output_bytes.to_le_bytes()))
        .and_then(|()| writer.write_all(input))
        .map_err(|error| format!("cannot write PDF worker request: {error}"))
}

pub fn read_request_header(mut reader: impl Read) -> Result<RequestHeader, String> {
    let mut header = [0_u8; REQUEST_HEADER_BYTES];
    reader
        .read_exact(&mut header)
        .map_err(|error| format!("cannot read PDF worker request header: {error}"))?;
    if header[..8] != REQUEST_MAGIC {
        return Err("PDF worker request magic does not match".to_string());
    }
    let version = u16::from_le_bytes([header[8], header[9]]);
    if version != PROTOCOL_VERSION {
        return Err(format!("unsupported PDF worker protocol version {version}"));
    }
    let input_bytes = u64::from_le_bytes(
        header[10..18]
            .try_into()
            .expect("request input length has fixed width"),
    );
    let max_output_bytes = u64::from_le_bytes(
        header[18..26]
            .try_into()
            .expect("request output limit has fixed width"),
    );
    validate_request_lengths(input_bytes, max_output_bytes)?;
    Ok(RequestHeader {
        input_bytes,
        max_output_bytes,
    })
}

pub fn read_request_payload(mut reader: impl Read, input_bytes: u64) -> Result<Vec<u8>, String> {
    if input_bytes > HARD_MAX_INPUT_BYTES {
        return Err("PDF worker input exceeds its hard byte ceiling".to_string());
    }
    let length = usize::try_from(input_bytes)
        .map_err(|_| "PDF worker input does not fit this platform".to_string())?;
    let mut input = vec![0_u8; length];
    reader
        .read_exact(&mut input)
        .map_err(|error| format!("cannot read PDF worker request body: {error}"))?;
    Ok(input)
}

pub fn write_response(
    mut writer: impl Write,
    status: WorkerStatus,
    payload: &[u8],
) -> Result<(), String> {
    let payload_bytes = u64::try_from(payload.len())
        .map_err(|_| "PDF worker response length does not fit u64".to_string())?;
    let allowed = match status {
        WorkerStatus::Ok => HARD_MAX_OUTPUT_BYTES,
        WorkerStatus::Rejected | WorkerStatus::Internal => MAX_ERROR_BYTES as u64,
        WorkerStatus::OutputLimit => 0,
    };
    if payload_bytes > allowed {
        return Err(format!(
            "PDF worker response exceeds the status payload ceiling of {allowed} bytes"
        ));
    }
    writer
        .write_all(&RESPONSE_MAGIC)
        .and_then(|()| writer.write_all(&PROTOCOL_VERSION.to_le_bytes()))
        .and_then(|()| writer.write_all(&[status as u8]))
        .and_then(|()| writer.write_all(&payload_bytes.to_le_bytes()))
        .and_then(|()| writer.write_all(payload))
        .map_err(|error| format!("cannot write PDF worker response: {error}"))
}

pub fn parse_response(bytes: &[u8], requested_output_limit: u64) -> Result<Response, String> {
    if bytes.len() < RESPONSE_HEADER_BYTES {
        return Err("PDF worker response is shorter than its header".to_string());
    }
    if bytes[..8] != RESPONSE_MAGIC {
        return Err("PDF worker response magic does not match".to_string());
    }
    let version = u16::from_le_bytes([bytes[8], bytes[9]]);
    if version != PROTOCOL_VERSION {
        return Err(format!(
            "unsupported PDF worker response protocol version {version}"
        ));
    }
    let status = WorkerStatus::from_byte(bytes[10])?;
    let payload_bytes = u64::from_le_bytes(
        bytes[11..19]
            .try_into()
            .expect("response payload length has fixed width"),
    );
    let allowed = match status {
        WorkerStatus::Ok => requested_output_limit.min(HARD_MAX_OUTPUT_BYTES),
        WorkerStatus::Rejected | WorkerStatus::Internal => MAX_ERROR_BYTES as u64,
        WorkerStatus::OutputLimit => 0,
    };
    if payload_bytes > allowed {
        return Err(format!(
            "PDF worker response declares {payload_bytes} bytes above its {allowed}-byte limit"
        ));
    }
    let payload_len = usize::try_from(payload_bytes)
        .map_err(|_| "PDF worker response does not fit this platform".to_string())?;
    let expected = RESPONSE_HEADER_BYTES
        .checked_add(payload_len)
        .ok_or_else(|| "PDF worker response length overflow".to_string())?;
    if bytes.len() != expected {
        return Err(format!(
            "PDF worker response length mismatch: declared {expected}, received {}",
            bytes.len()
        ));
    }
    Ok(Response {
        status,
        payload: bytes[RESPONSE_HEADER_BYTES..].to_vec(),
    })
}

fn validate_request_lengths(input_bytes: u64, max_output_bytes: u64) -> Result<(), String> {
    if input_bytes == 0 {
        return Err("PDF worker input cannot be empty".to_string());
    }
    if input_bytes > HARD_MAX_INPUT_BYTES {
        return Err(format!(
            "PDF worker input exceeds its {}-byte hard ceiling",
            HARD_MAX_INPUT_BYTES
        ));
    }
    if max_output_bytes == 0 || max_output_bytes > HARD_MAX_OUTPUT_BYTES {
        return Err(format!(
            "PDF worker output limit must be between 1 and {} bytes",
            HARD_MAX_OUTPUT_BYTES
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_frame_round_trips_with_bounded_lengths() {
        let mut frame = Vec::new();
        write_request(&mut frame, b"%PDF-x", 1024).unwrap();
        let header = read_request_header(&frame[..REQUEST_HEADER_BYTES]).unwrap();
        assert_eq!(header.input_bytes, 6);
        assert_eq!(header.max_output_bytes, 1024);
        let body =
            read_request_payload(&frame[REQUEST_HEADER_BYTES..], header.input_bytes).unwrap();
        assert_eq!(body, b"%PDF-x");
    }

    #[test]
    fn response_rejects_trailing_or_over_limit_data() {
        let mut frame = Vec::new();
        write_response(&mut frame, WorkerStatus::Ok, b"text").unwrap();
        assert_eq!(parse_response(&frame, 4).unwrap().payload, b"text");
        frame.push(0);
        assert!(parse_response(&frame, 4).is_err());

        let mut oversized_declaration = Vec::new();
        write_response(&mut oversized_declaration, WorkerStatus::Ok, b"text").unwrap();
        assert!(parse_response(&oversized_declaration, 3).is_err());
    }

    #[test]
    fn request_limits_are_enforced_before_payload_allocation() {
        let mut header = Vec::new();
        header.extend_from_slice(&REQUEST_MAGIC);
        header.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
        header.extend_from_slice(&(HARD_MAX_INPUT_BYTES + 1).to_le_bytes());
        header.extend_from_slice(&1_u64.to_le_bytes());
        assert!(read_request_header(&header[..]).is_err());
    }
}
