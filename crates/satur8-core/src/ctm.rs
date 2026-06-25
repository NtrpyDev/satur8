//! Conversion of the saturation matrix to the kernel's DRM CTM format.
//!
//! The DRM/KMS `CTM` property is a blob holding `struct drm_color_ctm`: nine
//! `__u64` entries, row-major, each in **S31.32 sign-magnitude** fixed point
//! (NOT two's complement). Bit 63 is the sign; the low 63 bits are the
//! magnitude with 32 fractional bits.
//!
//! This is shared so the DRM backend (B2) and any other CTM-capable backend
//! (Hyprland's CTM protocol, B3) hand the hardware identical numbers, and so
//! the tricky fixed-point packing is unit-tested once.

use crate::Saturation;

/// 32 fractional bits: the multiplier from a real value to S31.32 magnitude.
const FRAC: f64 = (1u64 << 32) as f64;
const SIGN_BIT: u64 = 1u64 << 63;

/// Pack one real coefficient into S31.32 sign-magnitude.
fn pack(value: f64) -> u64 {
    let magnitude = (value.abs() * FRAC).round() as u64;
    // Keep the magnitude within the 63 available bits (saturate rather than wrap;
    // our coefficients are tiny, this only guards against pathological input).
    let magnitude = magnitude & (SIGN_BIT - 1);
    if value < 0.0 {
        magnitude | SIGN_BIT
    } else {
        magnitude
    }
}

/// The nine CTM entries (row-major) for a given saturation, ready to drop into
/// a `drm_color_ctm` blob.
pub fn saturation_to_drm_ctm(saturation: Saturation) -> [u64; 9] {
    let m = saturation.matrix();
    let mut out = [0u64; 9];
    for r in 0..3 {
        for c in 0..3 {
            out[r * 3 + c] = pack(m[r][c] as f64);
        }
    }
    out
}

/// The identity CTM (no color change). Handy for reset paths.
pub fn identity_drm_ctm() -> [u64; 9] {
    saturation_to_drm_ctm(Saturation::IDENTITY)
}

/// Raw bytes of the `drm_color_ctm` blob (little-endian `__u64[9]`), which is
/// exactly what gets attached as the property blob.
pub fn drm_ctm_blob_bytes(saturation: Saturation) -> [u8; 72] {
    let entries = saturation_to_drm_ctm(saturation);
    let mut bytes = [0u8; 72];
    for (i, e) in entries.iter().enumerate() {
        bytes[i * 8..i * 8 + 8].copy_from_slice(&e.to_ne_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unpack(raw: u64) -> f64 {
        let sign = if raw & SIGN_BIT != 0 { -1.0 } else { 1.0 };
        let mag = (raw & (SIGN_BIT - 1)) as f64 / FRAC;
        sign * mag
    }

    fn apply_matrix(matrix: [[f32; 3]; 3], pixel: [f32; 3]) -> [f32; 3] {
        [
            matrix[0][0] * pixel[0] + matrix[0][1] * pixel[1] + matrix[0][2] * pixel[2],
            matrix[1][0] * pixel[0] + matrix[1][1] * pixel[1] + matrix[1][2] * pixel[2],
            matrix[2][0] * pixel[0] + matrix[2][1] * pixel[1] + matrix[2][2] * pixel[2],
        ]
    }

    fn luma(pixel: [f32; 3]) -> f32 {
        crate::LUMA_R * pixel[0] + crate::LUMA_G * pixel[1] + crate::LUMA_B * pixel[2]
    }

    fn distance_from_luma(pixel: [f32; 3]) -> f32 {
        let luma = luma(pixel);
        (pixel[0] - luma).abs() + (pixel[1] - luma).abs() + (pixel[2] - luma).abs()
    }

    #[test]
    fn identity_packs_to_one_and_zero() {
        let ctm = identity_drm_ctm();
        // Diagonal entries are 1.0 -> 2^32; off-diagonal are 0.
        assert_eq!(ctm[0], 1u64 << 32);
        assert_eq!(ctm[4], 1u64 << 32);
        assert_eq!(ctm[8], 1u64 << 32);
        for i in [1, 2, 3, 5, 6, 7] {
            assert_eq!(ctm[i], 0);
        }
    }

    #[test]
    fn greyscale_ctm_rows_sum_to_one() {
        let ctm = saturation_to_drm_ctm(Saturation::new(0.0));
        for row in 0..3 {
            let sum = unpack(ctm[row * 3]) + unpack(ctm[row * 3 + 1]) + unpack(ctm[row * 3 + 2]);
            assert!((sum - 1.0).abs() < 1e-6, "row {row} sums to {sum}");
        }
    }

    #[test]
    fn greyscale_ctm_uses_rec709_luma_weights() {
        let ctm = saturation_to_drm_ctm(Saturation::new(0.0));
        for row in 0..3 {
            assert!((unpack(ctm[row * 3]) - crate::LUMA_R as f64).abs() < 1e-6);
            assert!((unpack(ctm[row * 3 + 1]) - crate::LUMA_G as f64).abs() < 1e-6);
            assert!((unpack(ctm[row * 3 + 2]) - crate::LUMA_B as f64).abs() < 1e-6);
        }
    }

    #[test]
    fn pure_grey_input_is_unchanged_at_any_saturation() {
        let grey = [0.42, 0.42, 0.42];
        for &s in &[0.0f32, 0.5, 1.0, 2.0, 4.0] {
            let out = apply_matrix(Saturation::new(s).matrix(), grey);
            for channel in out {
                assert!((channel - 0.42).abs() < 1e-6, "s={s} channel={channel}");
            }
        }
    }

    #[test]
    fn higher_saturation_pushes_colored_pixel_farther_from_luma() {
        let pixel = [0.9, 0.2, 0.1];
        let muted = apply_matrix(Saturation::new(0.5).matrix(), pixel);
        let identity = apply_matrix(Saturation::new(1.0).matrix(), pixel);
        let vivid = apply_matrix(Saturation::new(2.0).matrix(), pixel);

        assert!(distance_from_luma(muted) < distance_from_luma(identity));
        assert!(distance_from_luma(identity) < distance_from_luma(vivid));
    }

    #[test]
    fn round_trips_within_fixed_point_precision() {
        for &s in &[0.0f32, 0.5, 1.0, 1.6, 2.0, 4.0] {
            let sat = Saturation::new(s);
            let ctm = saturation_to_drm_ctm(sat);
            let m = sat.matrix();
            for r in 0..3 {
                for c in 0..3 {
                    let got = unpack(ctm[r * 3 + c]);
                    let want = m[r][c] as f64;
                    assert!(
                        (got - want).abs() < 1e-6,
                        "s={s} [{r}][{c}] got {got} want {want}"
                    );
                }
            }
        }
    }

    #[test]
    fn negative_coefficients_use_sign_bit() {
        // For s>1, w=1-s<0, so off-diagonal coefficients (w*L) are negative.
        let ctm = saturation_to_drm_ctm(Saturation::new(2.0));
        // entry [0][1] = w*Lg = -1 * 0.7152 < 0 -> sign bit set.
        assert!(
            ctm[1] & SIGN_BIT != 0,
            "expected sign bit for negative coeff"
        );
        assert!(unpack(ctm[1]) < 0.0);
    }

    #[test]
    fn blob_bytes_are_72_and_match_entries() {
        let sat = Saturation::new(1.6);
        let entries = saturation_to_drm_ctm(sat);
        let bytes = drm_ctm_blob_bytes(sat);
        for (i, e) in entries.iter().enumerate() {
            let slice: [u8; 8] = bytes[i * 8..i * 8 + 8].try_into().unwrap();
            assert_eq!(u64::from_ne_bytes(slice), *e);
        }
    }
}
