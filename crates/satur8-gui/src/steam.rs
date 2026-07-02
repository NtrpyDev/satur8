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

/// Square-ish art for the row tile (portrait box art looks best as a tile).
pub fn icon_path(app_id: u32) -> Option<PathBuf> {
    let dir = cache_dir(app_id)?;
    first_existing(&dir, &["library_600x900.jpg", "header.jpg", "logo.png"])
}

/// Wide art for the before/after preview (the hero banner / header).
pub fn preview_path(app_id: u32) -> Option<PathBuf> {
    let dir = cache_dir(app_id)?;
    first_existing(
        &dir,
        &["header.jpg", "library_hero.jpg", "library_600x900.jpg"],
    )
}
