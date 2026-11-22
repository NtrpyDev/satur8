//! Locate a game's art in the user's *local* Steam install. We only read files
//! Steam already cached on this machine and display them locally - nothing is
//! bundled or redistributed, so there's no asset-licensing problem for the repo.

use std::path::{Path, PathBuf};

/// A few common games, so profiles added without a detected AppID still get art.
pub fn known_app_id(exe_or_name: &str) -> Option<u32> {
    let k = exe_or_name.to_ascii_lowercase();
    let k = k.trim_end_matches(".exe").trim_end_matches(".x86_64");
    Some(match k {
        "cs2" | "csgo" => 730,
        "dota2" | "dota" => 570,
        "r5apex" | "apex" => 1172470,
        "valorant" => return None,
        "tf_linux64" | "tf2" => 440,
        "left4dead2" => 550,
        "hl2" => 220,
        _ => return None,
    })
}

fn roots() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    [
        ".local/share/Steam",
        ".steam/steam",
        ".steam/root",
        ".var/app/com.valvesoftware.Steam/.local/share/Steam",
    ]
    .iter()
    .map(|p| PathBuf::from(&home).join(p))
    .filter(|p| p.is_dir())
    .collect()
}

fn cache_dir(app_id: u32) -> Option<PathBuf> {
    for root in roots() {
        let d = root.join("appcache/librarycache").join(app_id.to_string());
        if d.is_dir() {
            return Some(d);
        }
    }
    None
}

fn first_existing(dir: &Path, names: &[&str]) -> Option<PathBuf> {
    for n in names {
        let p = dir.join(n);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// Steam's small square game icon. Steam caches it at the top of the app's
/// librarycache folder under a 40-character hex name, e.g. `8dbc71...a355.jpg`.
fn game_icon(dir: &Path) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path.extension().is_some_and(|ext| ext == "jpg")
                && path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|stem| {
                        stem.len() == 40 && stem.bytes().all(|b| b.is_ascii_hexdigit())
                    })
        })
        .collect();
    found.sort();
    found.into_iter().next()
}

/// Newer Steam clients keep some art in hash-named subfolders of the app's
/// librarycache folder instead of at its top level.
fn first_existing_in_subdirs(dir: &Path, names: &[&str]) -> Option<PathBuf> {
    let mut subdirs: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    subdirs.sort();
    names
        .iter()
        .find_map(|name| subdirs.iter().map(|d| d.join(name)).find(|p| p.is_file()))
}

/// Art for the card tile: Steam's square game icon first, then the earlier
/// box art / header / logo fallbacks, at the top level or in a subfolder.
pub fn icon_path(app_id: u32) -> Option<PathBuf> {
    let dir = cache_dir(app_id)?;
    const FALLBACKS: [&str; 3] = ["library_600x900.jpg", "header.jpg", "logo.png"];
    game_icon(&dir)
        .or_else(|| first_existing(&dir, &FALLBACKS))
        .or_else(|| first_existing_in_subdirs(&dir, &FALLBACKS))
}
