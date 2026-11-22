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

/// Whether a loaded look is on screen, as `satur8 doctor` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnScreen {
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

/// Where the look shows on `gpu`, given the root's
/// `GAMESCOPE_COMPOSITE_FORCE` (`None` when absent) and whether gamescope
/// forces composition itself.
pub fn on_screen(gpu: Gpu, composite_force: Option<u32>, forces_itself: Option<bool>) -> OnScreen {
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
        assert_eq!(on_screen(Gpu::Amd, None, None), OnScreen::Always);
        assert_eq!(on_screen(Gpu::Amd, Some(0), None), OnScreen::Always);
        assert_eq!(on_screen(Gpu::Nvidia, Some(1), None), OnScreen::Composited);
        assert_eq!(
            on_screen(Gpu::Nvidia, None, Some(true)),
            OnScreen::ComposedByGamescope
        );
        assert_eq!(
            on_screen(Gpu::Intel, None, Some(false)),
            OnScreen::NotForDirectWindows
        );
        assert_eq!(
            on_screen(Gpu::Unknown, None, None),
            OnScreen::NotForDirectWindows
        );
        assert_eq!(
            on_screen(Gpu::Nvidia, Some(0), Some(true)),
            OnScreen::TurnedOffByOther
        );
    }
}
