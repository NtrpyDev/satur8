//! `satur8 steam-shortcut ...` - Satur8's own entry in the Steam library.
//!
//! Steam Game Mode shows only programs Steam started, so on a Steam Deck the
//! desktop app is opened from a library entry (a "non-Steam game"). This
//! command says whether that entry exists, asks Steam to add it where SteamOS
//! has a tool for that, and puts the entry's artwork in place. It never
//! writes Steam's `shortcuts.vdf`: Steam rewrites that file while it runs.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Subcommand;
use satur8_core::steam_shortcut::{self, Found};

/// SteamOS's tool that asks the running Steam to add a desktop entry.
const ADD_TOOL: &str = "steamos-add-to-steam";

/// How to remove a library entry: only Steam's own interface does that.
const HOW_TO_REMOVE: &str = "select it in the Steam library, open its settings (the gear), \
                             then Manage, then \"Remove non-Steam game from your library\"";

#[derive(Subcommand, Debug, PartialEq, Eq)]
pub enum SteamShortcutCmd {
    /// Say whether Satur8 is in the Steam library.
    Status,
    /// Ask Steam to add the desktop app to the library, unless it is there.
    Add {
        /// The launcher entry to add (default: the installed satur8.desktop).
        #[arg(long)]
        desktop_file: Option<PathBuf>,
        /// Ask even though Steam was asked before and the entry is not listed.
        #[arg(long)]
        again: bool,
    },
    /// Put Satur8's artwork on its library entry.
    Artwork {
        /// Take the artwork away again instead.
        #[arg(long)]
        remove: bool,
    },
}

pub fn run(cmd: SteamShortcutCmd) -> Result<()> {
    match cmd {
        SteamShortcutCmd::Status => {
            let found = steam_shortcut::find();
            for line in status_lines(&found, artwork_missing(&found), on_path(ADD_TOOL)) {
                println!("{line}");
            }
            Ok(())
        }
        SteamShortcutCmd::Add {
            desktop_file,
            again,
        } => add(desktop_file, again),
        SteamShortcutCmd::Artwork { remove } => {
            artwork(&steam_shortcut::find(), remove);
            Ok(())
        }
    }
}

/// How many of the entry's artwork files are not in place.
fn artwork_missing(found: &Found) -> usize {
    found
        .apps
        .iter()
        .filter(|(_, app_id)| *app_id != 0)
        .flat_map(|(user, app_id)| steam_shortcut::artwork_paths(user, *app_id))
        .filter(|(path, _)| !path.exists())
        .count()
}

/// What `satur8 steam-shortcut status` prints.
pub fn status_lines(found: &Found, artwork_missing: usize, has_add_tool: bool) -> Vec<String> {
    let mut lines = Vec::new();
    if found.users == 0 {
        lines.push(
            "satur8: no Steam user was found on this computer, so there is no Steam library to \
             look in"
                .to_string(),
        );
        return lines;
    }
    for (_, app_id) in &found.apps {
        lines.push(format!(
            "satur8: Satur8 is in the Steam library (AppID {app_id})"
        ));
    }
    if found.apps.is_empty() {
        lines.push("satur8: Satur8 is not in the Steam library".to_string());
        lines.push(if has_add_tool {
            "        add it with: satur8 steam-shortcut add".to_string()
        } else {
            "        add it in Steam: Add a Game, then Add a Non-Steam Game, then pick Satur8"
                .to_string()
        });
    } else if artwork_missing > 0 {
        lines.push(
            "        its artwork is not in place; put it there with: satur8 steam-shortcut artwork"
                .to_string(),
        );
    } else {
        lines.push("        its artwork is in place".to_string());
    }
    for (_, name) in &found.broken {
        lines.push(format!(
            "satur8: a leftover entry named \"{name}\" is in the Steam library and cannot start \
             anything"
        ));
        lines.push(format!("        to remove it: {HOW_TO_REMOVE}"));
    }
    lines
}

/// What `satur8 steam-shortcut add` does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddPlan {
    /// The entry exists; only its artwork is put in place.
    AlreadyThere,
    /// No Steam user here.
    NoSteam,
    /// Steam was asked before and has not listed the entry yet.
    AskedBefore,
    /// This system has no tool that asks Steam.
    NoTool,
    /// The tool talks to the running Steam, and none is running.
    SteamNotRunning,
    /// No launcher entry to hand to Steam.
    NoDesktopFile,
    Ask(PathBuf),
}

/// Decide what `add` does. Adding twice makes two entries, so it asks Steam
/// only when no entry is listed and Steam was not asked before (or `again`).
pub fn plan_add(
    found: &Found,
    asked_before: bool,
    again: bool,
    has_add_tool: bool,
    steam_running: bool,
    desktop_file: Option<PathBuf>,
) -> AddPlan {
    if !found.apps.is_empty() {
        AddPlan::AlreadyThere
    } else if found.users == 0 {
        AddPlan::NoSteam
    } else if asked_before && !again {
        AddPlan::AskedBefore
    } else if !has_add_tool {
        AddPlan::NoTool
    } else if !steam_running {
        AddPlan::SteamNotRunning
    } else {
        match desktop_file {
            Some(file) => AddPlan::Ask(file),
            None => AddPlan::NoDesktopFile,
        }
    }
}

fn add(desktop_file: Option<PathBuf>, again: bool) -> Result<()> {
    let found = steam_shortcut::find();
    let marker = asked_marker();
    let desktop_file = desktop_file.or_else(installed_desktop_file);
    let plan = plan_add(
        &found,
        marker.as_deref().is_some_and(Path::exists),
        again,
        on_path(ADD_TOOL),
        steam_shortcut::steam_is_running(),
        desktop_file,
    );
    match plan {
        AddPlan::AlreadyThere => {
            for line in status_lines(&found, 0, true)
                .into_iter()
                .filter(|line| !line.contains("artwork"))
            {
                println!("{line}");
            }
            artwork(&found, false);
        }
        AddPlan::NoSteam => println!(
            "satur8: no Steam user was found on this computer, so there is no Steam library to \
             add to"
        ),
        AddPlan::AskedBefore => {
            println!(
                "satur8: Steam was already asked to add Satur8 and has not listed it in its \
                 shortcuts file yet"
            );
            println!(
                "        if Satur8 is not in your library, ask again with: satur8 steam-shortcut \
                 add --again"
            );
        }
        AddPlan::NoTool => {
            println!("satur8: this system has no {ADD_TOOL}, so Steam cannot be asked from here");
            println!(
                "        add it in Steam: Add a Game, then Add a Non-Steam Game, then pick Satur8"
            );
        }
        AddPlan::SteamNotRunning => {
            println!("satur8: Steam is not running, and {ADD_TOOL} needs it running");
            println!("        start Steam, then run: satur8 steam-shortcut add");
        }
        AddPlan::NoDesktopFile => println!(
            "satur8: no installed satur8.desktop was found; name one with --desktop-file FILE"
        ),
        AddPlan::Ask(file) => {
            let status = std::process::Command::new(ADD_TOOL)
                .arg(&file)
                .status()
                .with_context(|| format!("running {ADD_TOOL}"))?;
            if !status.success() {
                anyhow::bail!("{ADD_TOOL} {} failed ({status})", file.display());
            }
            if let Some(marker) = &marker {
                if let Some(dir) = marker.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(marker, file.to_string_lossy().as_bytes());
            }
            println!("satur8: asked Steam to add Satur8 to the library");
            // Steam writes its shortcuts file in its own time; look for a
            // while, since the artwork needs the AppID Steam picks.
            let listed = wait_for(Duration::from_secs(15), || {
                Some(steam_shortcut::find()).filter(|found| !found.apps.is_empty())
            });
            match listed {
                Some(found) => {
                    for (_, app_id) in &found.apps {
                        println!("        it is listed now (AppID {app_id})");
                    }
                    artwork(&found, false);
                }
                None => {
                    println!(
                        "        Steam has not written it to its shortcuts file yet; it may do \
                         that later or when it exits"
                    );
                    println!(
                        "        the app puts its artwork in place itself the first time it is \
                         started from the library"
                    );
                }
            }
        }
    }
    Ok(())
}

/// Put the artwork in place for every entry, or take it away.
fn artwork(found: &Found, remove: bool) {
    let entries: Vec<_> = found
        .apps
        .iter()
        .filter(|(_, app_id)| *app_id != 0)
        .collect();
    if entries.is_empty() {
        println!("satur8: Satur8 is not listed in the Steam library, so there is no entry to put artwork on");
        return;
    }
    for (user, app_id) in entries {
        if remove {
            let removed = steam_shortcut::remove_artwork(user, *app_id);
            println!("satur8: removed {removed} artwork file(s) for AppID {app_id}");
        } else {
            match steam_shortcut::place_artwork(user, *app_id) {
                Ok(0) => println!("satur8: the artwork for AppID {app_id} was already in place"),
                Ok(written) => println!(
                    "satur8: put {written} artwork file(s) in place for AppID {app_id}; Steam \
                     shows them after it restarts"
                ),
                Err(error) => println!(
                    "satur8: could not write the artwork for AppID {app_id} under {}: {error}",
                    user.display()
                ),
            }
        }
    }
}

/// Call `look` every half second until it returns something or `limit` passes.
fn wait_for<T>(limit: Duration, mut look: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = std::time::Instant::now() + limit;
    loop {
        if let Some(found) = look() {
            return Some(found);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// The file that records that Steam was asked: `$XDG_STATE_HOME/satur8/`.
fn asked_marker() -> Option<PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .map(|state| state.join("satur8/steam-shortcut-asked"))
}

/// The installed launcher entry: the per-user one first, then the system's.
fn installed_desktop_file() -> Option<PathBuf> {
    let user = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    user.into_iter()
        .chain([
            PathBuf::from("/usr/local/share"),
            PathBuf::from("/usr/share"),
        ])
        .map(|data| data.join("applications/satur8.desktop"))
        .find(|file| file.is_file())
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(apps: &[u32], broken: &[&str], users: usize) -> Found {
        Found {
            apps: apps
                .iter()
                .map(|id| (PathBuf::from("/s/userdata/55"), *id))
                .collect(),
            broken: broken
                .iter()
                .map(|name| (PathBuf::from("/s/userdata/55"), name.to_string()))
                .collect(),
            users,
        }
    }

    /// The owner's Deck on 2026-11-22: the working entry and the leftover.
    #[test]
    fn status_names_the_entry_and_the_leftover() {
        let lines = status_lines(&found(&[3367991943], &["satur8.desktop"], 1), 5, true);
        assert_eq!(
            lines,
            [
                "satur8: Satur8 is in the Steam library (AppID 3367991943)",
                "        its artwork is not in place; put it there with: satur8 steam-shortcut artwork",
                "satur8: a leftover entry named \"satur8.desktop\" is in the Steam library and cannot start anything",
                "        to remove it: select it in the Steam library, open its settings (the gear), then Manage, then \"Remove non-Steam game from your library\"",
            ]
        );
        let lines = status_lines(&found(&[3367991943], &[], 1), 0, true);
        assert_eq!(lines[1], "        its artwork is in place");
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn status_says_how_to_add_it_here() {
        let with_tool = status_lines(&found(&[], &[], 1), 0, true);
        assert_eq!(
            with_tool,
            [
                "satur8: Satur8 is not in the Steam library",
                "        add it with: satur8 steam-shortcut add",
            ]
        );
        let without = status_lines(&found(&[], &[], 2), 0, false);
        assert_eq!(
            without[1],
            "        add it in Steam: Add a Game, then Add a Non-Steam Game, then pick Satur8"
        );
        let no_steam = status_lines(&found(&[], &[], 0), 0, true);
        assert_eq!(no_steam.len(), 1);
        assert!(no_steam[0].contains("no Steam user was found"));
    }

    #[test]
    fn add_never_asks_twice() {
        let file = Some(PathBuf::from(
            "/home/deck/.local/share/applications/satur8.desktop",
        ));
        let none = found(&[], &[], 1);
        // Nothing listed, never asked, tool and Steam there: ask.
        assert_eq!(
            plan_add(&none, false, false, true, true, file.clone()),
            AddPlan::Ask(file.clone().unwrap())
        );
        // Listed: never ask, whatever else is true.
        let listed = found(&[3367991943], &[], 1);
        for again in [false, true] {
            assert_eq!(
                plan_add(&listed, true, again, true, true, file.clone()),
                AddPlan::AlreadyThere
            );
        }
        // Asked before and not listed yet: do not ask again by itself.
        assert_eq!(
            plan_add(&none, true, false, true, true, file.clone()),
            AddPlan::AskedBefore
        );
        assert_eq!(
            plan_add(&none, true, true, true, true, file.clone()),
            AddPlan::Ask(file.clone().unwrap())
        );
        // A leftover broken entry is not the app: the app is still added.
        let leftover = found(&[], &["satur8.desktop"], 1);
        assert_eq!(
            plan_add(&leftover, false, false, true, true, file.clone()),
            AddPlan::Ask(file.clone().unwrap())
        );
    }

    #[test]
    fn add_says_why_it_cannot_ask() {
        let file = Some(PathBuf::from("/x/satur8.desktop"));
        let none = found(&[], &[], 1);
        assert_eq!(
            plan_add(&found(&[], &[], 0), false, false, true, true, file.clone()),
            AddPlan::NoSteam
        );
        assert_eq!(
            plan_add(&none, false, false, false, true, file.clone()),
            AddPlan::NoTool
        );
        assert_eq!(
            plan_add(&none, false, false, true, false, file),
            AddPlan::SteamNotRunning
        );
        assert_eq!(
            plan_add(&none, false, false, true, true, None),
            AddPlan::NoDesktopFile
        );
    }
}
