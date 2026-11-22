//! Whether Satur8's look reaches the screen in Steam Game Mode, and when
//! gamescope has to be told to composite for it to.
//!
//! gamescope colors a window it shows directly (scanned out on a display
//! plane, no composite pass) only through AMD's plane color properties
//! (`drm_supports_color_mgmt` checks the primary plane for `AMD_PLANE_CTM`
//! and `AMD_PLANE_BLEND_TF`, gamescope 3.17.0 `DRMBackend.cpp:3796-3805`). On
//! any other GPU the look table is used only when gamescope composites a
//! frame, so a fullscreen game shown directly gets no look at all. Read from
//! the screen on an RTX 3070 Ti under Bazzite, 2026-11-22: the game's own
//! 200,50,50 while the look was loaded, 255,24,25 once composition was forced.
//!
//! `GAMESCOPE_COMPOSITE_FORCE` on the root makes gamescope composite every
//! frame (`steamcompmgr.cpp:7341-7344`). That costs one GPU pass per frame,
//! so Satur8 sets it only while its own look is on and only on GPUs that need
//! it.

use std::fs;

use satur8_core::Gpu;

/// Whether gamescope needs composition forced for a look to show on `gpu`.
///
/// Only AMD has the plane color properties, so everything else is forced,
/// [`Gpu::Unknown`] included: a GPU Satur8 cannot name is not one whose
/// driver is `amdgpu` (that one is always named), so its planes cannot have
/// AMD's properties and the look would otherwise not show. The cost of the
/// extra pass is the price of the look being visible at all.
pub fn needs_forced_composition(gpu: Gpu) -> bool {
    gpu != Gpu::Amd
}

/// gamescope's short options that take a value, from its option table
/// (gamescope 3.17.0 `main.cpp`, `build_optstring`). Needed to tell `-c` from
/// a `c` inside another option's value.
const SHORT_WITH_VALUE: &str = "whrmSFWHsoORTC";

/// Whether gamescope's arguments force composition: `-c`, alone or in a
/// group of short options, or `--force-composition`. Options stop at `--`;
/// before it, GNU getopt also reads options that follow plain arguments.
pub fn forced_by_args<S: AsRef<str>>(args: &[S]) -> bool {
    let mut args = args.iter().skip(1).map(AsRef::as_ref);
    while let Some(arg) = args.next() {
        if arg == "--" {
            return false;
        }
        if arg == "--force-composition" {
            return true;
        }
        if arg.starts_with("--") || !arg.starts_with('-') || arg == "-" {
            continue;
        }
        for (index, flag) in arg.char_indices().skip(1) {
            if flag == 'c' {
                return true;
            }
            if SHORT_WITH_VALUE.contains(flag) {
                // The value is the rest of this argument, or the next one.
                if index + flag.len_utf8() == arg.len() {
                    args.next();
                }
                break;
            }
        }
    }
    false
}

/// Whether gamescope's environment forces composition: gamescope turns it on
/// when `ENABLE_VKBASALT` starts with `1` (`steamcompmgr.cpp:10104-10108`).
pub fn forced_by_environment(environ: &[u8]) -> bool {
    environ
        .split(|byte| *byte == 0)
        .any(|entry| entry.starts_with(b"ENABLE_VKBASALT=1"))
}

/// Whether the gamescope process `pid` forces composition by its own
/// settings, which removing the root property would end. `Some(true)` when
/// its arguments or environment say so, `Some(false)` when both were read and
/// neither does, `None` when its environment cannot be read. Bazzite's
/// gamescope has `cap_sys_nice=eip`, which makes `/proc/<pid>/environ`
/// readable by root only (read on Bazzite 2026-11-22); its command line is
/// always readable.
pub fn gamescope_forces_itself(pid: u32) -> Option<bool> {
    let args = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let args: Vec<String> = args
        .split(|byte| *byte == 0)
        .filter(|arg| !arg.is_empty())
        .map(|arg| String::from_utf8_lossy(arg).into_owned())
        .collect();
    if forced_by_args(&args) {
        return Some(true);
    }
    let environ = fs::read(format!("/proc/{pid}/environ")).ok()?;
    Some(forced_by_environment(&environ))
}

/// Whether Satur8's look shows on screen in Steam Game Mode. It shows only
/// when gamescope's color management is on and either the GPU is AMD or
/// something makes gamescope composite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnScreen {
    /// `GAMESCOPE_COLOR_MANAGEMENT_DISABLE` is set: gamescope ignores every
    /// color setting, the look included, on any GPU.
    ColorManagementOff,
    /// AMD: gamescope colors directly shown windows too.
    Always,
    /// Another GPU, with composition forced by the root property.
    Composited,
    /// Another GPU, with composition forced by gamescope's own settings.
    ComposedByGamescope,
    /// Another GPU and nothing forces composition: a fullscreen game is
    /// shown directly, without the look.
    NotForDirectWindows,
    /// Another GPU and something set the property to 0.
    TurnedOffByOther,
}

/// Where the look shows on `gpu`, given whether gamescope's color management
/// is disabled, the root's `GAMESCOPE_COMPOSITE_FORCE` (`None` when absent)
/// and whether gamescope forces composition itself.
pub fn on_screen(
    gpu: Gpu,
    color_management_disabled: bool,
    composite_force: Option<u32>,
    forces_itself: Option<bool>,
) -> OnScreen {
    if color_management_disabled {
        return OnScreen::ColorManagementOff;
    }
    if !needs_forced_composition(gpu) {
        return OnScreen::Always;
    }
    match composite_force {
        Some(0) => OnScreen::TurnedOffByOther,
        Some(_) => OnScreen::Composited,
        None if forces_itself == Some(true) => OnScreen::ComposedByGamescope,
        None => OnScreen::NotForDirectWindows,
    }
}

/// The words every report uses (`satur8 doctor`, `satur8 status`, the app's
/// "Right now" card and Health row) when gamescope's color management is off.
pub const COLOR_MANAGEMENT_OFF: &str =
    "gamescope's color management is off, so Satur8's setting is not shown";

/// The words for a loaded look on a GPU where nothing makes gamescope
/// composite.
pub const NOT_ON_SCREEN: &str = "Satur8's setting is not shown in a fullscreen game: on this \
     GPU gamescope shows such a game directly, without it, unless it composites";

/// The words for a GPU where another program set the force to 0.
pub const FORCE_TURNED_OFF: &str = "Satur8's setting is not shown in a fullscreen game: another \
     program set GAMESCOPE_COMPOSITE_FORCE to 0, so gamescope shows such a game directly";

/// What is wrong with the screen in this state, in the shared words, or
/// `None` when nothing is. Color management off, and another program's 0,
/// matter whether or not a Satur8 look is loaded; with nothing making
/// gamescope composite, only a loaded look is missing (Satur8 forces
/// composition when it loads one).
pub fn screen_problem(state: OnScreen, satur8_look_loaded: bool) -> Option<&'static str> {
    match state {
        OnScreen::ColorManagementOff => Some(COLOR_MANAGEMENT_OFF),
        OnScreen::TurnedOffByOther => Some(FORCE_TURNED_OFF),
        OnScreen::NotForDirectWindows if satur8_look_loaded => Some(NOT_ON_SCREEN),
        _ => None,
    }
}

/// [`screen_problem`] for gamescope's state as read from its root.
pub fn problem_for(
    gpu: Gpu,
    state: &crate::session::ColorState,
    forces_itself: Option<bool>,
) -> Option<&'static str> {
    let on = on_screen(
        gpu,
        state.color_management_disabled,
        state.composite_force,
        forces_itself,
    );
    let loaded = matches!(state.look, crate::session::Look::Satur8(_));
    screen_problem(on, loaded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<&str> {
        line.split(' ').collect()
    }

    #[test]
    fn only_amd_shows_the_look_without_forced_composition() {
        assert!(!needs_forced_composition(Gpu::Amd));
        assert!(needs_forced_composition(Gpu::Nvidia));
        assert!(needs_forced_composition(Gpu::Nouveau));
        assert!(needs_forced_composition(Gpu::Intel));
        assert!(needs_forced_composition(Gpu::Unknown));
    }

    #[test]
    fn force_composition_is_found_in_every_spelling() {
        assert!(forced_by_args(&args("gamescope -c -e")));
        assert!(forced_by_args(&args("gamescope --force-composition")));
        assert!(forced_by_args(&args("gamescope -ec")));
        assert!(forced_by_args(&args("gamescope -W 1920 -c")));
        // GNU getopt reads options after a plain argument too.
        assert!(forced_by_args(&args("gamescope stray -c")));
    }

    /// Bazzite's Game Mode gamescope on the RTX 3070 Ti, read from
    /// `/proc/<pid>/cmdline` on 2026-11-22, has no `-c`; a `c` inside a value
    /// or after `--` is not the option either.
    #[test]
    fn values_and_the_game_command_are_not_options() {
        assert!(!forced_by_args(&args(
            "/usr/bin/gamescope --touch-gestures --prefer-output *,eDP-1 \
             --xwayland-count 2 --default-touch-mode 4 --hide-cursor-delay 3000 \
             --fade-out-duration 200 --steam \
             -R /run/user/1000/gamescope.aITrnrO/startup.socket \
             -T /run/user/1000/gamescope.aITrnrO/stats.pipe"
        )));
        assert!(!forced_by_args(&args(
            "gamescope -R /run/c -T /run/c --steam"
        )));
        assert!(!forced_by_args(&args("gamescope -O cc -e")));
        assert!(!forced_by_args(&args("gamescope -Occ -e")));
        assert!(!forced_by_args(&args("gamescope -- game -c")));
        assert!(!forced_by_args(&args("-c")), "argv[0] is the program");
    }

    #[test]
    fn vkbasalt_in_the_environment_forces_composition() {
        assert!(forced_by_environment(
            b"HOME=/home/deck\0ENABLE_VKBASALT=1\0"
        ));
        assert!(!forced_by_environment(
            b"HOME=/home/deck\0ENABLE_VKBASALT=0\0"
        ));
        assert!(!forced_by_environment(b"X_ENABLE_VKBASALT=1\0"));
        assert!(!forced_by_environment(b""));
    }

    #[test]
    fn the_look_shows_on_amd_or_when_something_composites() {
        assert_eq!(on_screen(Gpu::Amd, false, None, None), OnScreen::Always);
        assert_eq!(on_screen(Gpu::Amd, false, Some(0), None), OnScreen::Always);
        assert_eq!(
            on_screen(Gpu::Nvidia, false, Some(1), None),
            OnScreen::Composited
        );
        assert_eq!(
            on_screen(Gpu::Nvidia, false, None, Some(true)),
            OnScreen::ComposedByGamescope
        );
        assert_eq!(
            on_screen(Gpu::Intel, false, None, Some(false)),
            OnScreen::NotForDirectWindows
        );
        assert_eq!(
            on_screen(Gpu::Unknown, false, None, None),
            OnScreen::NotForDirectWindows
        );
        assert_eq!(
            on_screen(Gpu::Nvidia, false, Some(0), Some(true)),
            OnScreen::TurnedOffByOther
        );
    }

    /// Color management off wins on every GPU and with any force: gamescope
    /// then ignores the look entirely.
    #[test]
    fn color_management_off_hides_the_look_on_every_gpu() {
        for gpu in [Gpu::Amd, Gpu::Nvidia, Gpu::Intel, Gpu::Unknown] {
            for force in [None, Some(0), Some(1)] {
                for itself in [None, Some(false), Some(true)] {
                    assert_eq!(
                        on_screen(gpu, true, force, itself),
                        OnScreen::ColorManagementOff
                    );
                }
            }
        }
    }

    #[test]
    fn each_state_has_one_message() {
        use OnScreen::*;
        for loaded in [false, true] {
            assert_eq!(
                screen_problem(ColorManagementOff, loaded),
                Some(COLOR_MANAGEMENT_OFF)
            );
            assert_eq!(
                screen_problem(TurnedOffByOther, loaded),
                Some(FORCE_TURNED_OFF)
            );
            assert_eq!(screen_problem(Always, loaded), None);
            assert_eq!(screen_problem(Composited, loaded), None);
            assert_eq!(screen_problem(ComposedByGamescope, loaded), None);
        }
        assert_eq!(
            screen_problem(NotForDirectWindows, true),
            Some(NOT_ON_SCREEN)
        );
        assert_eq!(screen_problem(NotForDirectWindows, false), None);
    }
}
