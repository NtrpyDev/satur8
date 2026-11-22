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
        /// Change only this output while the profile is applied, by the name
        /// `satur8 outputs` lists; repeat for more. `all` means every output.
        #[arg(long = "output", value_name = "NAME")]
        outputs: Vec<String>,
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
            outputs,
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
                outputs,
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
        println!("{}", list_row(p));
    }
    Ok(())
}

/// One `satur8 profile list` row. The outputs a profile names come last, and
/// only when it names any, so other rows are as they always were.
fn list_row(p: &Profile) -> String {
    let row = format!(
        "  {:<16} {:<8} sat {:.2}  {}",
        p.name,
        if p.enabled { "enabled" } else { "disabled" },
        p.saturation,
        describe_match(&p.match_rule)
    );
    if p.outputs.is_empty() {
        row
    } else {
        format!("{row} outputs={}", p.outputs.join(","))
    }
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

#[allow(clippy::too_many_arguments)]
fn add(
    name: String,
    saturation: f32,
    exe: Option<String>,
    window_class: Option<String>,
    steam_app_id: Option<u32>,
    outputs: Vec<String>,
    game: Option<&SteamGame>,
) -> Result<()> {
    let mut profiles = config::load_profiles()?;
    let mut profile = build_profile(name.clone(), saturation, exe, window_class, steam_app_id)?;
    profile.outputs = profile_outputs(outputs)?;
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

/// The `--output` names, trimmed, each once, in the order given.
fn profile_outputs(names: Vec<String>) -> Result<Vec<String>> {
    let mut outputs: Vec<String> = Vec::new();
    for name in names {
        let name = name.trim().to_string();
        if name.is_empty() {
            bail!("--output needs an output name, as `satur8 outputs` lists it, or `all`");
        }
        if !outputs.contains(&name) {
            outputs.push(name);
        }
    }
    Ok(outputs)
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
            ..Profiles::default()
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
            ..Profiles::default()
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
            ..Profiles::default()
        };

        assert!(remove_profile_by_name(&mut profiles, "missing").is_err());
        assert_eq!(profiles.profiles.len(), 1);
    }

    #[test]
    fn profile_enabled_state_can_be_changed_by_name() {
        let mut profiles = Profiles {
            default_saturation: 1.0,
            profiles: vec![profile("CS2", 1.6)],
            ..Profiles::default()
        };

        set_profile_enabled(&mut profiles, "cs2", false).unwrap();
        assert!(!profiles.profiles[0].enabled);
        set_profile_enabled(&mut profiles, "CS2", true).unwrap();
        assert!(profiles.profiles[0].enabled);
        assert!(set_profile_enabled(&mut profiles, "missing", false).is_err());
    }

    #[test]
    fn profile_outputs_are_trimmed_and_each_kept_once() {
        assert_eq!(
            profile_outputs(vec![" HDMI-A-1 ".into(), "DP-2".into(), "HDMI-A-1".into()]).unwrap(),
            ["HDMI-A-1", "DP-2"]
        );
        assert!(profile_outputs(vec![" ".into()]).is_err());
        assert!(profile_outputs(vec![]).unwrap().is_empty());
    }

    /// From an empty file to the design's per-output example through the
    /// commands' own edits, and back through the file.
    #[test]
    fn per_output_settings_from_an_empty_file_round_trip() {
        use crate::outputs_cmd::{set_follow, set_output_default, FollowMode};

        let mut profiles = Profiles::from_toml("").unwrap();
        set_output_default(&mut profiles, "HDMI-A-1", Some(0.85)).unwrap();
        set_follow(&mut profiles, FollowMode::Focused);
        let mut couch =
            build_profile("couch".into(), 1.3, Some("hades".into()), None, None).unwrap();
        couch.outputs = profile_outputs(vec!["HDMI-A-1".into()]).unwrap();
        add_or_replace_profile(&mut profiles, couch);

        let written = profiles.to_toml().unwrap();
        assert_eq!(
            written,
            "default_saturation = 1.0\n\
             profile_outputs = \"focused\"\n\
             \n\
             [output.HDMI-A-1]\n\
             default_saturation = 0.8500000238418579\n\
             \n\
             [[profile]]\n\
             name = \"couch\"\n\
             enabled = true\n\
             saturation = 1.2999999523162842\n\
             exe = \"hades\"\n\
             outputs = [\"HDMI-A-1\"]\n"
        );
        assert_eq!(Profiles::from_toml(&written).unwrap(), profiles);

        // Clearing both settings writes the file as before they existed.
        set_output_default(&mut profiles, "HDMI-A-1", None).unwrap();
        set_follow(&mut profiles, FollowMode::All);
        let cleared = profiles.to_toml().unwrap();
        assert!(!cleared.contains("profile_outputs"), "{cleared}");
        assert!(!cleared.contains("[output"), "{cleared}");
    }

    #[test]
    fn list_rows_show_outputs_only_when_a_profile_names_any() {
        let mut cs2 =
            build_profile("cs2".into(), 1.6, Some("cs2".into()), None, Some(730)).unwrap();
        assert_eq!(
            list_row(&cs2),
            "  cs2              enabled  sat 1.60  exe=cs2 appid=730"
        );
        cs2.outputs = vec!["HDMI-A-1".into(), "DP-2".into()];
        cs2.enabled = false;
        assert_eq!(
            list_row(&cs2),
            "  cs2              disabled sat 1.60  exe=cs2 appid=730 outputs=HDMI-A-1,DP-2"
        );
    }

    #[test]
    fn profile_add_rejects_an_empty_output_name() {
        let err = profile_outputs(vec!["".into()]).unwrap_err().to_string();
        assert_eq!(
            err,
            "--output needs an output name, as `satur8 outputs` lists it, or `all`"
        );
    }
}
