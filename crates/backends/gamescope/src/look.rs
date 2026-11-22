//! The look table gamescope reads from `GAMESCOPE_COLOR_LOOK_G22`.
//!
//! gamescope applies the look to each app's gamma-encoded sRGB values before
//! its own color pipeline, so Steam's vibrancy, white point and night mode
//! still run on top of it. The table is the same `luma + s * (c - luma)` blend
//! every other backend applies, sampled on gamescope's own 17-point lattice.
//!
//! One file per saturation value, named by that value and written once:
//! gamescope only reads the file when the property changes, so a file is never
//! rewritten in place, and the property value says which saturation is active.

use std::fmt::Write as _;

use satur8_core::{Saturation, LUMA_B, LUMA_G, LUMA_R};

/// Lattice points per axis. gamescope samples the look at 17 points
/// (`color_helpers_impl.h:5`), so a 17-edge table is read at its own nodes.
pub const EDGE: usize = 17;

const FILE_PREFIX: &str = "look-g22-s";
const FILE_SUFFIX: &str = ".cube";

/// Saturation in thousandths, the precision the file name carries.
fn milli(saturation: Saturation) -> u32 {
    (saturation.get() * 1000.0).round() as u32
}

/// The saturation a look file actually holds: rounded to the 0.001 step its
/// name records, so the name and the contents always agree.
pub fn quantize(saturation: Saturation) -> Saturation {
    Saturation::new(milli(saturation) as f32 / 1000.0)
}

/// The `.cube` text for `saturation`: a title, the size line, then 17^3 rows
/// with red varying fastest, as gamescope's loader expects.
pub fn cube_text(saturation: Saturation) -> String {
    let s = f64::from(quantize(saturation).get());
    let (lr, lg, lb) = (f64::from(LUMA_R), f64::from(LUMA_G), f64::from(LUMA_B));
    let last = (EDGE - 1) as f64;
    // About 27 bytes per row.
    let mut text = String::with_capacity(64 + EDGE.pow(3) * 27);
    let _ = writeln!(text, "TITLE \"satur8 look s={s:.3}\"");
    let _ = writeln!(text, "LUT_3D_SIZE {EDGE}");
    for b in 0..EDGE {
        for g in 0..EDGE {
            for r in 0..EDGE {
                let color = [r as f64 / last, g as f64 / last, b as f64 / last];
                let luma = lr * color[0] + lg * color[1] + lb * color[2];
                let [out_r, out_g, out_b] = color.map(|c| (luma + s * (c - luma)).clamp(0.0, 1.0));
                let _ = writeln!(text, "{out_r:.6} {out_g:.6} {out_b:.6}");
            }
        }
    }
    text
}

/// The file name for `saturation`, e.g. `look-g22-s1750.cube` for 1.75.
pub fn file_name(saturation: Saturation) -> String {
    format!("{FILE_PREFIX}{:04}{FILE_SUFFIX}", milli(saturation))
}

/// The saturation encoded in a look path written by Satur8, or `None` when
/// the path (or bare file name) is not one of ours.
pub fn saturation_from_path(path: &str) -> Option<Saturation> {
    let name = path.rsplit('/').next()?;
    let digits = name.strip_prefix(FILE_PREFIX)?.strip_suffix(FILE_SUFFIX)?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let milli = digits.parse::<u32>().ok()?;
    if milli > (Saturation::MAX * 1000.0) as u32 {
        return None;
    }
    Some(Saturation::new(milli as f32 / 1000.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(saturation: f32) -> Vec<String> {
        cube_text(Saturation::new(saturation))
            .lines()
            .skip(2)
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn header_is_a_title_then_the_size_line() {
        let text = cube_text(Saturation::new(1.75));
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some("TITLE \"satur8 look s=1.750\""));
        assert_eq!(lines.next(), Some("LUT_3D_SIZE 17"));
    }

    #[test]
    fn has_exactly_4913_rows_of_three_numbers() {
        let rows = rows(1.5);
        assert_eq!(rows.len(), 4913);
        for row in &rows {
            let values: Vec<f64> = row.split(' ').map(|v| v.parse().unwrap()).collect();
            assert_eq!(values.len(), 3, "{row}");
            assert!(values.iter().all(|v| (0.0..=1.0).contains(v)), "{row}");
        }
    }

    #[test]
    fn red_varies_fastest_then_green_then_blue() {
        let rows = rows(1.0);
        assert_eq!(rows[1], "0.062500 0.000000 0.000000");
        assert_eq!(rows[EDGE], "0.000000 0.062500 0.000000");
        assert_eq!(rows[EDGE * EDGE], "0.000000 0.000000 0.062500");
    }

    #[test]
    fn first_row_is_black_so_gamescope_keeps_its_blending() {
        for saturation in [0.0, 1.0, 2.0, 4.0] {
            assert_eq!(rows(saturation)[0], "0.000000 0.000000 0.000000");
        }
    }

    #[test]
    fn zero_saturation_turns_pure_red_into_its_luma() {
        assert_eq!(rows(0.0)[EDGE - 1], "0.212600 0.212600 0.212600");
    }

    #[test]
    fn double_saturation_clamps_pure_red_into_range() {
        assert_eq!(rows(2.0)[EDGE - 1], "1.000000 0.000000 0.000000");
    }

    #[test]
    fn file_name_round_trips_through_the_path() {
        for saturation in [0.0, 0.5, 1.0, 1.25, 1.75, 2.0, 4.0] {
            let name = file_name(Saturation::new(saturation));
            let path = format!("/home/deck/.local/state/satur8/gamescope/{name}");
            let parsed = saturation_from_path(&path).unwrap();
            assert_eq!(file_name(parsed), name);
            assert!((parsed.get() - saturation).abs() < 1e-6);
        }
        assert_eq!(file_name(Saturation::new(1.75)), "look-g22-s1750.cube");
        assert_eq!(file_name(Saturation::new(0.0)), "look-g22-s0000.cube");
    }

    #[test]
    fn foreign_paths_are_not_ours() {
        for path in [
            "",
            "/usr/share/gamescope/looks/vivid.cube",
            "look-g22-s.cube",
            "look-g22-s12a0.cube",
            "look-g22-s9999.cube",
            "look-g22-s1750.cube.tmp",
        ] {
            assert_eq!(saturation_from_path(path), None, "{path}");
        }
    }
}
