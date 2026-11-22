//! The "Add Steam game" picker on the Main page: which installed Steam games
//! it lists, what each row says, the filter, the empty states, and what
//! picking a game does to the profiles. Everything here is pure; `main.rs`
//! reads the Steam library and the Slint file only draws what this returns.

use satur8_core::steam::SteamGame;
use satur8_core::{MatchRule, Profile, Profiles};

/// The filter field is shown when the library has more games than this,
/// which is about as many rows as the open picker shows without scrolling.
pub const FILTER_FROM: usize = 7;

/// Shown when no Steam install folder exists.
pub const NO_STEAM: &str = "No Steam library was found on this computer, so start your game and use \u{201c}Add running game\u{201d} instead.";

/// Shown when Steam is installed but lists no games.
pub const NO_GAMES: &str = "Steam has no games installed yet, so start your game and use \u{201c}Add running game\u{201d} instead.";

/// What was read from the Steam library when the picker opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Library {
    /// No Steam install folder exists.
    NoSteam,
    /// Steam is installed; these are its games, already sorted by name.
    Games(Vec<SteamGame>),
}

/// One row of the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub app_id: u32,
    pub name: String,
    /// The second line: whether the game already has a profile.
    pub detail: String,
    pub has_profile: bool,
}

/// Everything the picker shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub rows: Vec<Row>,
    /// Shown in place of the list when it is empty; empty otherwise.
    pub message: String,
    pub show_filter: bool,
}

/// What picking a game does.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    /// The game already has this profile; show it.
    Open(usize),
    /// A profile with the game's name but no Steam AppID exists; give it the
    /// game's AppID and show it, instead of adding a second one.
    Link(usize),
    /// Add this profile at the end.
    Create(Profile),
}

/// The profile a game already has: the enabled one that applies to it, else
/// the first disabled one with its AppID. The same choice `satur8 games` makes
/// (`crates/satur8-cli/src/games_cmd.rs`, `profile_for`), so the app and the
/// command agree on which games have a profile.
pub fn profile_for(profiles: &Profiles, app_id: u32) -> Option<usize> {
    let has_id = |profile: &Profile| profile.match_rule.steam_app_id == Some(app_id);
    profiles
        .profiles
        .iter()
        .position(|profile| profile.enabled && has_id(profile))
        .or_else(|| profiles.profiles.iter().position(has_id))
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

/// The second line of a row.
fn detail(game: &SteamGame, profile: Option<&Profile>) -> String {
    let Some(profile) = profile else {
        return "No profile yet".to_string();
    };
    let named = if profile.name.eq_ignore_ascii_case(&game.name) {
        "Has a profile".to_string()
    } else {
        format!("Has a profile \u{201c}{}\u{201d}", profile.name)
    };
    let off = if profile.enabled { "" } else { ", disabled" };
    format!("{named}, {}{off}", percent(profile.saturation))
}

/// One row per installed game, in the library's order.
pub fn rows(games: &[SteamGame], profiles: &Profiles) -> Vec<Row> {
    games
        .iter()
        .map(|game| {
            let profile = profile_for(profiles, game.app_id).map(|i| &profiles.profiles[i]);
            Row {
                app_id: game.app_id,
                name: game.name.clone(),
                detail: detail(game, profile),
                has_profile: profile.is_some(),
            }
        })
        .collect()
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
            row.name.to_lowercase().contains(&query)
                || (numeric && row.app_id.to_string().starts_with(&query))
        })
        .cloned()
        .collect()
}

/// The whole picker for this library, these profiles and this filter text.
pub fn view(library: &Library, profiles: &Profiles, query: &str) -> View {
    let games = match library {
        Library::NoSteam => {
            return View {
                rows: Vec::new(),
                message: NO_STEAM.to_string(),
                show_filter: false,
            }
        }
        Library::Games(games) => games,
    };
    if games.is_empty() {
        return View {
            rows: Vec::new(),
            message: NO_GAMES.to_string(),
            show_filter: false,
        };
    }
    let rows = filter(&rows(games, profiles), query);
    let message = if rows.is_empty() {
        format!(
            "No installed game matches \u{201c}{}\u{201d}.",
            query.trim()
        )
    } else {
        String::new()
    };
    View {
        rows,
        message,
        // Kept while text is typed, so the field never vanishes under the cursor.
        show_filter: games.len() >= FILTER_FROM || !query.trim().is_empty(),
    }
}

/// Enter in the filter field picks the game when exactly one is listed.
pub fn enter_pick(view: &View) -> Option<u32> {
    match view.rows.as_slice() {
        [only] => Some(only.app_id),
        _ => None,
    }
}

/// What picking `game` does with these profiles. A new profile is named after
/// the game, matched by its AppID only, enabled, at `saturation`.
pub fn pick(profiles: &Profiles, game: &SteamGame, saturation: f32) -> Pick {
    if let Some(index) = profile_for(profiles, game.app_id) {
        return Pick::Open(index);
    }
    let same_name = profiles
        .profiles
        .iter()
        .position(|profile| profile.name.eq_ignore_ascii_case(&game.name));
    let name = match same_name {
        Some(index) if profiles.profiles[index].match_rule.steam_app_id.is_none() => {
            return Pick::Link(index);
        }
        // The name belongs to another game's profile; keep both apart.
        Some(_) => format!("{} ({})", game.name, game.app_id),
        None => game.name.clone(),
    };
    Pick::Create(Profile {
        name,
        enabled: true,
        saturation,
        match_rule: MatchRule {
            exe: None,
            window_class: None,
            steam_app_id: Some(game.app_id),
        },
        outputs: vec![],
    })
}

/// Carries out a [`Pick`]: returns the index of the profile to show and
/// whether the profiles changed (and so need saving).
pub fn apply(profiles: &mut Profiles, game: &SteamGame, pick: Pick) -> (usize, bool) {
    match pick {
        Pick::Open(index) => (index, false),
        Pick::Link(index) => {
            profiles.profiles[index].match_rule.steam_app_id = Some(game.app_id);
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

    fn profiles(list: Vec<Profile>) -> Profiles {
        Profiles {
            profiles: list,
            ..Profiles::default()
        }
    }

    #[test]
    fn rows_say_which_games_have_a_profile() {
        let profiles = profiles(vec![
            profile("sushiamo", 1.75, true, Some(4555040)),
            profile("Balatro", 1.4, false, Some(2379780)),
            profile("Palworld", 0.75, true, Some(1623730)),
        ]);
        let rows = rows(&library(), &profiles);
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
    fn a_profile_with_another_name_is_named_in_the_row() {
        let profiles = profiles(vec![profile("Spire", 1.0, true, Some(2868840))]);
        let rows = rows(&[game(2868840, "Slay the Spire 2")], &profiles);
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
        let rows = rows(&library(), &profiles(vec![by_exe]));
        assert!(rows.iter().all(|r| !r.has_profile));
    }

    #[test]
    fn filter_matches_part_of_a_name_ignoring_case_and_spaces_around() {
        let rows = rows(&library(), &Profiles::default());
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
        let rows = rows(&library(), &Profiles::default());
        let ids: Vec<u32> = filter(&rows, "4555")
            .into_iter()
            .map(|r| r.app_id)
            .collect();
        assert_eq!(ids, vec![4555040]);
        // Only the start: "555" is inside 4555040 but starts no AppID.
        assert!(filter(&rows, "555").is_empty());
    }

    #[test]
    fn no_steam_and_no_games_are_said_in_one_sentence() {
        let none = view(&Library::NoSteam, &Profiles::default(), "");
        assert!(none.rows.is_empty());
        assert!(!none.show_filter);
        assert_eq!(none.message, NO_STEAM);
        let empty = view(&Library::Games(vec![]), &Profiles::default(), "");
        assert!(empty.rows.is_empty());
        assert!(!empty.show_filter);
        assert_eq!(empty.message, NO_GAMES);
        for message in [NO_STEAM, NO_GAMES] {
            assert_eq!(message.matches('.').count(), 1, "{message}");
            assert!(message.ends_with('.'));
            assert!(message.contains("Add running game"));
        }
    }

    #[test]
    fn filter_shows_only_for_a_long_list_or_while_typing() {
        let short = Library::Games(library());
        assert!(!view(&short, &Profiles::default(), "").show_filter);
        assert!(view(&short, &Profiles::default(), "ba").show_filter);
        let mut seven = library();
        seven.push(game(570, "Dota 2"));
        assert_eq!(seven.len(), FILTER_FROM);
        assert!(view(&Library::Games(seven), &Profiles::default(), "").show_filter);
    }

    #[test]
    fn nothing_matching_the_filter_says_so() {
        let v = view(&Library::Games(library()), &Profiles::default(), " zelda ");
        assert!(v.rows.is_empty());
        assert!(v.show_filter);
        assert_eq!(
            v.message,
            "No installed game matches \u{201c}zelda\u{201d}."
        );
        let some = view(&Library::Games(library()), &Profiles::default(), "pal");
        assert_eq!(some.message, "");
        assert_eq!(some.rows.len(), 1);
    }

    #[test]
    fn enter_picks_only_a_single_listed_game() {
        let lib = Library::Games(library());
        let one = view(&lib, &Profiles::default(), "pal");
        assert_eq!(enter_pick(&one), Some(1623730));
        assert_eq!(enter_pick(&view(&lib, &Profiles::default(), "s")), None);
        assert_eq!(enter_pick(&view(&lib, &Profiles::default(), "zelda")), None);
    }

    #[test]
    fn picking_a_new_game_adds_a_profile_matched_by_app_id() {
        let mut profiles = profiles(vec![profile("Other", 1.2, true, Some(1))]);
        let spire = game(2868840, "Slay the Spire 2");
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
        assert_eq!(apply(&mut profiles, &spire, choice), (1, true));
        assert_eq!(profiles.profiles.len(), 2);
        assert_eq!(log_line(&profiles, 1, true, true), "Added Slay the Spire 2");
        // Picking it again opens it instead of adding a second one.
        assert_eq!(pick(&profiles, &spire, 1.4), Pick::Open(1));
    }

    #[test]
    fn picking_a_game_with_a_profile_opens_it_even_when_disabled() {
        let mut profiles = profiles(vec![
            profile("x", 1.0, true, Some(1)),
            profile("sushiamo", 1.75, false, Some(4555040)),
        ]);
        let sushiamo = game(4555040, "Sushiamo");
        let choice = pick(&profiles, &sushiamo, 1.4);
        assert_eq!(choice, Pick::Open(1));
        let before = profiles.clone();
        assert_eq!(apply(&mut profiles, &sushiamo, choice), (1, false));
        assert_eq!(profiles, before);
        assert_eq!(log_line(&profiles, 1, false, false), "Opened sushiamo");
    }

    #[test]
    fn a_same_named_profile_without_an_app_id_gets_the_app_id() {
        let mut by_exe = profile("balatro", 1.3, true, None);
        by_exe.match_rule.exe = Some("Balatro.exe".into());
        let mut profiles = profiles(vec![by_exe]);
        let balatro = game(2379780, "Balatro");
        let choice = pick(&profiles, &balatro, 1.4);
        assert_eq!(choice, Pick::Link(0));
        assert_eq!(apply(&mut profiles, &balatro, choice), (0, true));
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
        match pick(&profiles, &game(1623730, "Palworld"), 1.4) {
            Pick::Create(p) => {
                assert_eq!(p.name, "Palworld (1623730)");
                assert_eq!(p.match_rule.steam_app_id, Some(1623730));
            }
            other => panic!("expected a new profile, got {other:?}"),
        }
    }

    #[test]
    fn card_subtitle_prefers_exe_then_app_id_then_name() {
        let mut p = profile("Sushiamo", 1.75, true, Some(4555040));
        assert_eq!(card_subtitle(&p), "Steam app 4555040");
        p.match_rule.exe = Some("Sushiamo.exe".into());
        assert_eq!(card_subtitle(&p), "Sushiamo.exe");
        assert_eq!(card_subtitle(&profile("CS2", 1.6, true, None)), "CS2");
    }
}
