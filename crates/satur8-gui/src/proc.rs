//! Enumerate running userspace programs so the GUI can offer a "pick a running
//! game" list - and, for Steam games, recover the Steam AppID from the process
//! environment so we can show the game's real icon and art.

use std::collections::BTreeMap;
use std::fs;

pub struct RunningApp {
    pub exe: String,
    pub app_id: Option<u32>,
}

/// Distinct running programs (by exe basename), each with its Steam AppID if it
/// is a Steam game. Kernel threads and our own tooling are skipped.
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
        if base.is_empty() || base.starts_with("vibrance") {
            continue;
        }
        let app_id = steam_app_id_of(&pid);
        map.entry(base)
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
