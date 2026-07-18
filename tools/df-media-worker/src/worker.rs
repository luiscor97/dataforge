use std::io::Cursor;

use df_media::worker_protocol::{
    decode_request, phash_luma32_for_worker, serialize_response, ImageWorkerErrorCode,
    ImageWorkerResponse, IMAGE_WORKER_PROTOCOL_VERSION,
};
use image::imageops::FilterType;
use image::{ImageFormat, ImageReader};

const HARD_MAX_PIXELS: u64 = 100_000_000;

/// Process one complete framed request. All decoder errors are reduced to a
/// bounded, versioned response without reflecting attacker-controlled text.
#[must_use]
pub(crate) fn process_framed_request(framed: &[u8]) -> Vec<u8> {
    let response = match decode_request(framed) {
        Ok((header, input)) if header.max_pixels <= HARD_MAX_PIXELS => {
            decode_image(input, header.max_pixels)
        }
        Ok(_) | Err(ImageWorkerErrorCode::InvalidRequest) => {
            error(ImageWorkerErrorCode::InvalidRequest)
        }
        Err(code) => error(code),
    };
    serialize_response(&response)
}

fn decode_image(input: &[u8], max_pixels: u64) -> ImageWorkerResponse {
    let reader = match ImageReader::new(Cursor::new(input)).with_guessed_format() {
        Ok(reader) => reader,
        Err(_) => return error(ImageWorkerErrorCode::MalformedImage),
    };
    let Some(format) = reader.format() else {
        return error(ImageWorkerErrorCode::UnsupportedFormat);
    };
    let format_name = match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpeg",
        ImageFormat::WebP => "webp",
        _ => return error(ImageWorkerErrorCode::UnsupportedFormat),
    };

    let dimensions_reader = ImageReader::with_format(Cursor::new(input), format);
    let (width, height) = match dimensions_reader.into_dimensions() {
        Ok(dimensions) => dimensions,
        Err(_) => return error(ImageWorkerErrorCode::MalformedImage),
    };
    let Some(pixel_count) = u64::from(width).checked_mul(u64::from(height)) else {
        return error(ImageWorkerErrorCode::PixelLimit);
    };
    if width == 0 || height == 0 || pixel_count > max_pixels {
        return error(ImageWorkerErrorCode::PixelLimit);
    }

    let decoded = match ImageReader::with_format(Cursor::new(input), format).decode() {
        Ok(image) => image,
        Err(_) => return error(ImageWorkerErrorCode::MalformedImage),
    };
    if decoded.width() != width || decoded.height() != height {
        return error(ImageWorkerErrorCode::MalformedImage);
    }
    let normalized = decoded
        .resize_exact(32, 32, FilterType::Triangle)
        .to_luma8();
    let Some(phash64) = phash_luma32_for_worker(normalized.as_raw()) else {
        return error(ImageWorkerErrorCode::Internal);
    };
    ImageWorkerResponse::Ok {
        protocol_version: IMAGE_WORKER_PROTOCOL_VERSION,
        format: format_name.to_string(),
        width,
        height,
        pixel_count,
        phash64,
    }
}

fn error(code: ImageWorkerErrorCode) -> ImageWorkerResponse {
    ImageWorkerResponse::Error {
        protocol_version: IMAGE_WORKER_PROTOCOL_VERSION,
        code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use df_media::worker_protocol::{encode_request, parse_response};

    #[test]
    fn malformed_payload_is_rejected_without_detail_reflection() {
        let request = encode_request(b"not an image", 1_000).unwrap();
        assert!(matches!(
            parse_response(&process_framed_request(&request)).unwrap(),
            ImageWorkerResponse::Error {
                code: ImageWorkerErrorCode::UnsupportedFormat,
                ..
            }
        ));
    }
}
