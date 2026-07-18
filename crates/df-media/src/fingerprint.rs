use std::sync::OnceLock;

const SIDE: usize = 32;
const LOW: usize = 8;
const FIXED_SCALE: f64 = 16_384.0;

/// Deterministic 64-bit perceptual hash over a normalized 32x32 luma plane.
/// The DCT basis is quantized before accumulation so coefficient ordering is
/// integer-only after one process-wide initialization.
pub(crate) fn phash_luma32(luma: &[u8]) -> Option<String> {
    if luma.len() != SIDE * SIDE {
        return None;
    }
    let basis = dct_basis();
    let mut coefficients = [0_i128; LOW * LOW];
    for u in 0..LOW {
        for v in 0..LOW {
            let mut sum = 0_i128;
            for y in 0..SIDE {
                let by = i128::from(basis[v][y]);
                for x in 0..SIDE {
                    let centered = i128::from(luma[y * SIDE + x]) - 128;
                    sum += centered * i128::from(basis[u][x]) * by;
                }
            }
            coefficients[v * LOW + u] = sum;
        }
    }

    // Exclude the DC coefficient when selecting the threshold. The DC bit is
    // still populated, yielding a conventional fixed-width 64-bit value.
    let mut threshold_values = coefficients[1..].to_vec();
    threshold_values.sort_unstable();
    let median = threshold_values[threshold_values.len() / 2];
    let mut hash = 0_u64;
    for (index, coefficient) in coefficients.into_iter().enumerate() {
        if coefficient > median {
            hash |= 1_u64 << index;
        }
    }
    Some(format!("{hash:016x}"))
}

pub(crate) fn parse_phash(value: &str) -> Option<u64> {
    if value.len() != 16 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u64::from_str_radix(value, 16).ok()
}

fn dct_basis() -> &'static [[i32; SIDE]; LOW] {
    static BASIS: OnceLock<[[i32; SIDE]; LOW]> = OnceLock::new();
    BASIS.get_or_init(|| {
        let mut basis = [[0_i32; SIDE]; LOW];
        for (frequency, row) in basis.iter_mut().enumerate() {
            for (position, value) in row.iter_mut().enumerate() {
                let angle = std::f64::consts::PI * ((2 * position + 1) * frequency) as f64
                    / (2 * SIDE) as f64;
                *value = (angle.cos() * FIXED_SCALE).round() as i32;
            }
        }
        basis
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phash_is_stable_and_fixed_width() {
        let mut gradient = vec![0_u8; SIDE * SIDE];
        for (index, pixel) in gradient.iter_mut().enumerate() {
            *pixel = (index % SIDE * 8) as u8;
        }
        assert_eq!(
            phash_luma32(&gradient),
            phash_luma32(&gradient),
            "same normalized pixels must hash identically"
        );
        assert_eq!(phash_luma32(&gradient).unwrap().len(), 16);
        assert!(phash_luma32(&gradient[..100]).is_none());
    }
}
