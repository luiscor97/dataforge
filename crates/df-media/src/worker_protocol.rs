//! Versioned binary request and JSON response protocol for the image worker.

use serde::{Deserialize, Serialize};

pub const IMAGE_WORKER_PROTOCOL_VERSION: u16 = 1;
pub const MAX_IMAGE_WORKER_STDIN_BYTES: u64 = 256 * 1024 * 1024 + 4 * 1024;
pub const MAX_IMAGE_WORKER_STDOUT_BYTES: u64 = 64 * 1024;

const MAGIC: &[u8; 8] = b"DFMEDIA1";
const PREFIX_BYTES: usize = MAGIC.len() + 4;
const MAX_HEADER_BYTES: usize = 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[doc(hidden)]
pub struct ImageWorkerRequestHeader {
    pub protocol_version: u16,
    pub max_pixels: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "SCREAMING_SNAKE_CASE",
    deny_unknown_fields
)]
#[doc(hidden)]
pub enum ImageWorkerResponse {
    Ok {
        protocol_version: u16,
        format: String,
        width: u32,
        height: u32,
        pixel_count: u64,
        phash64: String,
    },
    Error {
        protocol_version: u16,
        code: ImageWorkerErrorCode,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[doc(hidden)]
pub enum ImageWorkerErrorCode {
    InvalidRequest,
    UnsupportedFormat,
    MalformedImage,
    PixelLimit,
    Internal,
}

#[doc(hidden)]
pub fn encode_request(input: &[u8], max_pixels: u64) -> Result<Vec<u8>, &'static str> {
    let header = serde_json::to_vec(&ImageWorkerRequestHeader {
        protocol_version: IMAGE_WORKER_PROTOCOL_VERSION,
        max_pixels,
    })
    .map_err(|_| "cannot serialize image worker header")?;
    if header.len() > MAX_HEADER_BYTES {
        return Err("image worker header is too large");
    }
    let total = PREFIX_BYTES
        .checked_add(header.len())
        .and_then(|value| value.checked_add(input.len()))
        .ok_or("image worker request length overflow")?;
    if u64::try_from(total).unwrap_or(u64::MAX) > MAX_IMAGE_WORKER_STDIN_BYTES {
        return Err("image worker request exceeds its absolute byte ceiling");
    }
    let header_len = u32::try_from(header.len()).map_err(|_| "image worker header is too large")?;
    let mut framed = Vec::with_capacity(total);
    framed.extend_from_slice(MAGIC);
    framed.extend_from_slice(&header_len.to_le_bytes());
    framed.extend_from_slice(&header);
    framed.extend_from_slice(input);
    Ok(framed)
}

#[doc(hidden)]
pub fn decode_request(
    framed: &[u8],
) -> Result<(ImageWorkerRequestHeader, &[u8]), ImageWorkerErrorCode> {
    if u64::try_from(framed.len()).unwrap_or(u64::MAX) > MAX_IMAGE_WORKER_STDIN_BYTES
        || framed.len() < PREFIX_BYTES
        || &framed[..MAGIC.len()] != MAGIC
    {
        return Err(ImageWorkerErrorCode::InvalidRequest);
    }
    let header_len = u32::from_le_bytes(
        framed[MAGIC.len()..PREFIX_BYTES]
            .try_into()
            .map_err(|_| ImageWorkerErrorCode::InvalidRequest)?,
    ) as usize;
    if header_len == 0 || header_len > MAX_HEADER_BYTES {
        return Err(ImageWorkerErrorCode::InvalidRequest);
    }
    let payload_start = PREFIX_BYTES
        .checked_add(header_len)
        .ok_or(ImageWorkerErrorCode::InvalidRequest)?;
    if payload_start >= framed.len() {
        return Err(ImageWorkerErrorCode::InvalidRequest);
    }
    let header: ImageWorkerRequestHeader =
        serde_json::from_slice(&framed[PREFIX_BYTES..payload_start])
            .map_err(|_| ImageWorkerErrorCode::InvalidRequest)?;
    if header.protocol_version != IMAGE_WORKER_PROTOCOL_VERSION || header.max_pixels == 0 {
        return Err(ImageWorkerErrorCode::InvalidRequest);
    }
    Ok((header, &framed[payload_start..]))
}

#[doc(hidden)]
pub fn serialize_response(response: &ImageWorkerResponse) -> Vec<u8> {
    serde_json::to_vec(response).unwrap_or_else(|_| {
        br#"{"status":"ERROR","protocol_version":1,"code":"INTERNAL"}"#.to_vec()
    })
}

#[doc(hidden)]
pub fn parse_response(bytes: &[u8]) -> Result<ImageWorkerResponse, &'static str> {
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_IMAGE_WORKER_STDOUT_BYTES {
        return Err("image worker response exceeds its byte ceiling");
    }
    serde_json::from_slice(bytes).map_err(|_| "image worker response is invalid JSON")
}

/// Worker-only entry point for hashing an already-decoded, fixed-size luma
/// plane. It performs no parsing or decoding.
#[doc(hidden)]
pub fn phash_luma32_for_worker(luma: &[u8]) -> Option<String> {
    crate::fingerprint::phash_luma32(luma)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_rejects_truncation_and_version_forgery() {
        let mut frame = encode_request(b"image", 100).unwrap();
        assert_eq!(decode_request(&frame).unwrap().1, b"image");
        assert!(decode_request(&frame[..10]).is_err());

        let header_start = PREFIX_BYTES;
        let version = frame[header_start..]
            .windows("\"protocol_version\":1".len())
            .position(|window| window == b"\"protocol_version\":1")
            .unwrap();
        let digit = header_start + version + "\"protocol_version\":".len();
        frame[digit] = b'9';
        assert!(decode_request(&frame).is_err());
    }
}
