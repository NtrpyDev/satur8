//! `satur8 profile ...` - manage the per-game profiles file.

use anyhow::{bail, Result};
use clap::Subcommand;
use satur8_core::{MatchRule, Profile};

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
        println!("  {:<16} sat {:.2}  {}", p.name, p.saturation, describe_match(&p.match_rule));
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
    if exe.is_none() && window_class.is_none() && steam_app_id.is_none() {
        bail!("a profile needs at least one match rule (--exe, --window-class, or --steam-app-id)");
    }
    let mut profiles = config::load_profiles()?;
    let profile = Profile {
        name: name.clone(),
        saturation,
        match_rule: MatchRule {
            exe,
            window_class,
            steam_app_id,
        },
        outputs: vec![],
    };
    // Replace an existing profile of the same name, else append.
    if let Some(slot) = profiles
        .profiles
        .iter_mut()
        .find(|p| p.name.eq_ignore_ascii_case(&name))
    {
        *slot = profile;
        println!("updated profile '{name}'");
    } else {
        profiles.profiles.push(profile);
        println!("added profile '{name}'");
    }
    config::save_profiles(&profiles)?;
    Ok(())
}

fn remove(name: &str) -> Result<()> {
    let mut profiles = config::load_profiles()?;
    let before = profiles.profiles.len();
    profiles.profiles.retain(|p| !p.name.eq_ignore_ascii_case(name));
    if profiles.profiles.len() == before {
        bail!("no profile named '{name}'");
    }
    config::save_profiles(&profiles)?;
    println!("removed profile '{name}'");
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
