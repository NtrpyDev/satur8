//! What `satur8 outputs`, `satur8 doctor` and `satur8 status` say about
//! per-output settings.
//!
//! Every line is built here by pure functions from a [`BackendView`] and the
//! profiles file, so each case is unit-tested without a backend. The callers
//! only gather the view.
//!
//! `doctor` and `status` print these lines only when the file uses the
//! per-output settings (`profile_outputs` or an `[output."NAME"]` table), so
//! their output for a file that does not use them is unchanged.

use satur8_core::levels::{outputs_not_connected, ALL_OUTPUTS};
use satur8_core::{Levels, Output, ProfileOutputs, Profiles, Saturation};

/// What the selected backend says about its outputs.
pub struct BackendView<'a> {
    pub name: &'a str,
    pub per_output: bool,
    pub outputs: &'a [Output],
    /// What every output shows right now, when the backend can tell.
    pub current: Option<&'a Levels>,
    /// Why one value applies to every output and what to do, when the backend
    /// says (an older KWin effect still loaded, for example).
    pub reason: Option<&'a str>,
}

/// Whether the file uses the per-output settings: `profile_outputs` or an
/// `[output."NAME"]` table. A profile's `outputs` alone is not enough, because
/// files written before these settings existed could already have it.
pub fn uses_per_output_settings(profiles: &Profiles) -> bool {
    profiles.profile_outputs != ProfileOutputs::All || !profiles.outputs.is_empty()
}

fn level_of(levels: &Levels, output: &str) -> Saturation {
    levels.outputs.get(output).copied().unwrap_or(levels.base)
}

/// One `satur8 outputs` row: id and name as before, then what is known about
/// the output, in parentheses.
fn output_row(view: &BackendView, profiles: &Profiles, output: &Output) -> String {
    let mut facts = Vec::new();
    if let Some(current) = view.current {
        facts.push(format!("now {:.2}", level_of(current, &output.id).get()));
    }
    if view.per_output {
        if let Some(value) = profiles
            .outputs
            .get(&output.id)
            .and_then(|config| config.default_saturation)
        {
            facts.push(format!("desktop default {value:.2}"));
        }
    }
    let row = format!("  {:<8} {}", output.id, output.human_name);
    if facts.is_empty() {
        row
    } else {
        format!("{row}  ({})", facts.join(", "))
    }
}

/// Settings in the file that this backend cannot honor, one line each. Empty
/// on a backend that can show a different saturation on each output.
pub fn not_in_use_lines(view: &BackendView, profiles: &Profiles) -> Vec<String> {
    if view.per_output {
        return Vec::new();
    }
    let mut lines: Vec<String> = profiles
        .outputs
        .iter()
        .filter_map(|(name, config)| {
            config.default_saturation.map(|value| {
                format!(
                    "not in use here: [output.\"{name}\"] default_saturation = {value:.2}; \
                     the desktop is default_saturation on every output"
                )
            })
        })
        .collect();
    if profiles.profile_outputs == ProfileOutputs::Focused {
        lines.push(
            "not in use here: profile_outputs = \"focused\"; game profiles change every output"
                .to_string(),
        );
    }
    lines.extend(
        profiles
            .profiles
            .iter()
            .filter(|profile| {
                !profile.outputs.is_empty() && !profile.outputs.iter().any(|o| o == ALL_OUTPUTS)
            })
            .map(|profile| {
                format!(
                    "not in use here: profile '{}' outputs = [{}]; it changes every output",
                    profile.name,
                    profile
                        .outputs
                        .iter()
                        .map(|name| format!("\"{name}\""))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }),
    );
    lines
}

/// Output names the file uses that this backend does not list. Only on a
/// backend that can target one output, where the list is the real outputs.
pub fn not_connected_lines(view: &BackendView, profiles: &Profiles) -> Vec<String> {
    if !view.per_output {
        return Vec::new();
    }
    outputs_not_connected(profiles, view.outputs)
        .into_iter()
        .map(|name| format!("not connected now: {name}"))
        .collect()
}

/// The one line on what the backend can do with outputs.
fn capability_line(view: &BackendView) -> String {
    if let (false, Some(reason)) = (view.per_output, view.reason) {
        return reason.to_string();
    }
    if view.per_output {
        format!(
            "the {} backend can show a different saturation on each output",
            view.name
        )
    } else {
        format!(
            "the {} backend applies one saturation to every output",
            view.name
        )
    }
}

/// What a game profile that lists no outputs changes, on a backend that can
/// target one output.
fn profile_outputs_line(profiles: &Profiles) -> String {
    match profiles.profile_outputs {
        ProfileOutputs::All => "game profiles that list no outputs change every output".to_string(),
        ProfileOutputs::Focused => "game profiles that list no outputs change the output \
                                    the game is on, or every output when that is unknown"
            .to_string(),
    }
}

/// The whole `satur8 outputs` report.
pub fn outputs_lines(view: &BackendView, profiles: &Profiles) -> Vec<String> {
    let mut lines = vec![format!("outputs on the {} backend:", view.name)];
    lines.extend(
        view.outputs
            .iter()
            .map(|output| output_row(view, profiles, output)),
    );
    if view.per_output {
        lines.push(format!("  {}", profile_outputs_line(profiles)));
    } else {
        lines.push(format!("  {}", capability_line(view)));
    }
    lines.extend(
        not_in_use_lines(view, profiles)
            .into_iter()
            .chain(not_connected_lines(view, profiles))
            .map(|line| format!("  {line}")),
    );
    lines
}

/// The per-output section of `satur8 doctor` and `satur8 status`: empty unless
/// the file uses the per-output settings. `view` is `Err` with the reason when
/// no backend could be selected.
pub fn settings_section(view: Result<&BackendView, &str>, profiles: &Profiles) -> Vec<String> {
    if !uses_per_output_settings(profiles) {
        return Vec::new();
    }
    let view = match view {
        Ok(view) => view,
        Err(reason) => {
            return vec![
                "  per-output settings:".to_string(),
                format!("    [..] not checked: no backend reachable ({reason})"),
            ]
        }
    };
    let mut lines = vec![
        format!("  per-output settings (backend {}):", view.name),
        format!(
            "    [{}] {}",
            if view.per_output { "ok" } else { ".." },
            capability_line(view)
        ),
    ];
    lines.extend(
        not_in_use_lines(view, profiles)
            .into_iter()
            .chain(not_connected_lines(view, profiles))
            .map(|line| format!("    [..] {line}")),
    );
    lines
}

/// Lines for `satur8 status` naming each output that shows something other
/// than the base value right now, for example after `satur8 set VALUE --output
/// NAME`. Empty when every output shows the base value, so status prints
/// nothing new then.
///
/// The backend keeps a level for an output that was unplugged, so it comes
/// back as it was. Such a level changes nothing on screen now, and its line
/// ends in `(not connected)`. `connected` is the backend's own list; when it
/// is only the every-output entry, the backend did not list real outputs and
/// nothing is labelled.
pub fn current_output_lines(levels: &Levels, connected: &[Output]) -> Vec<String> {
    let lists_real_outputs = connected.iter().any(|output| output.id != ALL_OUTPUTS);
    levels
        .outputs
        .iter()
        .map(|(name, saturation)| {
            let absent = lists_real_outputs && !connected.iter().any(|output| &output.id == name);
            format!(
                "  output {name}: saturation {:.2}{}",
                saturation.get(),
                if absent { " (not connected)" } else { "" }
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use satur8_backend::all_outputs;
    use satur8_core::{MatchRule, OutputConfig, Profile};

    fn output(id: &str, name: &str) -> Output {
        Output {
            id: id.into(),
            human_name: name.into(),
        }
    }

    fn profile(name: &str, outputs: &[&str]) -> Profile {
        Profile {
            name: name.into(),
            enabled: true,
            saturation: 1.3,
            match_rule: MatchRule {
                exe: Some(name.into()),
                ..MatchRule::default()
            },
            outputs: outputs.iter().map(|o| o.to_string()).collect(),
        }
    }

    /// The design's example: `profile_outputs = "focused"`, two output
    /// defaults, and profiles with no outputs, `"all"`, and one named output.
    fn example() -> Profiles {
        let mut profiles = Profiles {
            profile_outputs: ProfileOutputs::Focused,
            profiles: vec![
                profile("cs2", &[]),
                profile("surround", &["all"]),
                profile("couch", &["HDMI-A-1"]),
            ],
            ..Profiles::default()
        };
        for (name, value) in [("DP-2", 1.0), ("HDMI-A-1", 0.85)] {
            profiles.outputs.insert(
                name.into(),
                OutputConfig {
                    default_saturation: Some(value),
                },
            );
        }
        profiles
    }

    /// A v0.4.1 file: one default, profiles, one of them naming an output.
    fn old_style() -> Profiles {
        Profiles {
            default_saturation: 1.1,
            profiles: vec![profile("cs2", &[]), profile("dota", &["DP-1"])],
            ..Profiles::default()
        }
    }

    fn uniform<'a>(outputs: &'a [Output]) -> BackendView<'a> {
        BackendView {
            name: "gnome-shell",
            per_output: false,
            outputs,
            current: None,
            reason: None,
        }
    }

    #[test]
    fn outputs_on_a_uniform_backend_keep_the_old_rows_and_say_so() {
        let outputs = [all_outputs()];
        assert_eq!(
            outputs_lines(&uniform(&outputs), &old_style()),
            [
                "outputs on the gnome-shell backend:",
                "  all      All outputs",
                "  the gnome-shell backend applies one saturation to every output",
                "  not in use here: profile 'dota' outputs = [\"DP-1\"]; it changes every output",
            ]
        );
    }

    #[test]
    fn outputs_on_a_uniform_backend_list_each_setting_not_in_use() {
        let outputs = [all_outputs()];
        assert_eq!(
            outputs_lines(&uniform(&outputs), &example()),
            [
                "outputs on the gnome-shell backend:",
                "  all      All outputs",
                "  the gnome-shell backend applies one saturation to every output",
                "  not in use here: [output.\"DP-2\"] default_saturation = 1.00; \
                 the desktop is default_saturation on every output",
                "  not in use here: [output.\"HDMI-A-1\"] default_saturation = 0.85; \
                 the desktop is default_saturation on every output",
                "  not in use here: profile_outputs = \"focused\"; game profiles change every output",
                "  not in use here: profile 'couch' outputs = [\"HDMI-A-1\"]; it changes every output",
            ]
        );
    }

    #[test]
    fn outputs_on_a_per_output_backend_show_levels_defaults_and_missing_names() {
        let outputs = [output("DP-2", "Dell U2720Q"), output("DP-3", "LG 27GP850")];
        let current = Levels::new(
            Saturation::new(1.0),
            [("DP-3".to_string(), Saturation::new(1.6))],
        );
        let view = BackendView {
            name: "kwin",
            per_output: true,
            outputs: &outputs,
            current: Some(&current),
            reason: None,
        };
        assert_eq!(
            outputs_lines(&view, &example()),
            [
                "outputs on the kwin backend:",
                "  DP-2     Dell U2720Q  (now 1.00, desktop default 1.00)",
                "  DP-3     LG 27GP850  (now 1.60)",
                "  game profiles that list no outputs change the output the game is on, \
                 or every output when that is unknown",
                "  not connected now: HDMI-A-1",
            ]
        );
    }

    #[test]
    fn outputs_on_a_per_output_backend_with_no_settings() {
        let outputs = [output("31", "CRTC 31")];
        let view = BackendView {
            name: "drm-ctm",
            per_output: true,
            outputs: &outputs,
            current: None,
            reason: None,
        };
        assert_eq!(
            outputs_lines(&view, &Profiles::default()),
            [
                "outputs on the drm-ctm backend:",
                "  31       CRTC 31",
                "  game profiles that list no outputs change every output",
            ]
        );
    }

    /// `doctor` and `status` print nothing new for a file that does not use the
    /// per-output settings, whatever the backend, and do not even need one.
    #[test]
    fn doctor_and_status_add_nothing_for_an_old_style_file() {
        let outputs = [all_outputs()];
        assert!(settings_section(Ok(&uniform(&outputs)), &old_style()).is_empty());
        assert!(settings_section(Err("no backend"), &old_style()).is_empty());
        assert!(settings_section(Err("no backend"), &Profiles::default()).is_empty());
    }

    #[test]
    fn doctor_and_status_report_settings_a_uniform_backend_cannot_honor() {
        let outputs = [all_outputs()];
        let mut profiles = old_style();
        profiles.outputs.insert(
            "HDMI-A-1".into(),
            OutputConfig {
                default_saturation: Some(0.85),
            },
        );
        assert_eq!(
            settings_section(Ok(&uniform(&outputs)), &profiles),
            [
                "  per-output settings (backend gnome-shell):",
                "    [..] the gnome-shell backend applies one saturation to every output",
                "    [..] not in use here: [output.\"HDMI-A-1\"] default_saturation = 0.85; \
                 the desktop is default_saturation on every output",
                "    [..] not in use here: profile 'dota' outputs = [\"DP-1\"]; it changes every output",
            ]
        );
    }

    #[test]
    fn doctor_and_status_list_names_not_connected_on_a_per_output_backend() {
        let outputs = [output("DP-2", "Dell U2720Q")];
        let view = BackendView {
            name: "kwin",
            per_output: true,
            outputs: &outputs,
            current: None,
            reason: None,
        };
        assert_eq!(
            settings_section(Ok(&view), &example()),
            [
                "  per-output settings (backend kwin):",
                "    [ok] the kwin backend can show a different saturation on each output",
                "    [..] not connected now: HDMI-A-1",
            ]
        );
    }

    #[test]
    fn doctor_and_status_say_when_no_backend_could_check_the_settings() {
        assert_eq!(
            settings_section(Err("the session has no display backend"), &example()),
            [
                "  per-output settings:",
                "    [..] not checked: no backend reachable (the session has no display backend)",
            ]
        );
    }

    const OLDER_EFFECT: &str = "KDE is still running the Satur8 effect from before the upgrade, \
                                so one value applies to every output. Log out and back in once \
                                to use a different value per output.";

    fn older_kwin<'a>(outputs: &'a [Output]) -> BackendView<'a> {
        BackendView {
            name: "kwin",
            per_output: false,
            outputs,
            current: None,
            reason: Some(OLDER_EFFECT),
        }
    }

    #[test]
    fn outputs_with_an_older_effect_say_why_and_what_to_do() {
        let outputs = [all_outputs()];
        assert_eq!(
            outputs_lines(&older_kwin(&outputs), &Profiles::default()),
            [
                "outputs on the kwin backend:".to_string(),
                "  all      All outputs".to_string(),
                format!("  {OLDER_EFFECT}"),
            ]
        );
    }

    #[test]
    fn doctor_and_status_say_why_with_an_older_effect() {
        let outputs = [all_outputs()];
        let section = settings_section(Ok(&older_kwin(&outputs)), &example());
        assert_eq!(section[0], "  per-output settings (backend kwin):");
        assert_eq!(section[1], format!("    [..] {OLDER_EFFECT}"));
        // Still nothing for a file without per-output settings.
        assert!(settings_section(Ok(&older_kwin(&outputs)), &old_style()).is_empty());
    }

    #[test]
    fn status_names_only_the_outputs_that_differ_from_the_base() {
        let uniform = Levels::uniform(Saturation::new(1.5));
        let connected = [output("eDP-1", "Built-in"), output("DP-2", "Dell")];
        assert!(current_output_lines(&uniform, &connected).is_empty());
        let mixed = Levels::new(
            Saturation::IDENTITY,
            [
                ("eDP-1".to_string(), Saturation::new(0.0)),
                ("DP-2".to_string(), Saturation::new(2.0)),
                ("DP-3".to_string(), Saturation::IDENTITY),
            ],
        );
        assert_eq!(
            current_output_lines(&mixed, &connected),
            vec![
                "  output DP-2: saturation 2.00".to_string(),
                "  output eDP-1: saturation 0.00".to_string(),
            ]
        );
    }

    #[test]
    fn status_labels_a_level_kept_for_an_output_that_is_not_connected() {
        let kept = Levels::new(
            Saturation::IDENTITY,
            [
                ("eDP-1".to_string(), Saturation::new(1.2)),
                ("HDMI-A-1".to_string(), Saturation::new(0.85)),
            ],
        );
        // HDMI-A-1 was unplugged; the backend still holds its level.
        assert_eq!(
            current_output_lines(&kept, &[output("eDP-1", "Built-in")]),
            vec![
                "  output HDMI-A-1: saturation 0.85 (not connected)".to_string(),
                "  output eDP-1: saturation 1.20".to_string(),
            ]
        );
        // A backend that lists no real outputs cannot say which are
        // connected, so nothing is labelled.
        assert_eq!(
            current_output_lines(&kept, &[all_outputs()]),
            vec![
                "  output HDMI-A-1: saturation 0.85".to_string(),
                "  output eDP-1: saturation 1.20".to_string(),
            ]
        );
        assert_eq!(current_output_lines(&kept, &[]).len(), 2);
        assert!(!current_output_lines(&kept, &[])[0].contains("not connected"));
    }
}
