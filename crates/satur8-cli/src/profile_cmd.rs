//! `satur8 profile ...` - manage the per-game profiles file.

use anyhow::{bail, Result};
use clap::Subcommand;
use satur8_core::{MatchRule, Profile, Profiles};

use crate::config;

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
    },
    /// Remove a profile by name.
    Remove { name: String },
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
        } => add(name, saturation, exe, window_class, steam_app_id),
        ProfileCmd::Remove { name } => remove(&name),
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
            "  {:<16} sat {:.2}  {}",
            p.name,
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
) -> Result<()> {
    let mut profiles = config::load_profiles()?;
    let profile = build_profile(name.clone(), saturation, exe, window_class, steam_app_id)?;
    if add_or_replace_profile(&mut profiles, profile) {
        println!("updated profile '{name}'");
    } else {
        println!("added profile '{name}'");
    }
    config::save_profiles(&profiles)?;
    Ok(())
}

fn remove(name: &str) -> Result<()> {
    let mut profiles = config::load_profiles()?;
    remove_profile_by_name(&mut profiles, name)?;
    config::save_profiles(&profiles)?;
    println!("removed profile '{name}'");
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
        bail!("a profile needs at least one match rule (--exe, --window-class, or --steam-app-id)");
    }
    Ok(Profile {
        name,
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
}
