//! `satur8 games` - list the installed Steam games and the profile each one
//! uses, and resolve `profile add --steam-game` to one of them.

use anyhow::Result;
use satur8_core::steam::{self, GameQuery, SteamGame};
use satur8_core::{Profile, Profiles};

use crate::config;

/// Printed when no Steam install folder exists.
const NO_STEAM: &str = "satur8: no Steam install found \
                        (looked in ~/.local/share/Steam, ~/.steam, Flatpak Steam and Snap Steam)";

/// Printed when Steam is installed but has no games.
const NO_GAMES: &str = "satur8: Steam is installed but has no games installed";

/// Print the installed Steam games as a table, or as JSON with `json`. Exits
/// with status 1 when no Steam install is found.
pub fn run(json: bool) -> Result<()> {
    if steam::steam_roots().is_empty() {
        eprintln!("{NO_STEAM}");
        std::process::exit(1);
    }
    let games = steam::installed_games();
    let profiles = config::load_profiles()?;
    println!("{}", render(&games, &profiles, json));
    Ok(())
}

/// The whole `satur8 games` output once Steam is known to be installed.
fn render(games: &[SteamGame], profiles: &Profiles, json: bool) -> String {
    if json {
        games_json(games, profiles)
    } else if games.is_empty() {
        NO_GAMES.to_string()
    } else {
        games_table(games, profiles)
    }
}

/// The profile shown for a game: the enabled one that applies to it, else the
/// first disabled one with its AppID.
fn profile_for(profiles: &Profiles, app_id: u32) -> Option<&Profile> {
    profiles.match_steam_app_id(app_id).or_else(|| {
        profiles
            .profiles
            .iter()
            .find(|profile| profile.match_rule.steam_app_id == Some(app_id))
    })
}

/// `sushiamo 1.75`, or `sushiamo 1.75 (disabled)`.
fn profile_label(profile: &Profile) -> String {
    format!(
        "{} {:.2}{}",
        profile.name,
        profile.saturation,
        if profile.enabled { "" } else { " (disabled)" }
    )
}

/// The APPID / NAME / PROFILE table, columns padded to the widest entry.
fn games_table(games: &[SteamGame], profiles: &Profiles) -> String {
    let rows: Vec<[String; 3]> = games
        .iter()
        .map(|game| {
            [
                game.app_id.to_string(),
                game.name.clone(),
                profile_for(profiles, game.app_id).map_or_else(|| "-".to_string(), profile_label),
            ]
        })
        .collect();
    let width = |column: usize, header: &str| {
        rows.iter()
            .map(|row| row[column].chars().count())
            .fold(header.chars().count(), usize::max)
    };
    let id_width = width(0, "APPID");
    let name_width = width(1, "NAME");
    std::iter::once(["APPID", "NAME", "PROFILE"].map(String::from))
        .chain(rows.iter().cloned())
        .map(|[id, name, profile]| {
            format!(
                "{:<id_w$}  {:<name_w$}  {}",
                id,
                name,
                profile,
                id_w = id_width,
                name_w = name_width
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A JSON array of `{ "app_id", "name", "profile", "saturation" }` objects,
/// one per line. `profile` and `saturation` are `null` for a game without one.
fn games_json(games: &[SteamGame], profiles: &Profiles) -> String {
    if games.is_empty() {
        return "[]".to_string();
    }
    let objects: Vec<String> = games
        .iter()
        .map(|game| {
            let profile = profile_for(profiles, game.app_id);
            format!(
                "  {{\"app_id\": {}, \"name\": {}, \"profile\": {}, \"saturation\": {}}}",
                game.app_id,
                json_string(&game.name),
                profile.map_or_else(|| "null".to_string(), |p| json_string(&p.name)),
                profile
                    .filter(|p| p.saturation.is_finite())
                    .map_or_else(|| "null".to_string(), |p| p.saturation.to_string())
            )
        })
        .collect();
    format!("[\n{}\n]", objects.join(",\n"))
}

/// `text` as a quoted JSON string, escaping quotes, backslashes and control
/// characters.
fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Resolve a `--steam-game` query to one installed game, or the message that
/// explains why it could not be resolved.
pub fn resolve_game(games: &[SteamGame], query: &str) -> Result<SteamGame, String> {
    match steam::find_game(games, query) {
        GameQuery::Found(game) => Ok(game),
        GameQuery::Ambiguous(candidates) => Err(ambiguous_message(query, &candidates)),
        GameQuery::NotFound => Err(format!(
            "no installed Steam game matches '{query}'; run `satur8 games` to see the installed games"
        )),
    }
}

/// The candidates as `APPID  NAME` lines between an opening and a closing sentence.
fn ambiguous_message(query: &str, candidates: &[SteamGame]) -> String {
    let id_width = candidates
        .iter()
        .map(|game| game.app_id.to_string().len())
        .fold(0, usize::max);
    let mut lines = vec![format!(
        "'{query}' matches more than one installed Steam game:"
    )];
    lines.extend(
        candidates
            .iter()
            .map(|game| format!("{:<id_w$}  {}", game.app_id, game.name, id_w = id_width)),
    );
    lines.push("Pass a longer name or the AppID to --steam-game.".to_string());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use satur8_core::MatchRule;
    use std::path::PathBuf;

    fn game(app_id: u32, name: &str) -> SteamGame {
        SteamGame {
            app_id,
            name: name.to_string(),
            library: PathBuf::from("/home/deck/.local/share/Steam"),
        }
    }

    fn games() -> Vec<SteamGame> {
        vec![
            game(1086940, "Baldur's Gate 3"),
            game(2868840, "Slay the Spire 2"),
            game(4555040, "Sushiamo"),
            game(105600, "Terraria"),
        ]
    }

    fn profile(name: &str, saturation: f32, enabled: bool, app_id: u32) -> Profile {
        Profile {
            name: name.into(),
            enabled,
            saturation,
            match_rule: MatchRule {
                exe: None,
                window_class: None,
                steam_app_id: Some(app_id),
            },
            outputs: vec![],
        }
    }

    fn profiles() -> Profiles {
        Profiles {
            default_saturation: 1.0,
            profiles: vec![
                profile("sushiamo", 1.75, true, 4555040),
                profile("spire", 1.4, false, 2868840),
            ],
        }
    }

    #[test]
    fn table_shows_each_game_and_its_profile() {
        assert_eq!(
            render(&games(), &profiles(), false),
            "APPID    NAME              PROFILE\n\
             1086940  Baldur's Gate 3   -\n\
             2868840  Slay the Spire 2  spire 1.40 (disabled)\n\
             4555040  Sushiamo          sushiamo 1.75\n\
             105600   Terraria          -"
        );
    }

    #[test]
    fn enabled_profile_is_shown_ahead_of_a_disabled_one() {
        let profiles = Profiles {
            default_saturation: 1.0,
            profiles: vec![
                profile("old", 1.2, false, 4555040),
                profile("current", 1.9, true, 4555040),
            ],
        };
        assert_eq!(
            profile_for(&profiles, 4555040).map(profile_label),
            Some("current 1.90".to_string())
        );
        assert_eq!(profile_for(&profiles, 1), None);
    }

    #[test]
    fn no_games_is_said_in_words() {
        assert_eq!(
            render(&[], &profiles(), false),
            "satur8: Steam is installed but has no games installed"
        );
        assert_eq!(render(&[], &profiles(), true), "[]");
    }

    #[test]
    fn json_lists_profile_and_saturation_or_null() {
        assert_eq!(
            render(&games()[1..3], &profiles(), true),
            "[\n  \
             {\"app_id\": 2868840, \"name\": \"Slay the Spire 2\", \"profile\": \"spire\", \"saturation\": 1.4},\n  \
             {\"app_id\": 4555040, \"name\": \"Sushiamo\", \"profile\": \"sushiamo\", \"saturation\": 1.75}\n\
             ]"
        );
        assert_eq!(
            render(&games()[3..], &profiles(), true),
            "[\n  {\"app_id\": 105600, \"name\": \"Terraria\", \"profile\": null, \"saturation\": null}\n]"
        );
    }

    #[test]
    fn json_strings_are_escaped() {
        assert_eq!(json_string("Sushiamo"), "\"Sushiamo\"");
        assert_eq!(json_string("Baldur's Gate 3"), "\"Baldur's Gate 3\"");
        assert_eq!(json_string("a \"b\" c"), "\"a \\\"b\\\" c\"");
        assert_eq!(json_string("C:\\Games"), "\"C:\\\\Games\"");
        assert_eq!(json_string("line\nnext\ttab\r"), "\"line\\nnext\\ttab\\r\"");
        assert_eq!(json_string("bell\u{7}"), "\"bell\\u0007\"");
        assert_eq!(json_string("Ōkami"), "\"Ōkami\"");
    }

    #[test]
    fn resolve_finds_one_game() {
        assert_eq!(
            resolve_game(&games(), "sushiamo"),
            Ok(game(4555040, "Sushiamo"))
        );
        assert_eq!(
            resolve_game(&games(), "2868840"),
            Ok(game(2868840, "Slay the Spire 2"))
        );
    }

    #[test]
    fn resolve_lists_candidates_when_ambiguous() {
        assert_eq!(
            resolve_game(&games(), "a"),
            Err("'a' matches more than one installed Steam game:\n\
                 1086940  Baldur's Gate 3\n\
                 2868840  Slay the Spire 2\n\
                 4555040  Sushiamo\n\
                 105600   Terraria\n\
                 Pass a longer name or the AppID to --steam-game."
                .to_string())
        );
    }

    #[test]
    fn resolve_points_at_satur8_games_when_nothing_matches() {
        assert_eq!(
            resolve_game(&games(), "zelda"),
            Err("no installed Steam game matches 'zelda'; \
                 run `satur8 games` to see the installed games"
                .to_string())
        );
    }
}
