//! Resolving a running process to the program name a human would recognise.
//!
//! `/proc/<pid>/exe` is the obvious source and it is wrong for the exact case
//! that matters most: a Windows game under Proton/Wine. Every one of those
//! processes has `exe` pointing at the Wine ELF loader (`wine64-preloader`),
//! and `comm` truncated to 15 bytes (`EscapeFromTarko`). The real program name
//! only survives in `cmdline`, as a Windows path:
//!
//! ```text
//! \\?\C:\Battlestate Games\Escape from Tarkov\EscapeFromTarkov.exe -force-gfx-jobs ...
//! ```
//!
//! So we resolve through the loader when we see one. The GUI uses this to list
//! running games, and the daemon uses it to turn the focused window's pid into
//! the same name, which is what makes "pick it from the list" actually apply.

use std::collections::BTreeMap;
use std::fs;

/// A distinct running program, as offered in the GUI's "add running game" list.
pub struct RunningApp {
    /// Name used for display and for matching, e.g. `cs2`, `EscapeFromTarkov.exe`.
    pub exe: String,
    /// Steam AppID if the process environment carries one.
    pub app_id: Option<u32>,
    /// Looks like a game rather than a desktop service. Games sort first.
    pub is_game: bool,
}

/// Wine's ELF loaders. A process running one of these is a Windows program and
/// its real name has to come from `cmdline`.
fn is_wine_loader(base: &str) -> bool {
    matches!(
        base,
        "wine" | "wine64" | "wine-preloader" | "wine64-preloader" | "wineserver"
    )
}

/// Wine's own plumbing plus the launcher-side helpers. These are always running
/// alongside a Proton game and are never something you would profile.
fn is_wine_plumbing(base: &str) -> bool {
    const PLUMBING: &[&str] = &[
        "services.exe",
        "winedevice.exe",
        "plugplay.exe",
        "svchost.exe",
        "explorer.exe",
        "rpcss.exe",
        "wineboot.exe",
        "winemenubuilder.exe",
        "conhost.exe",
        "start.exe",
        "tabtip.exe",
        "umu.exe",
        "xalia.exe",
        "unitycrashhandler64.exe",
        "unitycrashhandler32.exe",
        "lsass.exe",
        "steam.exe",
        "steamwebhelper.exe",
        "gameoverlayui.exe",
        "steamerrorreporter.exe",
    ];
    let lower = base.to_ascii_lowercase();
    PLUMBING.contains(&lower.as_str())
}

/// The Windows executable basename from a Wine process's `argv[0]`, if any.
/// Handles the `\\?\` long-path prefix and both slash flavours.
fn windows_exe_from_arg0(arg0: &str) -> Option<String> {
    let base = arg0.trim().rsplit(['\\', '/']).next()?.trim();
    if base.len() > 4 && base.to_ascii_lowercase().ends_with(".exe") {
        Some(base.to_string())
    } else {
        None
    }
}

/// Whether a Wine `argv[0]` points into the prefix's Windows directory. Nothing
/// under `C:\windows` is a game, whatever it is called.
fn is_windows_system_path(arg0: &str) -> bool {
    arg0.replace('\\', "/")
        .to_ascii_lowercase()
        .contains("/windows/")
}

/// The game executable named by a Wine process's `argv[0]`, or `None` for
/// Wine's own system processes.
fn wine_game_from_arg0(arg0: &str) -> Option<String> {
    if is_windows_system_path(arg0) {
        return None;
    }
    windows_exe_from_arg0(arg0).filter(|exe| !is_wine_plumbing(exe))
}

/// The game executable from a Wine process's `cmdline`, if any.
fn windows_exe_of(pid: u32) -> Option<String> {
    let data = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let arg0 = data.split(|&byte| byte == 0).next()?;
    wine_game_from_arg0(&String::from_utf8_lossy(arg0))
}

/// The program name for a pid: the Windows executable for a Wine process, the
/// `exe` symlink's basename otherwise. `None` for kernel threads, processes we
/// cannot read, and Wine's own plumbing.
pub fn resolve_exe(pid: u32) -> Option<String> {
    let target = fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    let base = target.file_name()?.to_string_lossy().to_string();
    if base.is_empty() {
        return None;
    }
    if is_wine_loader(&base) {
        return windows_exe_of(pid);
    }
    Some(base)
}

/// Read a process's environment for a Steam AppID.
pub fn steam_app_id_of(pid: u32) -> Option<u32> {
    let data = fs::read(format!("/proc/{pid}/environ")).ok()?;
    for var in data.split(|&byte| byte == 0) {
        let var = String::from_utf8_lossy(var);
        for key in ["SteamAppId=", "SteamGameId=", "STEAM_COMPAT_APP_ID="] {
            if let Some(value) = var.strip_prefix(key) {
                if let Ok(id) = value.trim().parse::<u32>() {
                    if id != 0 {
                        return Some(id);
                    }
                }
            }
        }
    }
    None
}

/// Whether a process was launched through Steam, Proton, umu, or plain Wine.
/// umu sets `SteamGameId=0` for non-Steam titles, so presence matters here, not
/// the value - which is also why the AppID above is not a reliable game test.
fn looks_like_game(pid: u32) -> bool {
    let Ok(data) = fs::read(format!("/proc/{pid}/environ")) else {
        return false;
    };
    for var in data.split(|&byte| byte == 0) {
        let var = String::from_utf8_lossy(var);
        for key in [
            "SteamAppId=",
            "SteamGameId=",
            "STEAM_COMPAT_APP_ID=",
            "UMU_ID=",
            "WINEPREFIX=",
            "PROTONPATH=",
        ] {
            if var.starts_with(key) {
                return true;
            }
        }
    }
    false
}

/// Distinct running programs, games first and each group alphabetical. Kernel
/// threads, Wine plumbing, and our own tooling are skipped.
pub fn running_apps() -> Vec<RunningApp> {
    // Keyed by name so we dedup the many processes one game spawns; prefer an
    // entry that carries an AppID or that we recognised as a game.
    let mut map: BTreeMap<String, (Option<u32>, bool)> = BTreeMap::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Some(exe) = resolve_exe(pid) else {
            continue;
        };
        if exe.starts_with("satur8") {
            continue;
        }
        let app_id = steam_app_id_of(pid);
        let is_game = exe.to_ascii_lowercase().ends_with(".exe") || looks_like_game(pid);
        map.entry(exe)
            .and_modify(|current| {
                if current.0.is_none() {
                    current.0 = app_id;
                }
                current.1 |= is_game;
            })
            .or_insert((app_id, is_game));
    }
    let mut apps: Vec<RunningApp> = map
        .into_iter()
        .map(|(exe, (app_id, is_game))| RunningApp {
            exe,
            app_id,
            is_game,
        })
        .collect();
    apps.sort_by(|a, b| {
        b.is_game
            .cmp(&a.is_game)
            .then_with(|| a.exe.to_ascii_lowercase().cmp(&b.exe.to_ascii_lowercase()))
    });
    apps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wine_loaders_are_recognised() {
        assert!(is_wine_loader("wine64-preloader"));
        assert!(!is_wine_loader("cs2"));
    }

    #[test]
    fn wine_plumbing_is_filtered() {
        assert!(is_wine_plumbing("explorer.exe"));
        assert!(is_wine_plumbing("WineDevice.exe"));
        assert!(!is_wine_plumbing("EscapeFromTarkov.exe"));
    }

    /// Verbatim `argv[0]` read from a live Escape from Tarkov process running
    /// under GE-Proton, which is the case this whole module exists for.
    #[test]
    fn tarkov_argv0_resolves_to_the_game() {
        let arg0 = r"\\?\C:\Battlestate Games\Escape from Tarkov\EscapeFromTarkov.exe";
        assert_eq!(
            windows_exe_from_arg0(arg0).as_deref(),
            Some("EscapeFromTarkov.exe")
        );
    }

    #[test]
    fn unix_paths_and_non_exe_arg0_are_rejected() {
        assert_eq!(windows_exe_from_arg0("/usr/bin/wineserver"), None);
        assert_eq!(windows_exe_from_arg0(""), None);
        assert_eq!(
            windows_exe_from_arg0(r"C:\windows\system32\explorer.exe").as_deref(),
            Some("explorer.exe")
        );
    }

    #[test]
    fn wine_paths_resolve_to_the_game_exe() {
        for (arg0, exe) in [
            (r"Z:\mnt\games\Juvio\bin\juvio.exe", "juvio.exe"),
            (r"C:\Program Files\Game\Game.exe", "Game.exe"),
            ("/home/user/games/game/game.exe", "game.exe"),
        ] {
            assert_eq!(wine_game_from_arg0(arg0).as_deref(), Some(exe), "{arg0}");
        }
    }

    #[test]
    fn wine_system_processes_are_skipped() {
        for arg0 in [
            r"C:\windows\system32\services.exe",
            r"c:\windows\system32\steam.exe",
            r"C:\windows\system32\explorer.exe",
            r"C:\windows\system32\winedevice.exe",
            r"C:\Program Files (x86)\Steam\steamwebhelper.exe",
            "wineserver",
            "wine64-preloader",
        ] {
            assert_eq!(wine_game_from_arg0(arg0), None, "{arg0}");
        }
    }

    /// The Sushiamo window's process on a Steam Deck under Proton 11, read in
    /// Game Mode on 2026-11-21 (the start of the `exe` path was not recorded):
    /// `exe` is the Wine loader and `argv[0]` is the game on the `S:` drive.
    /// These are the two steps `resolve_exe` takes.
    #[test]
    fn proton_game_on_the_steam_library_drive_resolves_to_the_game() {
        let exe = "Proton 11.0/files/lib/wine/x86_64-unix/wine64-preloader";
        let base = std::path::Path::new(exe)
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        assert!(is_wine_loader(base));
        let cmdline = b"S:\\steamapps\\common\\Sushiamo\\Sushiamo.exe\0";
        let arg0 = cmdline.split(|&byte| byte == 0).next().unwrap();
        assert_eq!(
            wine_game_from_arg0(&String::from_utf8_lossy(arg0)).as_deref(),
            Some("Sushiamo.exe")
        );
    }

    #[test]
    fn our_own_pid_resolves() {
        let pid = std::process::id();
        assert!(resolve_exe(pid).is_some());
    }
}
