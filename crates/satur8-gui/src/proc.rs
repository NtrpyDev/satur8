//! Enumerate running userspace programs so the GUI can offer a "pick a running
//! game" list - and, for Steam games, recover the Steam AppID from the process
//! environment so we can show the game's real icon and art.

use std::collections::BTreeMap;
use std::fs;

/// Wine/Proton uses one shared loader binary for every Windows process, so
/// `/proc/<pid>/exe` is `wine64-preloader`, not the game. The real image name
/// is in `argv[0]`, as a Windows path.
const WINE_LOADERS: &[&str] = &["wine", "wine64", "wine-preloader", "wine64-preloader"];

/// Wine's own processes. They are not games and would otherwise show up once
/// `argv[0]` is used as the display name for Wine processes.
const WINE_SYSTEM_EXES: &[&str] = &[
    "wineserver",
    "services",
    "winedevice",
    "plugplay",
    "svchost",
    "lsass",
    "explorer",
    "rpcss",
    "tabtip",
    "conhost",
    "steam",
    "steamwebhelper",
    "gameoverlayui",
    "steamerrorreporter",
    "wineboot",
    "winemenubuilder",
];

pub struct RunningApp {
    pub exe: String,
    pub app_id: Option<u32>,
}

/// Distinct running programs (by exe basename), each with its Steam AppID if it
/// is a Steam game. Kernel threads, Wine's own helper processes, and our own
/// tooling are skipped.
pub fn running_apps() -> Vec<RunningApp> {
    // Keyed by exe so we dedup; prefer an entry that carries an AppID.
    let mut map: BTreeMap<String, Option<u32>> = BTreeMap::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let pid = entry.file_name().to_string_lossy().to_string();
        if !pid.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(target) = fs::read_link(format!("/proc/{pid}/exe")) else {
            continue;
        };
        let Some(base) = target.file_name().map(|n| n.to_string_lossy().to_string()) else {
            continue;
        };
        if base.is_empty() || base.starts_with("satur8") {
            continue;
        }

        // Under Wine the loader is shared by every process in the prefix. Use
        // the image name from argv[0] instead, and skip Wine's system helpers.
        let name = if is_wine_loader(&base) {
            let Some(name) = wine_process_name(&pid) else {
                continue;
            };
            name
        } else if base == "wineserver" {
            continue;
        } else {
            base
        };
        if name.is_empty() || name.starts_with("satur8") {
            continue;
        }

        let app_id = steam_app_id_of(&pid);
        map.entry(name)
            .and_modify(|cur| {
                if cur.is_none() {
                    *cur = app_id;
                }
            })
            .or_insert(app_id);
    }
    map.into_iter()
        .map(|(exe, app_id)| RunningApp { exe, app_id })
        .collect()
}

fn is_wine_loader(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    WINE_LOADERS.iter().any(|loader| *loader == lower)
}

fn is_wine_system_exe(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".exe").unwrap_or(&lower);
    WINE_SYSTEM_EXES.iter().any(|exe| *exe == stem) || is_wine_loader(stem)
}

/// Turn a Wine `argv[0]` (usually a Windows path) into the game's exe name.
/// Returns `None` for Wine's own system processes and loaders.
fn wine_image_name(argv0: &str) -> Option<String> {
    let argv0 = argv0.trim();
    if argv0.is_empty() {
        return None;
    }
    let normalized = argv0.replace('\\', "/").to_ascii_lowercase();
    if normalized.contains("/windows/") {
        return None;
    }
    let name = argv0.rsplit(|c| c == '\\' || c == '/').next().unwrap_or("");
    if name.is_empty() || is_wine_system_exe(name) {
        return None;
    }
    Some(name.to_string())
}

fn wine_process_name(pid: &str) -> Option<String> {
    let cmdline = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let argv0 = cmdline.split(|b| *b == 0).next()?;
    if argv0.is_empty() {
        return None;
    }
    wine_image_name(&String::from_utf8_lossy(argv0))
}

/// Read a process's environment for `SteamAppId` / `SteamGameId`.
fn steam_app_id_of(pid: &str) -> Option<u32> {
    let data = fs::read(format!("/proc/{pid}/environ")).ok()?;
    for var in data.split(|&b| b == 0) {
        let s = String::from_utf8_lossy(var);
        for key in ["SteamAppId=", "SteamGameId=", "STEAM_COMPAT_APP_ID="] {
            if let Some(v) = s.strip_prefix(key) {
                if let Ok(id) = v.trim().parse::<u32>() {
                    if id != 0 {
                        return Some(id);
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wine_paths_resolve_to_the_game_exe() {
        assert_eq!(
            wine_image_name(r"Z:\mnt\games\Juvio\bin\juvio.exe").as_deref(),
            Some("juvio.exe")
        );
        assert_eq!(
            wine_image_name(r"C:\Program Files\Game\Game.exe").as_deref(),
            Some("Game.exe")
        );
        assert_eq!(
            wine_image_name("/home/user/games/game/game.exe").as_deref(),
            Some("game.exe")
        );
    }

    #[test]
    fn wine_system_processes_are_skipped() {
        for argv0 in [
            r"C:\windows\system32\services.exe",
            r"c:\windows\system32\steam.exe",
            r"C:\windows\system32\explorer.exe",
            r"C:\windows\system32\winedevice.exe",
            "wineserver",
            "wine64-preloader",
        ] {
            assert_eq!(wine_image_name(argv0), None, "argv0: {argv0}");
        }
    }

    #[test]
    fn wine_loaders_are_recognized() {
        assert!(is_wine_loader("wine64-preloader"));
        assert!(is_wine_loader("wine-preloader"));
        assert!(!is_wine_loader("juvio.exe"));
    }
}
