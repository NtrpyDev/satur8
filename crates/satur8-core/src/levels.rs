//! What every output should show, worked out from the profiles file.
//!
//! Pure: no I/O, no backend, no D-Bus. The daemon, `satur8 run` and the desktop
//! app ask these functions for a [`Picture`] and hand its [`Levels`] to the
//! backend, so every rule for which output gets which saturation is decided
//! and tested here once.

use std::collections::BTreeMap;
use std::fmt;

use crate::{Output, Profile, ProfileOutputs, Profiles, Saturation};

/// The output id that means every output, both in a profile's `outputs` list
/// and as the target of a whole-picture apply.
pub const ALL_OUTPUTS: &str = "all";

/// What every output should show. Outputs not listed show `base`.
#[derive(Debug, Clone, PartialEq)]
pub struct Levels {
    /// The saturation of every output that has no entry in `outputs`.
    pub base: Saturation,
    /// Outputs that differ from `base`, by output name. [`Levels::new`] drops
    /// entries equal to `base`.
    pub outputs: BTreeMap<String, Saturation>,
}

impl Levels {
    /// The same saturation on every output.
    pub fn uniform(saturation: Saturation) -> Levels {
        Levels {
            base: saturation,
            outputs: BTreeMap::new(),
        }
    }

    /// `base` everywhere except the listed outputs. Entries equal to `base`
    /// are dropped, so a picture that is the same everywhere is uniform.
    pub fn new(
        base: Saturation,
        outputs: impl IntoIterator<Item = (String, Saturation)>,
    ) -> Levels {
        Levels {
            base,
            outputs: outputs
                .into_iter()
                .filter(|(_, saturation)| *saturation != base)
                .collect(),
        }
    }

    /// The one saturation every output shows, or `None` when outputs differ.
    pub fn uniform_value(&self) -> Option<Saturation> {
        self.outputs
            .values()
            .all(|saturation| *saturation == self.base)
            .then_some(self.base)
    }

    /// Whether every output is unchanged (saturation 1.0).
    pub fn is_identity(&self) -> bool {
        self.uniform_value().is_some_and(Saturation::is_identity)
    }
}

/// Something about a picture the user should be told, because the result is
/// not exactly what the file asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    /// The file has `[output."NAME"]` defaults, and this backend shows one
    /// saturation on every output, so they are not used.
    PerOutputDefaultsNotUsed,
    /// The profile names outputs, or `profile_outputs` is `"focused"`, and this
    /// backend shows one saturation on every output, so it changes all of them.
    ProfileChangesEveryOutput { profile: String },
    /// `profile_outputs` is `"focused"` but no output was reported for the
    /// focused window, so the profile changes every output.
    FocusedOutputUnknown { profile: String },
    /// The profile names an output that is not connected. Nothing on screen
    /// changes for it until it is.
    OutputNotConnected { profile: String, output: String },
    /// Why the backend shows one value on every output right now, and what to
    /// do, when that is something the user can change.
    OneValueBecause { reason: String },
}

impl fmt::Display for Note {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Note::PerOutputDefaultsNotUsed => f.write_str(
                "the per-output defaults are not used: this backend shows one saturation on every output",
            ),
            Note::ProfileChangesEveryOutput { profile } => write!(
                f,
                "profile '{profile}' changes every output: this backend shows one saturation on every output"
            ),
            Note::FocusedOutputUnknown { profile } => write!(
                f,
                "profile '{profile}' changes every output: the focused window's output is unknown"
            ),
            Note::OneValueBecause { reason } => f.write_str(reason),
            Note::OutputNotConnected { profile, output } => write!(
                f,
                "profile '{profile}' names output '{output}', which is not connected now"
            ),
        }
    }
}

/// The levels to show, and what to tell the user about them.
#[derive(Debug, Clone, PartialEq)]
pub struct Picture {
    pub levels: Levels,
    pub notes: Vec<Note>,
}

/// The per-output defaults the file sets, by output name.
fn output_defaults(profiles: &Profiles) -> impl Iterator<Item = (String, Saturation)> + '_ {
    profiles.outputs.iter().filter_map(|(name, config)| {
        config
            .default_saturation
            .map(|value| (name.clone(), Saturation::new(value)))
    })
}

/// What the outputs show for the focused window: the picture of the profile
/// that matches it, on or off, or the desktop picture when no profile does.
/// The four cases, in one place:
///
/// 1. a game whose profile is on: the profile's value;
/// 2. a game whose profile is off: normal colors (1.0), whatever the general
///    value is;
/// 3. a game with no profile: the general value;
/// 4. the desktop, and Steam's own screens in Game Mode: the general value.
pub fn focus_picture(
    profiles: &Profiles,
    matched: Option<&Profile>,
    focused_output: Option<&str>,
    per_output: bool,
) -> Picture {
    match matched {
        Some(profile) => profile_picture(profiles, profile, focused_output, per_output),
        None => desktop_picture(profiles, per_output),
    }
}

/// What the outputs show when no game profile is changing them. On a backend
/// that shows one saturation everywhere that is the top-level
/// `default_saturation`, with a note when the file sets per-output defaults.
pub fn desktop_picture(profiles: &Profiles, per_output: bool) -> Picture {
    let base = profiles.default_saturation();
    if per_output {
        return Picture {
            levels: Levels::new(base, output_defaults(profiles)),
            notes: Vec::new(),
        };
    }
    let notes = if output_defaults(profiles).next().is_some() {
        vec![Note::PerOutputDefaultsNotUsed]
    } else {
        Vec::new()
    };
    Picture {
        levels: Levels::uniform(base),
        notes,
    }
}

/// What the outputs show while `profile` is applied. `focused_output` is the
/// output the focused window is on, when the focus source reported one.
///
/// A profile that is off shows normal colors (1.0) on exactly the outputs it
/// would change when on: every output, the outputs it lists, or the focused
/// output with `profile_outputs = "focused"`. Other outputs keep the desktop
/// picture either way.
pub fn profile_picture(
    profiles: &Profiles,
    profile: &Profile,
    focused_output: Option<&str>,
    per_output: bool,
) -> Picture {
    let saturation = profile.shown_saturation();
    let every_output = |notes: Vec<Note>| Picture {
        levels: Levels::uniform(saturation),
        notes,
    };
    let changes_every_output = || {
        vec![Note::ProfileChangesEveryOutput {
            profile: profile.name.clone(),
        }]
    };
    let only = |names: &mut dyn Iterator<Item = String>| {
        let desktop = desktop_picture(profiles, true).levels;
        let mut outputs = desktop.outputs;
        for name in names {
            outputs.insert(name, saturation);
        }
        Picture {
            levels: Levels::new(desktop.base, outputs),
            notes: Vec::new(),
        }
    };

    if profile.outputs.iter().any(|name| name == ALL_OUTPUTS) {
        return every_output(Vec::new());
    }
    if !profile.outputs.is_empty() {
        if !per_output {
            return every_output(changes_every_output());
        }
        return only(&mut profile.outputs.iter().cloned());
    }
    match profiles.profile_outputs {
        ProfileOutputs::All => every_output(Vec::new()),
        ProfileOutputs::Focused if !per_output => every_output(changes_every_output()),
        ProfileOutputs::Focused => match focused_output {
            Some(output) => only(&mut std::iter::once(output.to_string())),
            None => every_output(vec![Note::FocusedOutputUnknown {
                profile: profile.name.clone(),
            }]),
        },
    }
}

/// Output names the file uses, in `[output."NAME"]` tables or in a profile's
/// `outputs`, that are not among `connected`. Sorted, each once.
pub fn outputs_not_connected(profiles: &Profiles, connected: &[Output]) -> Vec<String> {
    let mut missing: Vec<String> = profiles
        .outputs
        .keys()
        .chain(
            profiles
                .profiles
                .iter()
                .flat_map(|profile| &profile.outputs),
        )
        .filter(|name| name.as_str() != ALL_OUTPUTS)
        .filter(|name| !connected.iter().any(|output| output.id == **name))
        .cloned()
        .collect();
    missing.sort();
    missing.dedup();
    missing
}

/// One note for each output `profile` names that is not among `connected`.
/// None when the profile names `"all"`, which covers every output anyway.
pub fn profile_outputs_not_connected(
    profiles: &Profiles,
    profile: &Profile,
    connected: &[Output],
) -> Vec<Note> {
    if profile.outputs.iter().any(|name| name == ALL_OUTPUTS) {
        return Vec::new();
    }
    outputs_not_connected(profiles, connected)
        .into_iter()
        .filter(|name| profile.outputs.contains(name))
        .map(|output| Note::OutputNotConnected {
            profile: profile.name.clone(),
            output,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::OutputConfig;
    use crate::MatchRule;

    fn s(value: f32) -> Saturation {
        Saturation::new(value)
    }

    fn levels(base: f32, outputs: &[(&str, f32)]) -> Levels {
        Levels::new(
            s(base),
            outputs
                .iter()
                .map(|(name, value)| (name.to_string(), s(*value))),
        )
    }

    fn profile(outputs: &[&str]) -> Profile {
        Profile {
            name: "game".into(),
            enabled: true,
            saturation: 1.6,
            match_rule: MatchRule {
                exe: Some("game".into()),
                ..MatchRule::default()
            },
            outputs: outputs.iter().map(|name| name.to_string()).collect(),
        }
    }

    /// Desktop 1.0, `HDMI-A-1` 0.85, and `profile_outputs` as given.
    fn profiles(profile_outputs: ProfileOutputs) -> Profiles {
        let mut profiles = Profiles {
            profile_outputs,
            ..Profiles::default()
        };
        profiles.outputs.insert(
            "HDMI-A-1".into(),
            OutputConfig {
                default_saturation: Some(0.85),
            },
        );
        profiles
    }

    /// Profile `outputs`, `profile_outputs`, focused output, expected levels
    /// and notes.
    type PerOutputRow = (
        &'static [&'static str],
        ProfileOutputs,
        Option<&'static str>,
        Levels,
        Vec<Note>,
    );

    /// The same, where the expected levels are the profile's value everywhere.
    type UniformRow = (
        &'static [&'static str],
        ProfileOutputs,
        Option<&'static str>,
        Vec<Note>,
    );

    fn changes_every_output() -> Vec<Note> {
        vec![Note::ProfileChangesEveryOutput {
            profile: "game".into(),
        }]
    }

    #[test]
    fn levels_drop_entries_equal_to_the_base() {
        let picture = levels(1.2, &[("DP-1", 1.2), ("DP-2", 1.5)]);
        assert_eq!(picture.outputs.len(), 1);
        assert_eq!(picture.uniform_value(), None);
        assert_eq!(levels(1.2, &[("DP-1", 1.2)]).uniform_value(), Some(s(1.2)));
        assert!(Levels::uniform(Saturation::IDENTITY).is_identity());
        assert!(!levels(1.0, &[("DP-1", 1.5)]).is_identity());
    }

    /// Every row of the design's table for a backend that can do per-output:
    /// profile `outputs`, `profile_outputs`, focused output, expected picture.
    #[test]
    fn profile_picture_on_a_per_output_backend() {
        use ProfileOutputs::{All, Focused};
        let rows: [PerOutputRow; 7] = [
            // contains "all": every output, whatever the rest says.
            (&["all"], Focused, Some("DP-2"), levels(1.6, &[]), vec![]),
            (&["all", "DP-2"], All, None, levels(1.6, &[]), vec![]),
            // names: each named output, the rest keep their desktop default.
            (
                &["DP-2", "DP-3"],
                Focused,
                Some("HDMI-A-1"),
                levels(1.0, &[("DP-2", 1.6), ("DP-3", 1.6), ("HDMI-A-1", 0.85)]),
                vec![],
            ),
            // empty, "focused", known output: that output only.
            (
                &[],
                Focused,
                Some("DP-2"),
                levels(1.0, &[("DP-2", 1.6), ("HDMI-A-1", 0.85)]),
                vec![],
            ),
            // empty, "focused", unknown output: every output, with a note.
            (
                &[],
                Focused,
                None,
                levels(1.6, &[]),
                vec![Note::FocusedOutputUnknown {
                    profile: "game".into(),
                }],
            ),
            // empty, "all" or absent: every output (v0.4.1 behavior).
            (&[], All, Some("DP-2"), levels(1.6, &[]), vec![]),
            (&[], All, None, levels(1.6, &[]), vec![]),
        ];
        for (outputs, profile_outputs, focused, expected, notes) in rows {
            let picture =
                profile_picture(&profiles(profile_outputs), &profile(outputs), focused, true);
            assert_eq!(
                picture,
                Picture {
                    levels: expected,
                    notes
                },
                "outputs {outputs:?}, {profile_outputs:?}, focused {focused:?}"
            );
        }
    }

    #[test]
    fn a_focused_profile_on_an_output_with_its_own_default_replaces_it() {
        let picture = profile_picture(
            &profiles(ProfileOutputs::Focused),
            &profile(&[]),
            Some("HDMI-A-1"),
            true,
        );
        assert_eq!(picture.levels, levels(1.0, &[("HDMI-A-1", 1.6)]));
    }

    /// The fallback for a profile on a backend that shows one saturation on
    /// every output: every output shows the profile's value, with a note when
    /// the file asked for less.
    #[test]
    fn profile_picture_on_a_backend_without_per_output() {
        use ProfileOutputs::{All, Focused};
        let rows: [UniformRow; 6] = [
            (&["all"], Focused, Some("DP-2"), vec![]),
            (&["DP-2"], All, None, changes_every_output()),
            (&["DP-2"], Focused, Some("DP-2"), changes_every_output()),
            (&[], Focused, Some("DP-2"), changes_every_output()),
            (&[], Focused, None, changes_every_output()),
            (&[], All, Some("DP-2"), vec![]),
        ];
        for (outputs, profile_outputs, focused, notes) in rows {
            let picture = profile_picture(
                &profiles(profile_outputs),
                &profile(outputs),
                focused,
                false,
            );
            assert_eq!(
                picture,
                Picture {
                    levels: levels(1.6, &[]),
                    notes
                },
                "outputs {outputs:?}, {profile_outputs:?}, focused {focused:?}"
            );
        }
    }

    /// The fallback for the desktop on a backend that shows one saturation on
    /// every output: the top-level default everywhere, with a note when the
    /// file sets per-output defaults.
    #[test]
    fn desktop_picture_with_and_without_per_output() {
        let with_defaults = profiles(ProfileOutputs::All);
        assert_eq!(
            desktop_picture(&with_defaults, true),
            Picture {
                levels: levels(1.0, &[("HDMI-A-1", 0.85)]),
                notes: vec![],
            }
        );
        assert_eq!(
            desktop_picture(&with_defaults, false),
            Picture {
                levels: levels(1.0, &[]),
                notes: vec![Note::PerOutputDefaultsNotUsed],
            }
        );

        let plain = Profiles {
            default_saturation: 1.25,
            ..Profiles::default()
        };
        for per_output in [true, false] {
            assert_eq!(
                desktop_picture(&plain, per_output),
                Picture {
                    levels: levels(1.25, &[]),
                    notes: vec![],
                }
            );
        }
    }

    #[test]
    fn an_output_table_without_a_value_changes_nothing() {
        let mut profiles = Profiles::default();
        profiles
            .outputs
            .insert("DP-2".into(), OutputConfig::default());
        assert_eq!(
            desktop_picture(&profiles, false),
            Picture {
                levels: levels(1.0, &[]),
                notes: vec![],
            }
        );
        assert_eq!(desktop_picture(&profiles, true).levels, levels(1.0, &[]));
    }

    #[test]
    fn notes_only_the_profiles_own_names_that_are_not_connected() {
        let mut profiles = profiles(ProfileOutputs::All);
        let game = profile(&["DP-3", "HDMI-A-1"]);
        profiles.profiles = vec![game.clone(), profile(&["DP-9"])];
        let connected = [Output {
            id: "HDMI-A-1".into(),
            human_name: "HDMI-A-1".into(),
        }];
        assert_eq!(
            profile_outputs_not_connected(&profiles, &game, &connected),
            [Note::OutputNotConnected {
                profile: "game".into(),
                output: "DP-3".into()
            }]
        );
        assert!(
            profile_outputs_not_connected(&profiles, &profile(&["all", "DP-3"]), &connected)
                .is_empty()
        );
    }

    #[test]
    fn names_the_file_uses_that_are_not_connected() {
        let mut profiles = profiles(ProfileOutputs::All);
        profiles.profiles = vec![profile(&["all", "DP-3", "HDMI-A-1"]), profile(&["DP-3"])];
        let connected = [Output {
            id: "HDMI-A-1".into(),
            human_name: "HDMI-A-1".into(),
        }];
        assert_eq!(
            outputs_not_connected(&profiles, &connected),
            vec!["DP-3".to_string()]
        );
        assert_eq!(
            outputs_not_connected(&profiles, &[]),
            vec!["DP-3".to_string(), "HDMI-A-1".to_string()]
        );
    }

    /// A file with a general value of 1.3 and `DP-2` at 0.9, and one game.
    fn general_and_game(enabled: bool, outputs: &[&str], follow: ProfileOutputs) -> Profiles {
        let mut file = Profiles {
            default_saturation: 1.3,
            profile_outputs: follow,
            ..Profiles::default()
        };
        file.outputs.insert(
            "DP-2".into(),
            OutputConfig {
                default_saturation: Some(0.9),
            },
        );
        file.profiles.push(Profile {
            enabled,
            ..profile(outputs)
        });
        file
    }

    #[test]
    fn the_four_cases_on_a_backend_with_one_value_for_every_output() {
        // (the game's profile: None = no profile, Some(on?)), expected value.
        let cases: [(&str, Option<bool>, f32); 4] = [
            (
                "a game whose profile is on shows the profile's value",
                Some(true),
                1.6,
            ),
            (
                "a game whose profile is off shows normal colors",
                Some(false),
                1.0,
            ),
            ("a game with no profile shows the general value", None, 1.3),
            (
                "the desktop and Steam's own screens show the general value",
                None,
                1.3,
            ),
        ];
        for (what, game, expected) in cases {
            let file = general_and_game(game.unwrap_or(true), &[], ProfileOutputs::All);
            let matched = game.map(|_| &file.profiles[0]);
            let picture = focus_picture(&file, matched, None, false);
            assert_eq!(picture.levels, Levels::uniform(s(expected)), "{what}");
        }
    }

    #[test]
    fn a_profile_that_is_off_is_normal_on_the_outputs_it_would_change() {
        // Profile `outputs`, `profile_outputs`, focused output, expected.
        // The desktop is 1.3 with DP-2 at 0.9; the profile's own value, 1.6,
        // must appear nowhere.
        let rows: Vec<(&[&str], ProfileOutputs, Option<&str>, Levels)> = vec![
            // Every output: normal everywhere, the per-output default included.
            (&[], ProfileOutputs::All, Some("DP-1"), levels(1.0, &[])),
            (
                &["all"],
                ProfileOutputs::Focused,
                Some("DP-1"),
                levels(1.0, &[]),
            ),
            // The outputs it lists: normal there, the desktop elsewhere.
            (
                &["DP-1"],
                ProfileOutputs::All,
                Some("DP-2"),
                levels(1.3, &[("DP-1", 1.0), ("DP-2", 0.9)]),
            ),
            (
                &["DP-2"],
                ProfileOutputs::All,
                Some("DP-1"),
                levels(1.3, &[("DP-2", 1.0)]),
            ),
            // Following focus: normal on the focused output only.
            (
                &[],
                ProfileOutputs::Focused,
                Some("DP-1"),
                levels(1.3, &[("DP-1", 1.0), ("DP-2", 0.9)]),
            ),
            (
                &[],
                ProfileOutputs::Focused,
                Some("DP-2"),
                levels(1.3, &[("DP-2", 1.0)]),
            ),
            // Following focus with no output reported: every output.
            (&[], ProfileOutputs::Focused, None, levels(1.0, &[])),
        ];
        for (outputs, follow, focused, expected) in rows {
            let file = general_and_game(false, outputs, follow);
            let picture = focus_picture(&file, Some(&file.profiles[0]), focused, true);
            assert_eq!(
                picture.levels, expected,
                "outputs {outputs:?}, {follow:?}, focused {focused:?}"
            );
        }
    }

    #[test]
    fn the_four_cases_on_a_backend_with_a_value_per_output() {
        // The game follows focus and is on DP-1; the desktop is 1.3 with DP-2
        // at 0.9.
        let desktop = levels(1.3, &[("DP-2", 0.9)]);
        let cases: [(&str, Option<bool>, Levels); 4] = [
            (
                "on: the profile's value on the game's output",
                Some(true),
                levels(1.3, &[("DP-1", 1.6), ("DP-2", 0.9)]),
            ),
            (
                "off: normal colors on the game's output",
                Some(false),
                levels(1.3, &[("DP-1", 1.0), ("DP-2", 0.9)]),
            ),
            ("no profile: the desktop picture", None, desktop.clone()),
            (
                "the desktop itself: the desktop picture",
                None,
                desktop.clone(),
            ),
        ];
        for (what, game, expected) in cases {
            let file = general_and_game(game.unwrap_or(true), &[], ProfileOutputs::Focused);
            let matched = game.map(|_| &file.profiles[0]);
            let picture = focus_picture(&file, matched, Some("DP-1"), true);
            assert_eq!(picture.levels, expected, "{what}");
        }
    }

    #[test]
    fn a_profile_that_is_off_says_the_same_notes_as_one_that_is_on() {
        for enabled in [true, false] {
            let file = general_and_game(enabled, &["DP-1"], ProfileOutputs::All);
            let picture = profile_picture(&file, &file.profiles[0], None, false);
            assert_eq!(picture.notes, changes_every_output(), "enabled {enabled}");
        }
    }
}
