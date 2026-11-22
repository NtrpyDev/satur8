//! Where the app runs and how its window fits there. Everything here is pure;
//! `main.rs` reads the environment and the screen and acts on what this
//! returns.
//!
//! A Steam gaming session (gamescope) shows one app at a time, scaled to fill
//! the screen, and delivers touches as mouse clicks at screen positions. So
//! the app's window has to be exactly the size of the screen there. Left to
//! itself the window opens at its preferred size times the scale factor the
//! display reports, which on a Steam Deck (1280x800, about 2.08) is 2375x1458:
//! gamescope shrinks the picture to fit, but touches are not shrunk with it,
//! so only the top left of the window can be reached.

/// The width the Game Mode layout (`ui/touch.slint`) is drawn at, in logical
/// pixels. On a Steam Deck's 1280x800 screen that is a scale of 2: about 204
/// pixels per inch, so one logical pixel is a quarter of a millimeter and a
/// 44 pixel control is 11 mm.
pub const LAYOUT_WIDTH: f32 = 640.0;

/// The least height that layout needs, in logical pixels: its `min-height`.
pub const LAYOUT_MIN_HEIGHT: f32 = 360.0;

/// Whether this process runs in a gamescope session: Steam Game Mode, or a
/// program started inside a nested gamescope. `var` reads an environment
/// variable.
pub fn in_gamescope_session(var: impl Fn(&str) -> Option<String>) -> bool {
    let set = |name: &str| var(name).is_some_and(|value| !value.is_empty());
    let desktop_is_gamescope = var("XDG_CURRENT_DESKTOP").is_some_and(|desktop| {
        desktop
            .split(':')
            .any(|part| part.eq_ignore_ascii_case("gamescope"))
    });
    desktop_is_gamescope || set("GAMESCOPE_WAYLAND_DISPLAY")
}

/// How the window opens in a gamescope session.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sizing {
    /// The scale factor to draw at; `None` leaves the one the user set.
    pub scale: Option<f32>,
    /// The window's size in device pixels: the whole output.
    pub width: u32,
    pub height: u32,
}

/// The window for a gamescope session whose output is `output` device pixels.
///
/// The window is always the output's size. The scale is the one at which the
/// Game Mode layout is [`LAYOUT_WIDTH`] wide, lowered when the output is so
/// wide for its height that [`LAYOUT_MIN_HEIGHT`] would not fit. It is
/// rounded down to a hundredth so the window's minimum size never rounds past
/// the output.
///
/// The scale the display reports plays no part. In a gaming session it
/// describes the panel (about 2.08 on a Steam Deck, 1.0 on a television), not
/// how the app should fill it: Steam's own screens fill the output at any
/// size, and so does this.
///
/// `user_scale` is true when the user set `SLINT_SCALE_FACTOR`; their value
/// is kept and only the window size is fitted. `None` when the output's size
/// is unknown or empty: nothing changes then.
pub fn window_sizing(output: Option<(u32, u32)>, user_scale: bool) -> Option<Sizing> {
    let (width, height) = output.filter(|(width, height)| *width > 0 && *height > 0)?;
    let scale = if user_scale {
        None
    } else {
        let fit = (width as f32 / LAYOUT_WIDTH).min(height as f32 / LAYOUT_MIN_HEIGHT);
        Some((fit * 100.0).floor() / 100.0)
    };
    Some(Sizing {
        scale,
        width,
        height,
    })
}

/// The value for `SLINT_SCALE_FACTOR`.
pub fn scale_text(scale: f32) -> String {
    format!("{scale:.2}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    }

    /// The app's own environment on a Steam Deck in Game Mode, read on
    /// 2026-11-22 (SteamOS 3.9.5).
    #[test]
    fn steam_game_mode_is_a_gamescope_session() {
        assert!(in_gamescope_session(env(&[
            ("XDG_CURRENT_DESKTOP", "gamescope"),
            ("XDG_SESSION_TYPE", "x11"),
            (
                "GAMESCOPE_WAYLAND_DISPLAY",
                "/run/pressure-vessel/gamescope-socket"
            ),
            ("DISPLAY", ":1"),
        ])));
    }

    #[test]
    fn either_sign_alone_is_enough() {
        assert!(in_gamescope_session(env(&[(
            "XDG_CURRENT_DESKTOP",
            "gamescope"
        )])));
        assert!(in_gamescope_session(env(&[(
            "XDG_CURRENT_DESKTOP",
            "Gamescope:KDE"
        )])));
        // A program started inside a nested gamescope on a desktop.
        assert!(in_gamescope_session(env(&[
            ("XDG_CURRENT_DESKTOP", "KDE"),
            ("GAMESCOPE_WAYLAND_DISPLAY", "gamescope-0"),
        ])));
    }

    #[test]
    fn desktop_sessions_are_not_gamescope_sessions() {
        for desktop in ["KDE", "GNOME", "Hyprland", "ubuntu:GNOME", ""] {
            assert!(
                !in_gamescope_session(env(&[("XDG_CURRENT_DESKTOP", desktop)])),
                "{desktop}"
            );
        }
        assert!(!in_gamescope_session(env(&[])));
        // Set but empty counts as not set.
        assert!(!in_gamescope_session(env(&[(
            "GAMESCOPE_WAYLAND_DISPLAY",
            ""
        )])));
    }

    /// Output, the scale picked, and the layout size that gives in logical
    /// pixels. The scale each display reports is noted for the record; by
    /// design it does not change the result.
    #[test]
    fn the_layout_fills_the_output() {
        type Case = ((u32, u32), f32, (f32, f32));
        let cases: [Case; 9] = [
            // Steam Deck, reports about 2.08.
            ((1280, 800), 2.0, (640.0, 400.0)),
            // Television at 1080p, reports 1.0 or 2.0.
            ((1920, 1080), 3.0, (640.0, 360.0)),
            // Television at 4K, reports 2.0.
            ((3840, 2160), 6.0, (640.0, 360.0)),
            // Docked Deck at its default game resolution on a 16:9 screen.
            ((1280, 720), 2.0, (640.0, 360.0)),
            // A small output: the layout still gets its 640 pixels.
            ((800, 600), 1.25, (640.0, 480.0)),
            ((640, 480), 1.0, (640.0, 480.0)),
            // Wide for its height: the minimum height decides.
            ((2560, 1080), 3.0, (853.3, 360.0)),
            ((3440, 1440), 4.0, (860.0, 360.0)),
            // 1366x768 does not divide evenly; rounding down keeps the
            // minimum size (640 x 2.13 = 1363.2) inside the output.
            ((1366, 768), 2.13, (641.3, 360.6)),
        ];
        for (output, scale, layout) in cases {
            let sizing = window_sizing(Some(output), false).unwrap();
            assert_eq!((sizing.width, sizing.height), output, "{output:?}");
            let picked = sizing.scale.unwrap();
            assert!((picked - scale).abs() < 0.001, "{output:?}: {picked}");
            let (width, height) = (output.0 as f32 / picked, output.1 as f32 / picked);
            assert!((width - layout.0).abs() < 0.1, "{output:?}: {width}");
            assert!((height - layout.1).abs() < 0.1, "{output:?}: {height}");
            // The window's minimum size, in device pixels, fits the output.
            assert!(LAYOUT_WIDTH * picked <= output.0 as f32, "{output:?}");
            assert!(LAYOUT_MIN_HEIGHT * picked <= output.1 as f32, "{output:?}");
        }
    }

    #[test]
    fn an_output_smaller_than_the_layout_shrinks_it_to_fit() {
        let sizing = window_sizing(Some((480, 320)), false).unwrap();
        assert_eq!(sizing.scale, Some(0.75));
    }

    #[test]
    fn a_scale_the_user_set_is_kept_and_the_window_still_fits() {
        assert_eq!(
            window_sizing(Some((1280, 800)), true),
            Some(Sizing {
                scale: None,
                width: 1280,
                height: 800,
            })
        );
    }

    #[test]
    fn an_unknown_or_empty_output_changes_nothing() {
        assert_eq!(window_sizing(None, false), None);
        assert_eq!(window_sizing(Some((0, 800)), false), None);
        assert_eq!(window_sizing(Some((1280, 0)), true), None);
    }

    #[test]
    fn scale_is_written_with_two_decimals() {
        assert_eq!(scale_text(2.0), "2.00");
        assert_eq!(scale_text(2.13), "2.13");
        assert_eq!(scale_text(0.75), "0.75");
    }
}
