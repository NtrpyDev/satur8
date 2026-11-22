//! The "Add game" list: the user's games by name. Which games it lists, which
//! of them are running, what each row says, the filter, the empty states, and
//! what picking a row does to the profiles. Everything here is pure;
//! `main.rs` reads the Steam library and what is running, and the Slint files
//! only draw what this returns.
//!
//! The list never shows process names for Steam games. A running Steam game
//! is found by its Steam AppID (from gamescope in Steam Game Mode, from the
//! process environment on a desktop) and shown under its name from the Steam
//! library, first in the list. Only a running game that is not from Steam is
//! shown by its executable, and launchers, runtimes, wrappers and shells are
//! never shown at all.

use satur8_core::steam::SteamGame;
use satur8_core::{MatchRule, Profile, Profiles};

/// The filter field is shown when the list has more rows than this, which is
/// about as many as the open list shows without scrolling.
pub const FILTER_FROM: usize = 7;

/// The AppID gamescope reports for Steam's own interface.
pub const STEAM_UI_APP: u32 = 769;

/// Shown on a desktop when no Steam install folder exists and nothing that
/// looks like a game is running.
pub const NO_STEAM: &str =
    "No Steam library was found on this computer, so start your game and open this list again.";

/// Shown on a desktop when Steam is installed but lists no games and nothing
/// that looks like a game is running.
pub const NO_GAMES: &str =
    "Steam has no games installed yet, so start your game and open this list again.";

/// Shown in Steam Game Mode when the library lists no games.
pub const NO_GAMES_GAME_MODE: &str =
    "Steam has no games installed yet. Install one in Steam, then open this list again.";

/// What was read from the Steam library when the list opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Library {
    /// No Steam install folder exists.
    NoSteam,
    /// Steam is installed; these are its games, already sorted by name.
    Games(Vec<SteamGame>),
}

impl Library {
    fn games(&self) -> &[SteamGame] {
        match self {
            Library::NoSteam => &[],
            Library::Games(games) => games,
        }
    }
}

/// A distinct running program as `satur8_core::proc::running_apps` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proc {
    /// The program's name, for example `cs2` or `Sushiamo.exe`.
    pub exe: String,
    /// The Steam AppID in the process environment, if any.
    pub app_id: Option<u32>,
    /// Started through Steam, Proton, umu or Wine.
    pub is_game: bool,
}

/// What is running right now, as far as this session can tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Running {
    /// Steam Game Mode: the AppIDs gamescope lists as having a window
    /// (`GAMESCOPE_FOCUSABLE_APPS`).
    Apps(Vec<u32>),
    /// A desktop session: the programs seen running.
    Procs(Vec<Proc>),
}

/// What picking a row adds a profile for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// An installed Steam game, matched by its AppID.
    Steam(u32),
    /// A running program that is not an installed Steam game, matched by its
    /// executable.
    Program { exe: String, app_id: Option<u32> },
}

/// One row of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub target: Target,
    pub name: String,
    /// The second line: running or not, and whether it already has a profile.
    pub detail: String,
    pub has_profile: bool,
    pub running: bool,
}

/// Everything the list shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub rows: Vec<Row>,
    /// Shown in place of the list when it is empty; empty otherwise.
    pub message: String,
    pub show_filter: bool,
}

/// What picking a row does.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    /// The game already has this profile; show it.
    Open(usize),
    /// A profile with the game's name but no Steam AppID exists; give it the
    /// game's AppID and show it, instead of adding a second one.
    Link(usize, u32),
    /// Add this profile at the end.
    Create(Profile),
}

/// The profile a Steam game already has: the enabled one that applies to it,
/// else the first disabled one with its AppID. The same choice `satur8 games`
/// makes (`crates/satur8-cli/src/games_cmd.rs`, `profile_for`), so the app and
/// the command agree on which games have a profile.
pub fn profile_for(profiles: &Profiles, app_id: u32) -> Option<usize> {
    let has_id = |profile: &Profile| profile.match_rule.steam_app_id == Some(app_id);
    profiles
        .profiles
        .iter()
        .position(|profile| profile.enabled && has_id(profile))
        .or_else(|| profiles.profiles.iter().position(has_id))
}

/// The profile a program already has: the one the background service applies
/// to this executable, else one with the name a new profile for it would
/// get. The match by executable is `Profiles::match_exe` itself, the rule the
/// service uses (the `.exe` and `.x86_64` endings and letter case do not
/// matter, and a profile that is on is found ahead of one that is off), so
/// the list never names or opens a profile other than the one applied.
pub fn profile_for_program(profiles: &Profiles, exe: &str) -> Option<usize> {
    let wanted = program_name(exe).to_lowercase();
    let applied = profiles.match_exe(exe).and_then(|found| {
        profiles
            .profiles
            .iter()
            .position(|profile| std::ptr::eq(profile, found))
    });
    applied.or_else(|| {
        profiles
            .profiles
            .iter()
            .position(|profile| profile.name.to_lowercase() == wanted)
    })
}

/// The name shown for a program and given to its profile: the executable
/// without its `.exe` or `.x86_64` ending.
pub fn program_name(exe: &str) -> &str {
    let exe = exe.trim();
    let lower = exe.to_ascii_lowercase();
    for ending in [".exe", ".x86_64"] {
        if lower.ends_with(ending) && exe.len() > ending.len() {
            return &exe[..exe.len() - ending.len()];
        }
    }
    exe
}

/// Launchers, runtimes, wrappers, shells and the session's own programs: what
/// runs around a game and is never something to add. Compared without case.
pub fn is_plumbing(exe: &str) -> bool {
    const EXACT: &[&str] = &[
        "bwrap",
        "reaper",
        "steam",
        "steamwebhelper",
        "steam-launch-wrapper",
        "steamos-session",
        "steamtinkerlaunch",
        "proton",
        "protontricks",
        "winetricks",
        "umu",
        "umu-run",
        "gamemoderun",
        "gamemoded",
        "mangohud",
        "mangoapp",
        "lutris",
        "lutris-wrapper",
        "heroic",
        "legendary",
        "gogdl",
        "nile",
        "bottles",
        "env",
        "sh",
        "bash",
        "dash",
        "zsh",
        "fish",
        "sudo",
        "xwayland",
        "xorg",
        "x",
        "kwin_wayland",
        "kwin_x11",
        "plasmashell",
        "gnome-shell",
        "hyprland",
    ];
    // A name that starts with one of these, when it is not a Windows program.
    const PREFIXES: &[&str] = &[
        "pv-",
        "pressure-vessel",
        "steam-runtime-",
        "srt-",
        "python",
        "wine",
        "gamescope",
        "systemd",
        "dbus-",
        "xdg-",
        "ld-linux",
    ];
    let name = exe.trim().to_ascii_lowercase();
    if name.is_empty() || EXACT.contains(&name.as_str()) {
        return true;
    }
    !name.ends_with(".exe") && PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

/// `+75%`, `0%` or `-25%`.
fn percent(saturation: f32) -> String {
    let percent = ((saturation - 1.0) * 100.0).round() as i32;
    if percent > 0 {
        format!("+{percent}%")
    } else {
        format!("{percent}%")
    }
}

/// The second line of a row. `shown_name` is the name the row shows, so a
/// profile under another name is named.
fn detail(shown_name: &str, running: bool, profile: Option<&Profile>) -> String {
    let profile_part = match profile {
        None => "no profile yet".to_string(),
        Some(profile) => {
            let named = if profile.name.eq_ignore_ascii_case(shown_name) {
                "has a profile".to_string()
            } else {
                format!("has a profile \u{201c}{}\u{201d}", profile.name)
            };
            let off = if profile.enabled { "" } else { ", disabled" };
            format!("{named}, {}{off}", percent(profile.saturation))
        }
    };
    let line = if running {
        format!("Running now, {profile_part}")
    } else {
        profile_part
    };
    let mut chars = line.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => line,
    }
}

/// The AppIDs of installed games that are running. In Game Mode Steam's own
/// interface and this app's own Steam shortcut are left out.
fn running_app_ids(running: &Running, own_app_id: Option<u32>) -> Vec<u32> {
    let ids: Vec<u32> = match running {
        Running::Apps(ids) => ids.clone(),
        Running::Procs(procs) => procs.iter().filter_map(|p| p.app_id).collect(),
    };
    ids.into_iter()
        .filter(|id| *id != 0 && *id != STEAM_UI_APP && Some(*id) != own_app_id)
        .collect()
}

/// Running programs that are not an installed Steam game and are worth
/// offering: never plumbing, and either a game by how it was started or, when
/// the user typed a filter, anything the filter matches. One row per name.
fn running_programs(running: &Running, library: &[SteamGame], query: &str) -> Vec<Proc> {
    let Running::Procs(procs) = running else {
        return Vec::new();
    };
    let typed = query.trim().chars().count() >= 2;
    let mut seen: Vec<String> = Vec::new();
    let mut programs = Vec::new();
    for proc in procs {
        let from_library = proc
            .app_id
            .is_some_and(|id| library.iter().any(|game| game.app_id == id));
        if from_library || is_plumbing(&proc.exe) || !(proc.is_game || typed) {
            continue;
        }
        let key = program_name(&proc.exe).to_lowercase();
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        programs.push(proc.clone());
    }
    programs.sort_by_key(|proc| program_name(&proc.exe).to_lowercase());
    programs
}

/// Every row, before the filter: running Steam games, then running programs
/// that are not from Steam, then the rest of the library, each group by name.
pub fn rows(
    library: &Library,
    profiles: &Profiles,
    running: &Running,
    own_app_id: Option<u32>,
    query: &str,
) -> Vec<Row> {
    let games = library.games();
    let running_ids = running_app_ids(running, own_app_id);
    let steam_row = |game: &SteamGame, running: bool| {
        let profile = profile_for(profiles, game.app_id).map(|i| &profiles.profiles[i]);
        Row {
            target: Target::Steam(game.app_id),
            name: game.name.clone(),
            detail: detail(&game.name, running, profile),
            has_profile: profile.is_some(),
            running,
        }
    };
    let is_running = |game: &&SteamGame| running_ids.contains(&game.app_id);
    let mut rows: Vec<Row> = games
        .iter()
        .filter(is_running)
        .map(|game| steam_row(game, true))
        .collect();
    for proc in running_programs(running, games, query) {
        let name = program_name(&proc.exe).to_string();
        let profile = profile_for_program(profiles, &proc.exe).map(|i| &profiles.profiles[i]);
        rows.push(Row {
            detail: detail(&name, true, profile),
            has_profile: profile.is_some(),
            running: true,
            name,
            target: Target::Program {
                exe: proc.exe,
                app_id: proc.app_id,
            },
        });
    }
    rows.extend(
        games
            .iter()
            .filter(|game| !is_running(game))
            .map(|game| steam_row(game, false)),
    );
    rows
}

/// The rows whose name contains the query, ignoring case, or whose AppID
/// starts with it when it is a number. An empty query keeps every row.
pub fn filter(rows: &[Row], query: &str) -> Vec<Row> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return rows.to_vec();
    }
    let numeric = query.bytes().all(|b| b.is_ascii_digit());
    rows.iter()
        .filter(|row| {
            let id_matches = match &row.target {
                Target::Steam(app_id) => numeric && app_id.to_string().starts_with(&query),
                Target::Program { .. } => false,
            };
            row.name.to_lowercase().contains(&query) || id_matches
        })
        .cloned()
        .collect()
}

/// The whole list for this library, these profiles, what is running and this
/// filter text. `game_mode` picks the wording of the empty states.
pub fn view(
    library: &Library,
    profiles: &Profiles,
    running: &Running,
    own_app_id: Option<u32>,
    query: &str,
    game_mode: bool,
) -> View {
    let all = rows(library, profiles, running, own_app_id, "");
    if all.is_empty() && query.trim().is_empty() {
        let message = match (library, game_mode) {
            (_, true) => NO_GAMES_GAME_MODE,
            (Library::NoSteam, false) => NO_STEAM,
            (Library::Games(_), false) => NO_GAMES,
        };
        return View {
            rows: Vec::new(),
            message: message.to_string(),
            show_filter: false,
        };
    }
    let rows = filter(&rows(library, profiles, running, own_app_id, query), query);
    let message = if rows.is_empty() {
        format!("No game matches \u{201c}{}\u{201d}.", query.trim())
    } else {
        String::new()
    };
    View {
        rows,
        message,
        // Kept while text is typed, so the field never vanishes under the cursor.
        show_filter: all.len() >= FILTER_FROM || !query.trim().is_empty(),
    }
}

/// Enter in the filter field picks the row when exactly one is listed.
pub fn enter_pick(rows: &[Row]) -> Option<usize> {
    (rows.len() == 1).then_some(0)
}

/// What picking this row does with these profiles. A new profile for a Steam
/// game is named after the game and matched by its AppID only; one for a
/// program is named after the executable and matched by it. Both are enabled,
/// at `saturation`.
pub fn pick(profiles: &Profiles, row: &Row, saturation: f32) -> Pick {
    let new_profile = |name: String, match_rule: MatchRule| Profile {
        name,
        enabled: true,
        saturation,
        match_rule,
        outputs: vec![],
    };
    match &row.target {
        Target::Steam(app_id) => {
            if let Some(index) = profile_for(profiles, *app_id) {
                return Pick::Open(index);
            }
            let same_name = profiles
                .profiles
                .iter()
                .position(|profile| profile.name.eq_ignore_ascii_case(&row.name));
            let name = match same_name {
                Some(index) if profiles.profiles[index].match_rule.steam_app_id.is_none() => {
                    return Pick::Link(index, *app_id);
                }
                // The name belongs to another game's profile; keep both apart.
                Some(_) => format!("{} ({})", row.name, app_id),
                None => row.name.clone(),
            };
            Pick::Create(new_profile(
                name,
                MatchRule {
                    exe: None,
                    window_class: None,
                    steam_app_id: Some(*app_id),
                },
            ))
        }
        Target::Program { exe, app_id } => {
            if let Some(index) = profile_for_program(profiles, exe) {
                return Pick::Open(index);
            }
            Pick::Create(new_profile(
                program_name(exe).to_string(),
                MatchRule {
                    exe: Some(exe.clone()),
                    window_class: None,
                    steam_app_id: *app_id,
                },
            ))
        }
    }
}

/// Carries out a [`Pick`]: returns the index of the profile to show and
/// whether the profiles changed (and so need saving).
pub fn apply(profiles: &mut Profiles, pick: Pick) -> (usize, bool) {
    match pick {
        Pick::Open(index) => (index, false),
        Pick::Link(index, app_id) => {
            profiles.profiles[index].match_rule.steam_app_id = Some(app_id);
            (index, true)
        }
        Pick::Create(profile) => {
            profiles.profiles.push(profile);
            (profiles.profiles.len() - 1, true)
        }
    }
}

/// The activity log line for a pick.
pub fn log_line(profiles: &Profiles, index: usize, changed: bool, created: bool) -> String {
    let name = profiles
        .profiles
        .get(index)
        .map(|p| p.name.as_str())
        .unwrap_or_default();
    if created {
        format!("Added {name}")
    } else if changed {
        format!("Linked {name} to its Steam game")
    } else {
        format!("Opened {name}")
    }
}

/// The name a profile's row shows. The file is not changed.
///
/// Before the app named a profile after its game in the Steam library, it
/// named it after the program: `eldenring` for `eldenring.exe`. A profile
/// still named exactly like its program (ignoring case, `.exe` and
/// `.x86_64`) that carries a Steam AppID shows that game's library name,
/// when `library_name` knows it. Any other name is one somebody chose, and
/// is shown as it is.
pub fn display_name(profile: &Profile, library_name: impl Fn(u32) -> Option<String>) -> String {
    let stem = |text: &str| {
        let text = text.trim().to_ascii_lowercase();
        text.trim_end_matches(".exe")
            .trim_end_matches(".x86_64")
            .to_string()
    };
    let named_after_its_program = profile
        .match_rule
        .exe
        .as_deref()
        .is_some_and(|exe| stem(exe) == stem(&profile.name));
    profile
        .match_rule
        .steam_app_id
        .filter(|_| named_after_its_program)
        .and_then(library_name)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| profile.name.clone())
}

/// The small line under a game card's name: the executable it matches, else
/// the Steam AppID, else the profile name.
pub fn card_subtitle(profile: &Profile) -> String {
    match (&profile.match_rule.exe, profile.match_rule.steam_app_id) {
        (Some(exe), _) => exe.clone(),
        (None, Some(app_id)) => format!("Steam app {app_id}"),
        (None, None) => profile.name.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn game(app_id: u32, name: &str) -> SteamGame {
        SteamGame {
            app_id,
            name: name.to_string(),
            library: PathBuf::from("/home/deck/.local/share/Steam"),
        }
    }

    fn profile(name: &str, saturation: f32, enabled: bool, app_id: Option<u32>) -> Profile {
        Profile {
            name: name.to_string(),
            enabled,
            saturation,
            match_rule: MatchRule {
                exe: None,
                window_class: None,
                steam_app_id: app_id,
            },
            outputs: vec![],
        }
    }

    fn library() -> Vec<SteamGame> {
        vec![
            game(2379780, "Balatro"),
            game(4421010, "Bills Must Be Paid"),
            game(1623730, "Palworld"),
            game(2868840, "Slay the Spire 2"),
            game(413150, "Stardew Valley"),
            game(4555040, "Sushiamo"),
        ]
    }

    /// The ten games `satur8 games` listed on the owner's Steam Deck on
    /// 2026-11-22.
    fn deck_library() -> Library {
        Library::Games(vec![
            game(2379780, "Balatro"),
            game(1086940, "Baldur's Gate 3"),
            game(4421010, "Bills Must Be Paid"),
            game(1313140, "Cult of the Lamb"),
            game(1245620, "ELDEN RING"),
            game(1623730, "Palworld"),
            game(2868840, "Slay the Spire 2"),
            game(413150, "Stardew Valley"),
            game(4555040, "Sushiamo"),
            game(105600, "Terraria"),
        ])
    }

    fn profiles(list: Vec<Profile>) -> Profiles {
        Profiles {
            profiles: list,
            ..Profiles::default()
        }
    }

    fn proc(exe: &str, app_id: Option<u32>, is_game: bool) -> Proc {
        Proc {
            exe: exe.to_string(),
            app_id,
            is_game,
        }
    }

    fn nothing_running() -> Running {
        Running::Procs(Vec::new())
    }

    fn all_rows(games: Vec<SteamGame>, profiles: &Profiles) -> Vec<Row> {
        rows(
            &Library::Games(games),
            profiles,
            &nothing_running(),
            None,
            "",
        )
    }

    fn names(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|row| row.name.as_str()).collect()
    }

    /// What the owner saw in "Add running game" on his Steam Deck with
    /// Sushiamo running (2026-11-22): the game's executable among Steam's own
    /// runtime processes. All of them carry the game's AppID or none.
    fn deck_processes_with_sushiamo() -> Vec<Proc> {
        vec![
            proc("Sushiamo.exe", Some(4555040), true),
            proc("pv-adverb", Some(4555040), true),
            proc("python3", Some(4555040), true),
            proc("python3.13", Some(4555040), true),
            proc("steam-runtime-launch-client", Some(4555040), true),
            proc("steam-runtime-launcher-service", Some(4555040), true),
            proc("steam-runtime-supervisor", None, false),
            proc("srt-bwrap", Some(4555040), true),
            proc("reaper", Some(4555040), true),
            proc("steam-launch-wrapper", Some(4555040), true),
            proc("proton", Some(4555040), true),
            proc("wineserver", Some(4555040), true),
            proc("gamescope-wl", None, false),
            proc("gamescopereaper", None, false),
            proc("steam", None, false),
            proc("steamwebhelper", None, false),
            proc("bash", None, false),
            // This app's own container, started from its Steam shortcut.
            proc("pv-adverb", Some(3367991943), true),
        ]
    }

    #[test]
    fn what_the_owner_saw_becomes_one_running_row_named_sushiamo() {
        let running = Running::Procs(deck_processes_with_sushiamo());
        let rows = rows(
            &deck_library(),
            &Profiles::default(),
            &running,
            Some(3367991943),
            "",
        );
        let running_rows: Vec<&Row> = rows.iter().filter(|row| row.running).collect();
        assert_eq!(running_rows.len(), 1);
        assert_eq!(running_rows[0].name, "Sushiamo");
        assert_eq!(running_rows[0].target, Target::Steam(4555040));
        assert_eq!(running_rows[0].detail, "Running now, no profile yet");
        // It is first, and the other nine installed games follow by name.
        assert_eq!(rows[0].name, "Sushiamo");
        assert_eq!(rows.len(), 10);
        assert!(rows
            .iter()
            .all(|row| matches!(row.target, Target::Steam(_))));
    }

    #[test]
    fn without_a_readable_library_the_game_is_still_the_only_row() {
        let running = Running::Procs(deck_processes_with_sushiamo());
        let rows = rows(
            &Library::NoSteam,
            &Profiles::default(),
            &running,
            Some(3367991943),
            "",
        );
        assert_eq!(names(&rows), vec!["Sushiamo"]);
        assert_eq!(
            rows[0].target,
            Target::Program {
                exe: "Sushiamo.exe".into(),
                app_id: Some(4555040),
            }
        );
    }

    /// Game Mode on the owner's Deck with Sushiamo and ELDEN RING running:
    /// gamescope lists Steam (769), this app's shortcut and the two games.
    #[test]
    fn game_mode_lists_running_games_first_by_their_steam_names() {
        let running = Running::Apps(vec![769, 3367991943, 4555040, 1245620]);
        let rows = rows(
            &deck_library(),
            &Profiles::default(),
            &running,
            Some(3367991943),
            "",
        );
        assert_eq!(
            names(&rows),
            vec![
                "ELDEN RING",
                "Sushiamo",
                "Balatro",
                "Baldur's Gate 3",
                "Bills Must Be Paid",
                "Cult of the Lamb",
                "Palworld",
                "Slay the Spire 2",
                "Stardew Valley",
                "Terraria",
            ]
        );
        assert_eq!(
            rows.iter().map(|row| row.running).collect::<Vec<_>>(),
            vec![true, true, false, false, false, false, false, false, false, false]
        );
        assert_eq!(rows[0].detail, "Running now, no profile yet");
        assert_eq!(rows[2].detail, "No profile yet");
    }

    #[test]
    fn game_mode_ignores_steam_itself_and_ids_that_are_not_installed_games() {
        // Only Steam and this app have a window; an unknown shortcut ID too.
        let running = Running::Apps(vec![769, 3367991943, 2999999999, 0]);
        let rows = rows(
            &deck_library(),
            &Profiles::default(),
            &running,
            Some(3367991943),
            "",
        );
        assert_eq!(rows.len(), 10);
        assert!(rows.iter().all(|row| !row.running));
    }

    #[test]
    fn plumbing_is_never_offered() {
        for name in [
            "pv-adverb",
            "PV-Adverb",
            "pv-bwrap",
            "pressure-vessel-wrap",
            "python",
            "python3",
            "python3.13",
            "steam-runtime-launch-client",
            "steam-runtime-launcher-service",
            "steam-runtime-supervisor",
            "srt-bwrap",
            "srt-logger",
            "reaper",
            "steam",
            "steamwebhelper",
            "steam-launch-wrapper",
            "proton",
            "wineserver",
            "wine64-preloader",
            "gamescope",
            "gamescope-wl",
            "gamescopereaper",
            "mangoapp",
            "umu-run",
            "bash",
            "sh",
            "env",
            "systemd-run",
            "dbus-daemon",
            "xdg-dbus-proxy",
            "",
        ] {
            assert!(is_plumbing(name), "{name}");
        }
        for name in [
            "Sushiamo.exe",
            "eldenring.exe",
            "cs2",
            "EscapeFromTarkov.exe",
            "hl2_linux",
            "factorio",
            // A Windows game may start with a word a wrapper starts with.
            "Winelands.exe",
            "PythonQuest.exe",
        ] {
            assert!(!is_plumbing(name), "{name}");
        }
    }

    #[test]
    fn a_running_game_that_is_not_from_steam_is_shown_by_its_executable() {
        let running = Running::Procs(vec![
            proc("EscapeFromTarkov.exe", None, true),
            proc("umu-run", None, true),
            proc("bash", None, false),
            proc("firefox", None, false),
        ]);
        let rows = rows(
            &Library::Games(library()),
            &Profiles::default(),
            &running,
            None,
            "",
        );
        assert_eq!(rows[0].name, "EscapeFromTarkov");
        assert_eq!(rows[0].detail, "Running now, no profile yet");
        assert!(rows[0].running);
        assert_eq!(
            rows[0].target,
            Target::Program {
                exe: "EscapeFromTarkov.exe".into(),
                app_id: None,
            }
        );
        // The browser is not a game and nothing was typed: not listed.
        assert_eq!(rows.len(), 1 + library().len());
    }

    #[test]
    fn another_running_program_is_listed_only_when_its_name_is_typed() {
        let running = Running::Procs(vec![
            proc("factorio", None, false),
            proc("bash", None, false),
        ]);
        let lib = Library::Games(library());
        let none = view(&lib, &Profiles::default(), &running, None, "", false);
        assert!(names(&none.rows).iter().all(|name| *name != "factorio"));
        let typed = view(&lib, &Profiles::default(), &running, None, "fact", false);
        assert_eq!(names(&typed.rows), vec!["factorio"]);
        // Typing never brings plumbing in.
        let shell = view(&lib, &Profiles::default(), &running, None, "bash", false);
        assert!(shell.rows.is_empty());
    }

    #[test]
    fn one_row_per_program_however_many_processes_it_has() {
        let running = Running::Procs(vec![
            proc("Game.exe", None, true),
            proc("game.exe", None, true),
            proc("Game.x86_64", None, true),
        ]);
        let rows = rows(&Library::NoSteam, &Profiles::default(), &running, None, "");
        assert_eq!(names(&rows), vec!["Game"]);
    }

    #[test]
    fn rows_say_which_games_have_a_profile() {
        let profiles = profiles(vec![
            profile("sushiamo", 1.75, true, Some(4555040)),
            profile("Balatro", 1.4, false, Some(2379780)),
            profile("Palworld", 0.75, true, Some(1623730)),
        ]);
        let rows = all_rows(library(), &profiles);
        let lines: Vec<(&str, &str, bool)> = rows
            .iter()
            .map(|r| (r.name.as_str(), r.detail.as_str(), r.has_profile))
            .collect();
        assert_eq!(
            lines,
            vec![
                ("Balatro", "Has a profile, +40%, disabled", true),
                ("Bills Must Be Paid", "No profile yet", false),
                ("Palworld", "Has a profile, -25%", true),
                ("Slay the Spire 2", "No profile yet", false),
                ("Stardew Valley", "No profile yet", false),
                ("Sushiamo", "Has a profile, +75%", true),
            ]
        );
    }

    #[test]
    fn a_running_game_with_a_profile_says_both() {
        let profiles = profiles(vec![profile("Sushiamo", 1.75, true, Some(4555040))]);
        let rows = rows(
            &Library::Games(library()),
            &profiles,
            &Running::Apps(vec![4555040]),
            None,
            "",
        );
        assert_eq!(rows[0].detail, "Running now, has a profile, +75%");
    }

    #[test]
    fn a_profile_with_another_name_is_named_in_the_row() {
        let profiles = profiles(vec![profile("Spire", 1.0, true, Some(2868840))]);
        let rows = all_rows(vec![game(2868840, "Slay the Spire 2")], &profiles);
        assert_eq!(rows[0].detail, "Has a profile \u{201c}Spire\u{201d}, 0%");
    }

    #[test]
    fn an_enabled_profile_is_shown_ahead_of_a_disabled_one() {
        let profiles = profiles(vec![
            profile("old", 1.2, false, Some(4555040)),
            profile("new", 1.75, true, Some(4555040)),
        ]);
        assert_eq!(profile_for(&profiles, 4555040), Some(1));
        let only_disabled = Profiles {
            profiles: vec![profile("old", 1.2, false, Some(4555040))],
            ..Profiles::default()
        };
        assert_eq!(profile_for(&only_disabled, 4555040), Some(0));
        assert_eq!(profile_for(&only_disabled, 1), None);
    }

    #[test]
    fn a_profile_without_an_app_id_does_not_mark_the_game() {
        // Matched by executable only: the row cannot know it is this game.
        let mut by_exe = profile("Stardew Valley", 1.4, true, None);
        by_exe.match_rule.exe = Some("StardewValley".into());
        let rows = all_rows(library(), &profiles(vec![by_exe]));
        assert!(rows.iter().all(|r| !r.has_profile));
    }

    #[test]
    fn filter_matches_part_of_a_name_ignoring_case_and_spaces_around() {
        let rows = all_rows(library(), &Profiles::default());
        let names = |query: &str| -> Vec<String> {
            filter(&rows, query).into_iter().map(|r| r.name).collect()
        };
        assert_eq!(names("  sTA "), vec!["Stardew Valley"]);
        assert_eq!(names("s"), {
            let mut all: Vec<String> = library().into_iter().map(|g| g.name).collect();
            all.retain(|n| n.to_lowercase().contains('s'));
            all
        });
        assert_eq!(names("").len(), 6);
        assert_eq!(names("   ").len(), 6);
        assert!(names("zelda").is_empty());
    }

    #[test]
    fn filter_matches_the_start_of_an_app_id() {
        let rows = all_rows(library(), &Profiles::default());
        let ids: Vec<Target> = filter(&rows, "4555")
            .into_iter()
            .map(|r| r.target)
            .collect();
        assert_eq!(ids, vec![Target::Steam(4555040)]);
        // Only the start: "555" is inside 4555040 but starts no AppID.
        assert!(filter(&rows, "555").is_empty());
    }

    #[test]
    fn nothing_to_list_is_said_in_one_sentence() {
        let nothing = nothing_running();
        let none = view(
            &Library::NoSteam,
            &Profiles::default(),
            &nothing,
            None,
            "",
            false,
        );
        assert!(none.rows.is_empty());
        assert!(!none.show_filter);
        assert_eq!(none.message, NO_STEAM);
        let empty = view(
            &Library::Games(vec![]),
            &Profiles::default(),
            &nothing,
            None,
            "",
            false,
        );
        assert_eq!(empty.message, NO_GAMES);
        let game_mode = view(
            &Library::Games(vec![]),
            &Profiles::default(),
            &Running::Apps(vec![769]),
            None,
            "",
            true,
        );
        assert_eq!(game_mode.message, NO_GAMES_GAME_MODE);
        // Each says what is the case and what to do about it.
        for message in [NO_STEAM, NO_GAMES, NO_GAMES_GAME_MODE] {
            assert!(message.ends_with("open this list again."), "{message}");
        }
    }

    #[test]
    fn filter_shows_only_for_a_long_list_or_while_typing() {
        let nothing = nothing_running();
        let short = Library::Games(library());
        let shown = |lib: &Library, query: &str| {
            view(lib, &Profiles::default(), &nothing, None, query, false).show_filter
        };
        assert!(!shown(&short, ""));
        assert!(shown(&short, "ba"));
        let mut seven = library();
        seven.push(game(570, "Dota 2"));
        assert_eq!(seven.len(), FILTER_FROM);
        assert!(shown(&Library::Games(seven), ""));
    }

    #[test]
    fn nothing_matching_the_filter_says_so() {
        let nothing = nothing_running();
        let lib = Library::Games(library());
        let v = view(&lib, &Profiles::default(), &nothing, None, " zelda ", false);
        assert!(v.rows.is_empty());
        assert!(v.show_filter);
        assert_eq!(v.message, "No game matches \u{201c}zelda\u{201d}.");
        let some = view(&lib, &Profiles::default(), &nothing, None, "pal", false);
        assert_eq!(some.message, "");
        assert_eq!(some.rows.len(), 1);
    }

    #[test]
    fn enter_picks_only_a_single_listed_game() {
        let nothing = nothing_running();
        let lib = Library::Games(library());
        let of = |query: &str| view(&lib, &Profiles::default(), &nothing, None, query, false);
        assert_eq!(enter_pick(&of("pal").rows), Some(0));
        assert_eq!(enter_pick(&of("s").rows), None);
        assert_eq!(enter_pick(&of("zelda").rows), None);
    }

    fn steam_row(app_id: u32, name: &str) -> Row {
        Row {
            target: Target::Steam(app_id),
            name: name.to_string(),
            detail: String::new(),
            has_profile: false,
            running: false,
        }
    }

    #[test]
    fn picking_a_new_game_adds_a_profile_matched_by_app_id() {
        let mut profiles = profiles(vec![profile("Other", 1.2, true, Some(1))]);
        let spire = steam_row(2868840, "Slay the Spire 2");
        let choice = pick(&profiles, &spire, 1.4);
        assert_eq!(
            choice,
            Pick::Create(Profile {
                name: "Slay the Spire 2".into(),
                enabled: true,
                saturation: 1.4,
                match_rule: MatchRule {
                    exe: None,
                    window_class: None,
                    steam_app_id: Some(2868840),
                },
                outputs: vec![],
            })
        );
        assert_eq!(apply(&mut profiles, choice), (1, true));
        assert_eq!(profiles.profiles.len(), 2);
        assert_eq!(log_line(&profiles, 1, true, true), "Added Slay the Spire 2");
        // Picking it again opens it instead of adding a second one.
        assert_eq!(pick(&profiles, &spire, 1.4), Pick::Open(1));
    }

    /// The owner's Deck saved ELDEN RING as `eldenring` from its executable.
    /// From the list it is the game's name from the Steam library.
    #[test]
    fn a_running_steam_game_is_added_under_its_steam_name() {
        let running = Running::Apps(vec![769, 1245620]);
        let rows = rows(&deck_library(), &Profiles::default(), &running, None, "");
        assert_eq!(rows[0].name, "ELDEN RING");
        match pick(&Profiles::default(), &rows[0], 1.4) {
            Pick::Create(profile) => {
                assert_eq!(profile.name, "ELDEN RING");
                assert_eq!(profile.match_rule.steam_app_id, Some(1245620));
                assert_eq!(profile.match_rule.exe, None);
            }
            other => panic!("expected a new profile, got {other:?}"),
        }
    }

    #[test]
    fn picking_a_game_with_a_profile_opens_it_even_when_disabled() {
        let mut profiles = profiles(vec![
            profile("x", 1.0, true, Some(1)),
            profile("sushiamo", 1.75, false, Some(4555040)),
        ]);
        let choice = pick(&profiles, &steam_row(4555040, "Sushiamo"), 1.4);
        assert_eq!(choice, Pick::Open(1));
        let before = profiles.clone();
        assert_eq!(apply(&mut profiles, choice), (1, false));
        assert_eq!(profiles, before);
        assert_eq!(log_line(&profiles, 1, false, false), "Opened sushiamo");
    }

    #[test]
    fn a_same_named_profile_without_an_app_id_gets_the_app_id() {
        let mut by_exe = profile("balatro", 1.3, true, None);
        by_exe.match_rule.exe = Some("Balatro.exe".into());
        let mut profiles = profiles(vec![by_exe]);
        let choice = pick(&profiles, &steam_row(2379780, "Balatro"), 1.4);
        assert_eq!(choice, Pick::Link(0, 2379780));
        assert_eq!(apply(&mut profiles, choice), (0, true));
        assert_eq!(profiles.profiles.len(), 1);
        let rule = &profiles.profiles[0].match_rule;
        assert_eq!(rule.exe.as_deref(), Some("Balatro.exe"));
        assert_eq!(rule.steam_app_id, Some(2379780));
        assert_eq!(profiles.profiles[0].saturation, 1.3);
        assert_eq!(
            log_line(&profiles, 0, true, false),
            "Linked balatro to its Steam game"
        );
    }

    #[test]
    fn a_name_used_by_another_games_profile_gets_the_app_id_added() {
        let profiles = profiles(vec![profile("Palworld", 1.2, true, Some(99))]);
        match pick(&profiles, &steam_row(1623730, "Palworld"), 1.4) {
            Pick::Create(p) => {
                assert_eq!(p.name, "Palworld (1623730)");
                assert_eq!(p.match_rule.steam_app_id, Some(1623730));
            }
            other => panic!("expected a new profile, got {other:?}"),
        }
    }

    #[test]
    fn picking_a_program_adds_a_profile_matched_by_its_executable() {
        let mut profiles = Profiles::default();
        let row = Row {
            target: Target::Program {
                exe: "EscapeFromTarkov.exe".into(),
                app_id: None,
            },
            name: "EscapeFromTarkov".into(),
            detail: String::new(),
            has_profile: false,
            running: true,
        };
        let choice = pick(&profiles, &row, 0.5);
        assert_eq!(apply(&mut profiles, choice), (0, true));
        let added = &profiles.profiles[0];
        assert_eq!(added.name, "EscapeFromTarkov");
        assert_eq!(added.saturation, 0.5);
        assert_eq!(
            added.match_rule.exe.as_deref(),
            Some("EscapeFromTarkov.exe")
        );
        // Picking it again opens it; its saturation is left alone.
        profiles.profiles[0].saturation = 1.9;
        assert_eq!(pick(&profiles, &row, 0.5), Pick::Open(0));
        assert_eq!(profile_for_program(&profiles, "escapefromtarkov"), Some(0));
    }

    /// Two profiles for one program, the earlier one off: the background
    /// service applies the one that is on, so that is the one the list must
    /// name and open.
    #[test]
    fn a_programs_profile_is_the_one_the_service_applies() {
        let for_exe = |name: &str, exe: &str, enabled: bool, saturation: f32| Profile {
            name: name.into(),
            enabled,
            saturation,
            match_rule: MatchRule {
                exe: Some(exe.into()),
                window_class: None,
                steam_app_id: None,
            },
            outputs: vec![],
        };
        let mut profiles = Profiles {
            profiles: vec![
                for_exe("old tarkov", "EscapeFromTarkov.exe", false, 1.2),
                for_exe("tarkov", "escapefromtarkov", true, 1.6),
            ],
            ..Profiles::default()
        };
        let applied = profiles
            .match_exe("EscapeFromTarkov.exe")
            .unwrap()
            .name
            .clone();
        assert_eq!(applied, "tarkov");
        assert_eq!(
            profile_for_program(&profiles, "EscapeFromTarkov.exe"),
            Some(1)
        );

        // The row names that profile, and picking it opens that profile.
        let running = Running::Procs(vec![Proc {
            exe: "EscapeFromTarkov.exe".into(),
            app_id: None,
            is_game: true,
        }]);
        let listed = rows(&Library::NoSteam, &profiles, &running, None, "");
        assert_eq!(listed.len(), 1);
        assert!(listed[0].detail.contains("tarkov") && !listed[0].detail.contains("old tarkov"));
        assert!(!listed[0].detail.contains("disabled"));
        assert_eq!(pick(&profiles, &listed[0], 1.4), Pick::Open(1));

        // With both off, the first is the one the service uses, and so here.
        profiles.profiles[1].enabled = false;
        assert_eq!(profile_for_program(&profiles, "escapefromtarkov"), Some(0));
        // With only the earlier one on, it is that one.
        profiles.profiles[0].enabled = true;
        assert_eq!(profile_for_program(&profiles, "escapefromtarkov"), Some(0));
    }

    #[test]
    fn card_subtitle_prefers_exe_then_app_id_then_name() {
        let mut p = profile("Sushiamo", 1.75, true, Some(4555040));
        assert_eq!(card_subtitle(&p), "Steam app 4555040");
        p.match_rule.exe = Some("Sushiamo.exe".into());
        assert_eq!(card_subtitle(&p), "Sushiamo.exe");
        assert_eq!(card_subtitle(&profile("CS2", 1.6, true, None)), "CS2");
    }

    /// The profile the v0.5.0 app left on the owner's Deck.
    fn old_elden_ring() -> Profile {
        Profile {
            name: "eldenring".into(),
            enabled: true,
            saturation: 1.4,
            match_rule: MatchRule {
                exe: Some("eldenring.exe".into()),
                window_class: None,
                steam_app_id: Some(1245620),
            },
            outputs: vec![],
        }
    }

    fn steam_name(app_id: u32) -> Option<String> {
        (app_id == 1245620).then(|| "ELDEN RING".to_string())
    }

    #[test]
    fn a_profile_named_after_its_program_shows_the_steam_library_name() {
        assert_eq!(display_name(&old_elden_ring(), steam_name), "ELDEN RING");
        // Case and the Linux suffix do not matter.
        let mut native = old_elden_ring();
        native.name = "EldenRing".into();
        native.match_rule.exe = Some("eldenring.x86_64".into());
        assert_eq!(display_name(&native, steam_name), "ELDEN RING");
    }

    #[test]
    fn any_other_profile_keeps_the_name_it_has() {
        // A name somebody chose.
        let mut chosen = old_elden_ring();
        chosen.name = "Souls, vivid".into();
        assert_eq!(display_name(&chosen, steam_name), "Souls, vivid");
        // No Steam AppID: nothing says which game it is.
        let mut no_id = old_elden_ring();
        no_id.match_rule.steam_app_id = None;
        assert_eq!(display_name(&no_id, steam_name), "eldenring");
        // No program to compare the name with.
        let mut no_exe = old_elden_ring();
        no_exe.match_rule.exe = None;
        assert_eq!(display_name(&no_exe, steam_name), "eldenring");
        // A game that is not installed, so the library has no name for it.
        assert_eq!(display_name(&old_elden_ring(), |_| None), "eldenring");
        assert_eq!(
            display_name(&old_elden_ring(), |_| Some("  ".into())),
            "eldenring"
        );
        // A profile already named from the library.
        let mut named = old_elden_ring();
        named.name = "ELDEN RING".into();
        assert_eq!(
            display_name(&named, |_| Some("something else".into())),
            "ELDEN RING"
        );
    }
}
