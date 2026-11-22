//! Per-game profiles: how a game is recognised and what saturation it gets.
//!
//! Stored as TOML at `$XDG_CONFIG_HOME/satur8/profiles.toml`. The launch
//! wrapper (M2) and the focus watcher (M4) both resolve a running game to a
//! profile through [`Profiles::match_*`].

use std::collections::BTreeMap;

use crate::Saturation;
use serde::{Deserialize, Deserializer, Serialize};

/// How a profile is matched to a running game. Any populated field that matches
/// selects the profile; the first matching profile in file order wins.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MatchRule {
    /// Executable basename, e.g. `cs2`. Case-insensitive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exe: Option<String>,
    /// Window class / app id, e.g. `cs2.x86_64`. Case-insensitive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_class: Option<String>,
    /// Steam AppID, e.g. `730` for CS2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steam_app_id: Option<u32>,
}

/// A single named profile.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Profile {
    pub name: String,
    /// Disabled profiles remain editable but never match a running game.
    #[serde(default = "profile_enabled")]
    pub enabled: bool,
    /// Target saturation, mirrors `Saturation` (0.0..=4.0, 1.0 = unchanged).
    #[serde(deserialize_with = "deserialize_saturation")]
    pub saturation: f32,
    #[serde(default, flatten)]
    pub match_rule: MatchRule,
    /// Output ids to affect; empty = all outputs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<String>,
}

fn profile_enabled() -> bool {
    true
}

impl Profile {
    pub fn saturation(&self) -> Saturation {
        Saturation::new(self.saturation)
    }
}

/// Which outputs a game profile changes when the profile itself lists none.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProfileOutputs {
    /// Every output. The value when the key is absent.
    #[default]
    All,
    /// Only the output the focused game window is on.
    Focused,
}

impl ProfileOutputs {
    fn is_all(&self) -> bool {
        *self == ProfileOutputs::All
    }
}

/// Settings for one output, keyed by the name `satur8 outputs` prints.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct OutputConfig {
    /// The saturation this output shows when no game profile is changing it,
    /// in place of the top-level `default_saturation`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_saturation"
    )]
    pub default_saturation: Option<f32>,
}

/// The whole profiles file: a default applied when nothing matches, plus a list.
///
/// `profile_outputs` and the `[output."NAME"]` tables are written only when they
/// are used, so a file that uses neither is written exactly as before they
/// existed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Profiles {
    /// Desktop default saturation (usually 1.0 = no change).
    #[serde(
        default = "default_saturation",
        deserialize_with = "deserialize_saturation"
    )]
    pub default_saturation: f32,
    /// Which outputs a profile that lists none changes.
    #[serde(default, skip_serializing_if = "ProfileOutputs::is_all")]
    pub profile_outputs: ProfileOutputs,
    /// Per-output settings, from the `[output."NAME"]` tables.
    #[serde(default, rename = "output", skip_serializing_if = "BTreeMap::is_empty")]
    pub outputs: BTreeMap<String, OutputConfig>,
    #[serde(default, rename = "profile")]
    pub profiles: Vec<Profile>,
}

fn default_saturation() -> f32 {
    1.0
}

fn deserialize_optional_saturation<'de, D>(deserializer: D) -> Result<Option<f32>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = f32::deserialize(deserializer)?;
    Saturation::try_new(value).map_err(serde::de::Error::custom)?;
    Ok(Some(value))
}

fn deserialize_saturation<'de, D>(deserializer: D) -> Result<f32, D::Error>
where
    D: Deserializer<'de>,
{
    let value = f32::deserialize(deserializer)?;
    Saturation::try_new(value).map_err(serde::de::Error::custom)?;
    Ok(value)
}

impl Default for Profiles {
    fn default() -> Profiles {
        Profiles {
            default_saturation: 1.0,
            profile_outputs: ProfileOutputs::All,
            outputs: BTreeMap::new(),
            profiles: Vec::new(),
        }
    }
}

impl Profiles {
    pub fn default_saturation(&self) -> Saturation {
        Saturation::new(self.default_saturation)
    }

    /// Find a profile by executable basename. Case-insensitive, and `.exe` /
    /// `.x86_64` suffixes are ignored so a profile added as `EscapeFromTarkov.exe`
    /// still matches a process resolved as `EscapeFromTarkov`.
    pub fn match_exe(&self, exe: &str) -> Option<&Profile> {
        let exe_norm = normalize_game_id(exe);
        self.profiles.iter().find(|p| {
            p.enabled
                && p.match_rule
                    .exe
                    .as_deref()
                    .is_some_and(|e| normalize_game_id(e) == exe_norm)
        })
    }

    /// Find a profile by window class / app id (case-insensitive).
    pub fn match_window_class(&self, class: &str) -> Option<&Profile> {
        let class = class.trim();
        let class_norm = normalize_game_id(class);
        let steam_app_id = class_norm
            .strip_prefix("steam_app_")
            .and_then(|id| id.parse::<u32>().ok());

        self.profiles.iter().find(|p| {
            p.enabled
                && (p
                    .match_rule
                    .window_class
                    .as_deref()
                    .is_some_and(|c| normalize_game_id(c) == class_norm)
                    || p.match_rule
                        .exe
                        .as_deref()
                        .is_some_and(|e| normalize_game_id(e) == class_norm)
                    || steam_app_id.is_some_and(|app_id| p.match_rule.steam_app_id == Some(app_id)))
        })
    }

    /// Find a profile by Steam AppID.
    pub fn match_steam_app_id(&self, app_id: u32) -> Option<&Profile> {
        self.profiles
            .iter()
            .find(|p| p.enabled && p.match_rule.steam_app_id == Some(app_id))
    }

    /// Look a profile up by its name (case-insensitive).
    pub fn by_name(&self, name: &str) -> Option<&Profile> {
        self.profiles
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
    }

    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    pub fn from_toml(s: &str) -> Result<Profiles, toml::de::Error> {
        toml::from_str(s)
    }
}

fn normalize_game_id(id: &str) -> String {
    let id = id.trim().to_ascii_lowercase();
    id.trim_end_matches(".exe")
        .trim_end_matches(".x86_64")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Profiles {
        Profiles {
            default_saturation: 1.0,
            profiles: vec![
                Profile {
                    name: "cs2".into(),
                    enabled: true,
                    saturation: 1.6,
                    match_rule: MatchRule {
                        exe: Some("cs2".into()),
                        window_class: None,
                        steam_app_id: Some(730),
                    },
                    outputs: vec![],
                },
                Profile {
                    name: "Dota 2".into(),
                    enabled: true,
                    saturation: 1.4,
                    match_rule: MatchRule {
                        exe: Some("dota2".into()),
                        window_class: Some("dota2.x86_64".into()),
                        steam_app_id: Some(570),
                    },
                    outputs: vec!["DP-1".into()],
                },
                Profile {
                    name: "Window Only".into(),
                    enabled: true,
                    saturation: 1.2,
                    match_rule: MatchRule {
                        exe: None,
                        window_class: Some("com.example.Game".into()),
                        steam_app_id: None,
                    },
                    outputs: vec![],
                },
            ],
            ..Profiles::default()
        }
    }

    #[test]
    fn matches_by_exe_case_insensitive() {
        let p = sample();
        assert_eq!(p.match_exe("CS2").unwrap().name, "cs2");
        assert_eq!(p.match_exe("dota2").unwrap().name, "Dota 2");
        assert!(p.match_exe("unknown").is_none());
    }

    #[test]
    fn matches_by_steam_app_id() {
        let p = sample();
        assert_eq!(p.match_steam_app_id(730).unwrap().saturation, 1.6);
        assert_eq!(p.match_steam_app_id(570).unwrap().name, "Dota 2");
        assert!(p.match_steam_app_id(440).is_none());
    }

    #[test]
    fn matches_by_window_class_case_insensitive() {
        let p = sample();
        assert_eq!(
            p.match_window_class("COM.EXAMPLE.GAME").unwrap().name,
            "Window Only"
        );
        assert_eq!(p.match_window_class("DOTA2.X86_64").unwrap().name, "Dota 2");
    }

    #[test]
    fn first_matching_profile_wins() {
        let p = Profiles {
            default_saturation: 1.0,
            profiles: vec![
                Profile {
                    name: "first".into(),
                    enabled: true,
                    saturation: 1.2,
                    match_rule: MatchRule {
                        exe: Some("same-game".into()),
                        window_class: Some("same-class".into()),
                        steam_app_id: Some(1),
                    },
                    outputs: vec![],
                },
                Profile {
                    name: "second".into(),
                    enabled: true,
                    saturation: 1.8,
                    match_rule: MatchRule {
                        exe: Some("same-game".into()),
                        window_class: Some("same-class".into()),
                        steam_app_id: Some(1),
                    },
                    outputs: vec![],
                },
            ],
            ..Profiles::default()
        };

        assert_eq!(p.match_exe("same-game").unwrap().name, "first");
        assert_eq!(p.match_window_class("same-class").unwrap().name, "first");
        assert_eq!(p.match_steam_app_id(1).unwrap().name, "first");
    }

    #[test]
    fn finds_profile_by_name_case_insensitive() {
        let p = sample();
        assert_eq!(p.by_name("CS2").unwrap().name, "cs2");
        assert_eq!(p.by_name("dota 2").unwrap().name, "Dota 2");
        assert!(p.by_name("missing").is_none());
    }

    #[test]
    fn window_class_falls_back_to_exe_and_steam_app_id() {
        let p = sample();
        assert_eq!(p.match_window_class("cs2").unwrap().name, "cs2");
        assert_eq!(p.match_window_class("cs2.x86_64").unwrap().name, "cs2");
        assert_eq!(p.match_window_class("steam_app_730").unwrap().name, "cs2");
        assert!(p.match_window_class("unknown").is_none());
    }

    #[test]
    fn window_class_does_not_fall_back_to_profile_name() {
        let p = Profiles {
            default_saturation: 1.0,
            profiles: vec![Profile {
                name: "Steam".into(),
                enabled: true,
                saturation: 1.5,
                match_rule: MatchRule::default(),
                outputs: vec![],
            }],
            ..Profiles::default()
        };

        assert!(p.match_window_class("Steam").is_none());
    }

    #[test]
    fn disabled_profile_never_matches() {
        let profiles = Profiles::from_toml(
            "[[profile]]\nname = 'CS2'\nsaturation = 1.6\nenabled = false\nexe = 'cs2'\nsteam_app_id = 730",
        )
        .unwrap();

        assert!(profiles.match_exe("cs2").is_none());
        assert!(profiles.match_window_class("cs2").is_none());
        assert!(profiles.match_window_class("steam_app_730").is_none());
        assert!(profiles.match_steam_app_id(730).is_none());
    }

    #[test]
    fn profiles_without_enabled_field_remain_enabled() {
        let profiles = Profiles::from_toml(
            "[[profile]]\nname = 'Legacy'\nsaturation = 1.5\nexe = 'legacy-game'",
        )
        .unwrap();

        assert!(profiles.profiles[0].enabled);
        assert_eq!(profiles.match_exe("legacy-game").unwrap().name, "Legacy");
    }

    /// What v0.4.1 writes for `sample()`, pinned so the new schema items cannot
    /// appear in a file that does not use them. Saturations are `f32`, which
    /// the TOML writer prints at full `f64` precision.
    const SAMPLE_TOML: &str = r#"default_saturation = 1.0

[[profile]]
name = "cs2"
enabled = true
saturation = 1.600000023841858
exe = "cs2"
steam_app_id = 730

[[profile]]
name = "Dota 2"
enabled = true
saturation = 1.399999976158142
exe = "dota2"
window_class = "dota2.x86_64"
steam_app_id = 570
outputs = ["DP-1"]

[[profile]]
name = "Window Only"
enabled = true
saturation = 1.2000000476837158
window_class = "com.example.Game"
"#;

    #[test]
    fn an_old_file_writes_back_with_no_new_keys() {
        assert_eq!(sample().to_toml().unwrap(), SAMPLE_TOML);
        let parsed = Profiles::from_toml(SAMPLE_TOML).unwrap();
        assert_eq!(parsed, sample());
        assert_eq!(parsed.profile_outputs, ProfileOutputs::All);
        assert!(parsed.outputs.is_empty());
        assert_eq!(parsed.to_toml().unwrap(), SAMPLE_TOML);
    }

    /// The example file from the per-output design.
    const PER_OUTPUT_EXAMPLE: &str = r#"
default_saturation = 1.0
profile_outputs = "focused"

[output."DP-2"]
default_saturation = 1.0

[output."HDMI-A-1"]
default_saturation = 0.85

[[profile]]
name = "cs2"
saturation = 1.6
exe = "cs2"
steam_app_id = 730

[[profile]]
name = "surround racing"
saturation = 1.4
exe = "ams2"
outputs = ["all"]

[[profile]]
name = "couch game"
saturation = 1.3
exe = "hades"
outputs = ["HDMI-A-1"]
"#;

    #[test]
    fn the_per_output_example_file_round_trips() {
        let parsed = Profiles::from_toml(PER_OUTPUT_EXAMPLE).unwrap();
        assert_eq!(parsed.profile_outputs, ProfileOutputs::Focused);
        assert_eq!(parsed.outputs.len(), 2);
        assert_eq!(parsed.outputs["DP-2"].default_saturation, Some(1.0));
        assert_eq!(parsed.outputs["HDMI-A-1"].default_saturation, Some(0.85));
        assert_eq!(parsed.profiles.len(), 3);
        assert_eq!(parsed.profiles[1].outputs, vec!["all".to_string()]);
        assert_eq!(parsed.profiles[2].outputs, vec!["HDMI-A-1".to_string()]);

        let written = parsed.to_toml().unwrap();
        assert!(
            written.contains("profile_outputs = \"focused\""),
            "{written}"
        );
        assert!(
            written.contains("[output.DP-2]") || written.contains("[output.\"DP-2\"]"),
            "{written}"
        );
        assert_eq!(Profiles::from_toml(&written).unwrap(), parsed);
    }

    #[test]
    fn per_output_values_are_checked_like_every_other_saturation() {
        assert!(Profiles::from_toml("[output.\"DP-2\"]\ndefault_saturation = nan").is_err());
        assert!(Profiles::from_toml("profile_outputs = \"sideways\"").is_err());
        let empty = Profiles::from_toml("[output.\"DP-2\"]\n").unwrap();
        assert_eq!(empty.outputs["DP-2"].default_saturation, None);
    }

    /// A file written by a newer Satur8 must still load in this one: keys it
    /// does not know, at the top level, in a table it does not know, and inside
    /// a profile, are ignored and every known value is kept.
    #[test]
    fn unknown_keys_and_tables_are_ignored_when_loading() {
        let profiles = Profiles::from_toml(
            r#"
default_saturation = 1.2
some_future_setting = "focused"

[some_future_table."DP-2"]
default_saturation = 0.9

[[profile]]
name = "cs2"
saturation = 1.6
exe = "cs2"
some_future_profile_key = ["HDMI-A-1"]
another_one = 3
"#,
        )
        .expect("unknown keys must not stop the file from loading");

        assert_eq!(profiles.default_saturation, 1.2);
        assert_eq!(profiles.profiles.len(), 1);
        let cs2 = &profiles.profiles[0];
        assert_eq!(cs2.name, "cs2");
        assert_eq!(cs2.saturation, 1.6);
        assert!(cs2.enabled);
        assert_eq!(cs2.match_rule.exe.as_deref(), Some("cs2"));
        assert_eq!(cs2.match_rule.window_class, None);
        assert_eq!(cs2.match_rule.steam_app_id, None);
        assert!(cs2.outputs.is_empty());
    }

    #[test]
    fn toml_rejects_non_finite_saturation() {
        assert!(Profiles::from_toml("default_saturation = nan").is_err());
        assert!(
            Profiles::from_toml("[[profile]]\nname = 'bad'\nsaturation = inf\nexe = 'bad'")
                .is_err()
        );
    }

    #[test]
    fn missing_default_saturation_defaults_to_identity() {
        let p = Profiles::from_toml("").unwrap();
        assert!(p.default_saturation().is_identity());
        assert!(p.profiles.is_empty());
    }

    #[test]
    fn toml_round_trips() {
        let p = sample();
        let s = p.to_toml().unwrap();
        let back = Profiles::from_toml(&s).unwrap();
        assert_eq!(p, back);
    }
}
