//! `satur8 outputs default ...` and `satur8 outputs follow ...`: the per-output
//! settings in the profiles file.
//!
//! The edits are pure functions on [`Profiles`]; `run` loads the file, applies
//! one, saves through the same path as every other command (which tells the
//! daemon to reload), and says when the selected backend cannot use the
//! setting or the output is not connected now.

use anyhow::{bail, Result};
use clap::{Subcommand, ValueEnum};
use satur8_backend::select_backend;
use satur8_core::levels::ALL_OUTPUTS;
use satur8_core::{Output, OutputConfig, ProfileOutputs, Profiles};

use crate::config;

#[derive(Subcommand)]
pub enum OutputsCmd {
    /// Set the saturation one output shows when no game profile is changing
    /// it, or remove that setting with --unset.
    Default {
        /// The output's name, as `satur8 outputs` lists it, e.g. DP-2.
        name: String,
        /// Saturation for this output (0.0..=4.0).
        #[arg(
            value_parser = crate::parse_saturation,
            required_unless_present = "unset",
            conflicts_with = "unset"
        )]
        saturation: Option<f32>,
        /// Remove this output's own value, so it shows default_saturation again.
        #[arg(long)]
        unset: bool,
    },
    /// Choose what a game profile that lists no outputs changes.
    Follow {
        /// `all`: every output (the default). `focused`: only the output the
        /// game is on. `off` and `on` mean the same.
        #[arg(value_enum)]
        mode: FollowMode,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum FollowMode {
    /// Every output.
    #[value(alias = "off")]
    All,
    /// Only the output the game is on.
    #[value(alias = "on")]
    Focused,
}

pub fn run(cmd: OutputsCmd) -> Result<()> {
    let mut profiles = config::load_profiles()?;
    let (message, name) = match cmd {
        OutputsCmd::Default {
            name,
            saturation,
            unset,
        } => {
            let value = if unset { None } else { saturation };
            (set_output_default(&mut profiles, &name, value)?, Some(name))
        }
        OutputsCmd::Follow { mode } => (set_follow(&mut profiles, mode), None),
    };
    config::save_profiles(&profiles)?;
    println!("{message}");
    if let Ok(backend) = select_backend() {
        let outputs = backend.outputs();
        if let Some(note) = backend_note(
            backend.name(),
            backend.per_output(),
            &outputs,
            name.as_deref(),
        ) {
            println!("{note}");
        }
    }
    Ok(())
}

/// Set or remove `name`'s own desktop value. Returns what to tell the user.
pub fn set_output_default(
    profiles: &mut Profiles,
    name: &str,
    saturation: Option<f32>,
) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name == ALL_OUTPUTS {
        bail!("name one output, as `satur8 outputs` lists it");
    }
    match saturation {
        Some(value) => {
            satur8_core::Saturation::try_new(value)?;
            profiles
                .outputs
                .entry(name.to_string())
                .or_default()
                .default_saturation = Some(value);
            Ok(format!(
                "output '{name}' now shows {value:.2} on the desktop"
            ))
        }
        None => {
            let had_value = profiles
                .outputs
                .get(name)
                .is_some_and(|config| config.default_saturation.is_some());
            if let Some(config) = profiles.outputs.get_mut(name) {
                config.default_saturation = None;
                if *config == OutputConfig::default() {
                    profiles.outputs.remove(name);
                }
            }
            Ok(if had_value {
                format!(
                    "output '{name}' shows default_saturation ({:.2}) on the desktop again",
                    profiles.default_saturation
                )
            } else {
                format!("output '{name}' had no desktop value of its own")
            })
        }
    }
}

/// Set what a game profile that lists no outputs changes.
pub fn set_follow(profiles: &mut Profiles, mode: FollowMode) -> String {
    profiles.profile_outputs = match mode {
        FollowMode::All => ProfileOutputs::All,
        FollowMode::Focused => ProfileOutputs::Focused,
    };
    match mode {
        FollowMode::All => "game profiles that list no outputs change every output".to_string(),
        FollowMode::Focused => {
            "game profiles that list no outputs change only the output the game is on".to_string()
        }
    }
}

/// What to add when this backend cannot use the setting, or `name` is not
/// connected now. The setting is saved either way.
pub fn backend_note(
    backend: &str,
    per_output: bool,
    outputs: &[Output],
    name: Option<&str>,
) -> Option<String> {
    if !per_output {
        return Some(format!(
            "note: the {backend} backend applies one saturation to every output, \
             so this setting is not in use here"
        ));
    }
    let name = name?;
    (!outputs.iter().any(|output| output.id == name))
        .then(|| format!("note: '{name}' is not connected now; the setting applies when it is"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_and_removing_an_outputs_desktop_value() {
        let mut profiles = Profiles::default();
        assert_eq!(
            set_output_default(&mut profiles, "HDMI-A-1", Some(0.85)).unwrap(),
            "output 'HDMI-A-1' now shows 0.85 on the desktop"
        );
        assert_eq!(profiles.outputs["HDMI-A-1"].default_saturation, Some(0.85));
        assert_eq!(
            set_output_default(&mut profiles, "HDMI-A-1", None).unwrap(),
            "output 'HDMI-A-1' shows default_saturation (1.00) on the desktop again"
        );
        assert!(profiles.outputs.is_empty());
        assert_eq!(
            set_output_default(&mut profiles, "HDMI-A-1", None).unwrap(),
            "output 'HDMI-A-1' had no desktop value of its own"
        );
    }

    #[test]
    fn an_output_default_needs_one_real_name_and_a_valid_value() {
        let mut profiles = Profiles::default();
        assert!(set_output_default(&mut profiles, "", Some(1.2)).is_err());
        assert!(set_output_default(&mut profiles, "all", Some(1.2)).is_err());
        assert!(set_output_default(&mut profiles, "DP-2", Some(f32::NAN)).is_err());
        assert!(profiles.outputs.is_empty());
    }

    #[test]
    fn follow_sets_profile_outputs() {
        let mut profiles = Profiles::default();
        set_follow(&mut profiles, FollowMode::Focused);
        assert_eq!(profiles.profile_outputs, ProfileOutputs::Focused);
        set_follow(&mut profiles, FollowMode::All);
        assert_eq!(profiles.profile_outputs, ProfileOutputs::All);
    }

    #[test]
    fn notes_for_a_uniform_backend_and_an_output_not_connected() {
        let outputs = [Output {
            id: "DP-2".into(),
            human_name: "Dell".into(),
        }];
        assert_eq!(
            backend_note("gnome-shell", false, &outputs, Some("DP-2")).as_deref(),
            Some(
                "note: the gnome-shell backend applies one saturation to every output, \
                 so this setting is not in use here"
            )
        );
        assert_eq!(
            backend_note("kwin", true, &outputs, Some("HDMI-A-1")).as_deref(),
            Some("note: 'HDMI-A-1' is not connected now; the setting applies when it is")
        );
        assert_eq!(backend_note("kwin", true, &outputs, Some("DP-2")), None);
        assert_eq!(backend_note("kwin", true, &outputs, None), None);
    }
}
