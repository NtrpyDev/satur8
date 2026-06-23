//! Per-game profiles: how a game is recognised and what saturation it gets.
//!
//! Stored as TOML at `$XDG_CONFIG_HOME/satur8/profiles.toml`. The launch
//! wrapper (M2) and the focus watcher (M4) both resolve a running game to a
//! profile through [`Profiles::match_*`].

use crate::Saturation;
use serde::{Deserialize, Serialize};

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
    /// Target saturation, mirrors `Saturation` (0.0..=4.0, 1.0 = unchanged).
    pub saturation: f32,
    #[serde(default, flatten)]
    pub match_rule: MatchRule,
    /// Output ids to affect; empty = all outputs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<String>,
}

impl Profile {
    pub fn saturation(&self) -> Saturation {
        Saturation::new(self.saturation)
    }
}

/// The whole profiles file: a default applied when nothing matches, plus a list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Profiles {
    /// Desktop default saturation (usually 1.0 = no change).
    #[serde(default = "default_saturation")]
    pub default_saturation: f32,
    #[serde(default, rename = "profile")]
    pub profiles: Vec<Profile>,
}

fn default_saturation() -> f32 {
    1.0
}

impl Default for Profiles {
    fn default() -> Profiles {
        Profiles {
            default_saturation: 1.0,
            profiles: Vec::new(),
        }
    }
}

impl Profiles {
    pub fn default_saturation(&self) -> Saturation {
        Saturation::new(self.default_saturation)
    }

    /// Find a profile by executable basename (case-insensitive).
    pub fn match_exe(&self, exe: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| {
            p.match_rule
                .exe
                .as_deref()
                .is_some_and(|e| e.eq_ignore_ascii_case(exe))
        })
    }

    /// Find a profile by window class / app id (case-insensitive).
    pub fn match_window_class(&self, class: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| {
            p.match_rule
                .window_class
                .as_deref()
                .is_some_and(|c| c.eq_ignore_ascii_case(class))
        })
    }

    /// Find a profile by Steam AppID.
    pub fn match_steam_app_id(&self, app_id: u32) -> Option<&Profile> {
        self.profiles
            .iter()
            .find(|p| p.match_rule.steam_app_id == Some(app_id))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Profiles {
        Profiles {
            default_saturation: 1.0,
            profiles: vec![Profile {
                name: "cs2".into(),
                saturation: 1.6,
                match_rule: MatchRule {
                    exe: Some("cs2".into()),
                    window_class: None,
                    steam_app_id: Some(730),
                },
                outputs: vec![],
            }],
        }
    }

    #[test]
    fn matches_by_exe_case_insensitive() {
        let p = sample();
        assert_eq!(p.match_exe("CS2").unwrap().name, "cs2");
        assert!(p.match_exe("dota2").is_none());
    }

    #[test]
    fn matches_by_steam_app_id() {
        let p = sample();
        assert_eq!(p.match_steam_app_id(730).unwrap().saturation, 1.6);
        assert!(p.match_steam_app_id(570).is_none());
    }

    #[test]
    fn toml_round_trips() {
        let p = sample();
        let s = p.to_toml().unwrap();
        let back = Profiles::from_toml(&s).unwrap();
        assert_eq!(p, back);
    }
}
