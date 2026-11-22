//! `satur8 profile ...` - manage the per-game profiles file.

use anyhow::{bail, Result};
use clap::Subcommand;
use satur8_core::steam::{self, SteamGame};
use satur8_core::{MatchRule, Profile, Profiles};

use crate::{config, games_cmd};

#[derive(Subcommand)]
pub enum ProfileCmd {
    /// List all profiles.
    List,
    /// Show one profile's details.
    Show { name: String },
    /// Add or replace a profile.
    Add {
        name: String,
        /// Target saturation (0.0..=4.0).
        #[arg(value_parser = crate::parse_saturation)]
        saturation: f32,
        /// Match by executable basename, e.g. cs2.
        #[arg(long)]
        exe: Option<String>,
        /// Match by window class / app id.
        #[arg(long)]
        window_class: Option<String>,
        /// Match by Steam AppID, e.g. 730.
        #[arg(long)]
        steam_app_id: Option<u32>,
        /// Match by an installed Steam game, by name or AppID (see `satur8 games`).
        #[arg(long, conflicts_with = "steam_app_id")]
        steam_game: Option<String>,
    },
    /// Remove a profile by name.
    Remove { name: String },
    /// Enable a profile so it can match running games.
    Enable { name: String },
    /// Disable a profile without deleting it.
    Disable { name: String },
    /// Print the profiles file path.
    Path,
}

pub fn run(cmd: ProfileCmd) -> Result<()> {
    match cmd {
        ProfileCmd::List => list(),
        ProfileCmd::Show { name } => show(&name),
        ProfileCmd::Add {
            name,
            saturation,
            exe,
            window_class,
            steam_app_id,
            steam_game,
        } => {
            let game = match steam_game {
                Some(query) => Some(
                    games_cmd::resolve_game(&steam::installed_games(), &query)
                        .map_err(anyhow::Error::msg)?,
                ),
                None => None,
            };
            let steam_app_id = game.as_ref().map(|game| game.app_id).or(steam_app_id);
            add(
                name,
                saturation,
                exe,
                window_class,
                steam_app_id,
                game.as_ref(),
            )
        }
        ProfileCmd::Remove { name } => remove(&name),
        ProfileCmd::Enable { name } => set_enabled(&name, true),
        ProfileCmd::Disable { name } => set_enabled(&name, false),
        ProfileCmd::Path => {
            println!("{}", config::profiles_path()?.display());
            Ok(())
        }
    }
}

fn list() -> Result<()> {
    let profiles = config::load_profiles()?;
    println!("default saturation: {:.2}", profiles.default_saturation);
    if profiles.profiles.is_empty() {
        println!("(no profiles yet - add one with `satur8 profile add`)");
        return Ok(());
    }
    for p in &profiles.profiles {
        println!(
            "  {:<16} {:<8} sat {:.2}  {}",
            p.name,
            if p.enabled { "enabled" } else { "disabled" },
            p.saturation,
            describe_match(&p.match_rule)
        );
    }
    Ok(())
}

fn show(name: &str) -> Result<()> {
    let profiles = config::load_profiles()?;
    match profiles.by_name(name) {
        Some(p) => {
            println!("name:        {}", p.name);
            println!("enabled:     {}", p.enabled);
            println!("saturation:  {:.2}", p.saturation);
            println!("match:       {}", describe_match(&p.match_rule));
            if !p.outputs.is_empty() {
                println!("outputs:     {}", p.outputs.join(", "));
            }
            Ok(())
        }
        None => bail!("no profile named '{name}'"),
    }
}

fn add(
    name: String,
    saturation: f32,
    exe: Option<String>,
    window_class: Option<String>,
    steam_app_id: Option<u32>,
    game: Option<&SteamGame>,
) -> Result<()> {
    let mut profiles = config::load_profiles()?;
    let profile = build_profile(name.clone(), saturation, exe, window_class, steam_app_id)?;
    let updated = add_or_replace_profile(&mut profiles, profile);
    println!("{}", added_line(&name, updated, game));
    config::save_profiles(&profiles)?;
    Ok(())
}

/// `added profile 'NAME'` (or `updated ...`), followed by the resolved game
/// when `--steam-game` picked one. Scripts match on the first words.
fn added_line(name: &str, updated: bool, game: Option<&SteamGame>) -> String {
    let verb = if updated { "updated" } else { "added" };
    match game {
        Some(game) => format!(
            "{verb} profile '{name}' ({}, AppID {})",
            game.name, game.app_id
        ),
        None => format!("{verb} profile '{name}'"),
    }
}

fn remove(name: &str) -> Result<()> {
    let mut profiles = config::load_profiles()?;
    remove_profile_by_name(&mut profiles, name)?;
    config::save_profiles(&profiles)?;
    println!("removed profile '{name}'");
    Ok(())
}

fn set_enabled(name: &str, enabled: bool) -> Result<()> {
    let mut profiles = config::load_profiles()?;
    set_profile_enabled(&mut profiles, name, enabled)?;
    config::save_profiles(&profiles)?;
    println!(
        "{} profile '{name}'",
        if enabled { "enabled" } else { "disabled" }
    );
    Ok(())
}

fn build_profile(
    name: String,
    saturation: f32,
    exe: Option<String>,
    window_class: Option<String>,
    steam_app_id: Option<u32>,
) -> Result<Profile> {
    satur8_core::Saturation::try_new(saturation)?;
    if exe.is_none() && window_class.is_none() && steam_app_id.is_none() {
        bail!("a profile needs at least one match rule (--exe, --window-class, --steam-app-id, or --steam-game)");
    }
    Ok(Profile {
        name,
        enabled: true,
        saturation,
        match_rule: MatchRule {
            exe,
            window_class,
            steam_app_id,
        },
        outputs: vec![],
    })
}

fn add_or_replace_profile(profiles: &mut Profiles, profile: Profile) -> bool {
    if let Some(slot) = profiles
        .profiles
        .iter_mut()
        .find(|p| p.name.eq_ignore_ascii_case(&profile.name))
    {
        *slot = profile;
        true
    } else {
        profiles.profiles.push(profile);
        false
    }
}

fn remove_profile_by_name(profiles: &mut Profiles, name: &str) -> Result<()> {
    let before = profiles.profiles.len();
    profiles
        .profiles
        .retain(|p| !p.name.eq_ignore_ascii_case(name));
    if profiles.profiles.len() == before {
        bail!("no profile named '{name}'");
    }
    Ok(())
}

fn set_profile_enabled(profiles: &mut Profiles, name: &str, enabled: bool) -> Result<()> {
    let profile = profiles
        .profiles
        .iter_mut()
        .find(|profile| profile.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| anyhow::anyhow!("no profile named '{name}'"))?;
    profile.enabled = enabled;
    Ok(())
}

fn describe_match(m: &MatchRule) -> String {
    let mut parts = Vec::new();
    if let Some(e) = &m.exe {
        parts.push(format!("exe={e}"));
    }
    if let Some(c) = &m.window_class {
        parts.push(format!("class={c}"));
    }
    if let Some(id) = m.steam_app_id {
        parts.push(format!("appid={id}"));
    }
    if parts.is_empty() {
        "(no match rule)".into()
    } else {
        parts.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str, saturation: f32) -> Profile {
        Profile {
            name: name.into(),
            enabled: true,
            saturation,
            match_rule: MatchRule {
                exe: Some(name.to_ascii_lowercase()),
                window_class: None,
                steam_app_id: None,
            },
            outputs: vec![],
        }
    }

    #[test]
    fn build_profile_requires_a_match_rule() {
        assert!(build_profile("CS2".into(), 1.6, None, None, None).is_err());
        assert!(build_profile("CS2".into(), 1.6, Some("cs2".into()), None, None).is_ok());
        assert!(build_profile("CS2".into(), 1.6, None, Some("cs2".into()), None).is_ok());
        assert!(build_profile("CS2".into(), 1.6, None, None, Some(730)).is_ok());
    }

    #[test]
    fn added_line_names_the_resolved_game() {
        let game = SteamGame {
            app_id: 4555040,
            name: "Sushiamo".into(),
            library: std::path::PathBuf::from("/home/deck/.local/share/Steam"),
        };
        assert_eq!(
            added_line("sushiamo", false, Some(&game)),
            "added profile 'sushiamo' (Sushiamo, AppID 4555040)"
        );
        assert_eq!(
            added_line("sushiamo", true, Some(&game)),
            "updated profile 'sushiamo' (Sushiamo, AppID 4555040)"
        );
    }

    #[test]
    fn added_line_without_a_game_is_unchanged() {
        assert_eq!(added_line("cs2", false, None), "added profile 'cs2'");
        assert_eq!(added_line("cs2", true, None), "updated profile 'cs2'");
    }

    #[test]
    fn add_profile_appends_new_name() {
        let mut profiles = Profiles::default();
        let updated = add_or_replace_profile(&mut profiles, profile("CS2", 1.6));

        assert!(!updated);
        assert_eq!(profiles.profiles.len(), 1);
        assert_eq!(profiles.profiles[0].name, "CS2");
    }

    #[test]
    fn add_profile_replaces_same_name_case_insensitive() {
        let mut profiles = Profiles {
            default_saturation: 1.0,
            profiles: vec![profile("CS2", 1.6)],
        };

        let updated = add_or_replace_profile(&mut profiles, profile("cs2", 1.9));

        assert!(updated);
        assert_eq!(profiles.profiles.len(), 1);
        assert_eq!(profiles.profiles[0].name, "cs2");
        assert_eq!(profiles.profiles[0].saturation, 1.9);
    }

    #[test]
    fn remove_profile_is_case_insensitive() {
        let mut profiles = Profiles {
            default_saturation: 1.0,
            profiles: vec![profile("CS2", 1.6), profile("Dota2", 1.4)],
        };

        remove_profile_by_name(&mut profiles, "cs2").unwrap();

        assert_eq!(profiles.profiles.len(), 1);
        assert_eq!(profiles.profiles[0].name, "Dota2");
    }

    #[test]
    fn remove_profile_errors_on_missing_name() {
        let mut profiles = Profiles {
            default_saturation: 1.0,
            profiles: vec![profile("CS2", 1.6)],
        };

        assert!(remove_profile_by_name(&mut profiles, "missing").is_err());
        assert_eq!(profiles.profiles.len(), 1);
    }

    #[test]
    fn profile_enabled_state_can_be_changed_by_name() {
        let mut profiles = Profiles {
            default_saturation: 1.0,
            profiles: vec![profile("CS2", 1.6)],
        };

        set_profile_enabled(&mut profiles, "cs2", false).unwrap();
        assert!(!profiles.profiles[0].enabled);
        set_profile_enabled(&mut profiles, "CS2", true).unwrap();
        assert!(profiles.profiles[0].enabled);
        assert!(set_profile_enabled(&mut profiles, "missing", false).is_err());
    }
}
