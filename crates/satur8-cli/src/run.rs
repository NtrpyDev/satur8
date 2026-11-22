//! The launch wrapper: `satur8 run [...] -- <game command>`.
//!
//! This is the primary, zero-polling trigger (PLAN.md section 6). We apply the
//! saturation, spawn the game, wait, and restore on exit - there is no watcher
//! process running during play. It is the Steam launch-option path:
//!
//!   satur8 run --profile cs2 -- %command%
//!
//! Restore is best-effort-guaranteed: whatever ends the game (normal exit,
//! Ctrl-C, or Steam's SIGTERM), we forward the signal to the game and always
//! reset the backend once it has exited.

use anyhow::{bail, Context, Result};
use satur8_backend::{install_signal_handler, select_backend, RestoreGuard};
use satur8_core::levels::{desktop_picture, profile_outputs_not_connected, profile_picture};
use satur8_core::{Levels, Note, Picture, Profile, Profiles, Saturation};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::config;

pub struct RunArgs {
    pub profile: Option<String>,
    pub saturation: Option<f32>,
    /// Force a specific run strategy. Currently "gamescope" or "gamescope-native".
    pub via: Option<String>,
    /// Extra args passed to gamescope before `--` (e.g. -W 2560 -H 1440).
    pub gamescope_args: Vec<String>,
    pub command: Vec<String>,
}

struct ResolvedSaturation {
    saturation: Saturation,
    /// The profile the saturation came from, when it came from one.
    profile: Option<Profile>,
}

#[cfg(test)]
impl ResolvedSaturation {
    /// The outputs the profile names; none for an explicit saturation.
    fn outputs(&self) -> &[String] {
        self.profile
            .as_ref()
            .map_or(&[], |profile| &profile.outputs)
    }
}

/// What the outputs show while the game runs. There is no focused window to
/// follow, so a profile that would follow focus changes every output.
fn launch_picture(profiles: &Profiles, resolved: &ResolvedSaturation, per_output: bool) -> Picture {
    match &resolved.profile {
        Some(profile) => profile_picture(profiles, profile, None, per_output),
        None => Picture {
            levels: Levels::uniform(resolved.saturation),
            notes: Vec::new(),
        },
    }
}

/// What to tell the user in a `--via gamescope` mode, which shows one
/// saturation on every output: the same notes the normal path prints for a
/// backend that cannot target one output.
fn one_level_notes(profiles: &Profiles, resolved: Option<&ResolvedSaturation>) -> Vec<Note> {
    resolved
        .map(|resolved| launch_picture(profiles, resolved, false).notes)
        .unwrap_or_default()
}

pub fn run(args: RunArgs) -> Result<i32> {
    if args.command.is_empty() {
        bail!("nothing to run. Usage: satur8 run [--profile NAME | --saturation S] -- <command>");
    }

    let profiles = config::load_profiles().unwrap_or_default();
    let resolved = resolve_saturation(&profiles, &args)?;

    // SteamOS/Bazzite path: drive the running gamescope compositor at runtime.
    if args.via.as_deref() == Some("gamescope-native") {
        for note in one_level_notes(&profiles, resolved.as_ref()) {
            eprintln!("satur8: {note}");
        }
        let sat = resolved
            .as_ref()
            .map(|resolved| resolved.saturation)
            .unwrap_or(Saturation::IDENTITY);
        let mut session = satur8_gamescope::native_session()
            .context("connecting to the running gamescope compositor")?;
        session
            .apply(sat)
            .context("applying native gamescope LUT override")?;
        eprintln!(
            "satur8: {:.2} via gamescope-native (running compositor), launching {}",
            sat.get(),
            args.command[0]
        );
        return launch_with_restore(&args.command, move || {
            if let Err(e) = session.restore() {
                eprintln!("satur8: warning, failed to restore gamescope color state: {e}");
            }
        });
    }

    // The gamescope fallback wraps the whole launch in a nested compositor and
    // exits with the game, so it bypasses the apply/restore backend entirely.
    if args.via.as_deref() == Some("gamescope") {
        for note in one_level_notes(&profiles, resolved.as_ref()) {
            eprintln!("satur8: {note}");
        }
        let sat = resolved
            .as_ref()
            .map(|resolved| resolved.saturation)
            .unwrap_or(Saturation::IDENTITY);
        eprintln!(
            "satur8: {:.2} via gamescope (nested compositor: extra pass + latency), launching {}",
            sat.get(),
            args.command[0]
        );
        return satur8_gamescope::run(sat, &args.gamescope_args, &args.command)
            .context("running via gamescope");
    }
    if let Some(other) = &args.via {
        bail!("unknown --via '{other}' (supported: gamescope, gamescope-native)");
    }

    // Apply before launch (if we have something to apply).
    let selected = select_backend()?;
    let per_output = selected.per_output();
    let desktop = desktop_picture(&profiles, per_output);
    let mut backend = RestoreGuard::new(selected, desktop.levels);
    if let Some(resolved) = resolved {
        let picture = launch_picture(&profiles, &resolved, per_output);
        let not_connected = match (&resolved.profile, per_output) {
            (Some(profile), true) => {
                profile_outputs_not_connected(&profiles, profile, &backend.backend().outputs())
            }
            _ => Vec::new(),
        };
        for note in desktop
            .notes
            .iter()
            .chain(&picture.notes)
            .chain(&not_connected)
        {
            eprintln!("satur8: {note}");
        }
        backend
            .backend_mut()
            .apply_levels(&picture.levels)
            .context("applying saturation before launch")?;
        eprintln!(
            "satur8: {:.2} via {} backend, launching {}",
            resolved.saturation.get(),
            backend.backend().name(),
            args.command[0]
        );
    } else {
        eprintln!(
            "satur8: no matching profile and no --saturation; launching {} without changes",
            args.command[0]
        );
    }

    // Launch the game as a child. It shares our process group, so an interactive
    // Ctrl-C reaches it directly; we additionally forward INT, TERM, and HUP
    // signals that arrive at the wrapper on to the child.
    let mut child = match Command::new(&args.command[0])
        .args(&args.command[1..])
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return Err(e).with_context(|| format!("launching {}", args.command[0])),
    };
    let pid = child.id() as libc::pid_t;
    let child_running = Arc::new(AtomicBool::new(true));
    let signal_running = child_running.clone();
    let signal_handler = install_signal_handler(move |signal| {
        if signal_running.load(Ordering::Acquire) {
            // Forward to the game; when it exits, child.wait() below returns and
            // we restore. SAFETY: kill() with a known pid and signal number.
            unsafe {
                libc::kill(pid, signal);
            }
        }
    })
    .context("installing signal handlers")?;

    let status = child.wait().context("waiting for game to exit");
    child_running.store(false, Ordering::Release);

    // Stop forwarding before restoring, whatever ended the child.
    drop(signal_handler);
    if let Err(error) = backend.restore_now() {
        eprintln!("satur8: warning, failed to restore saturation: {error}");
    }

    let code = status?.code().unwrap_or(1);
    Ok(code)
}

fn launch_with_restore<F>(command: &[String], restore: F) -> Result<i32>
where
    F: FnOnce(),
{
    let mut restore = Some(restore);
    let mut child = match Command::new(&command[0]).args(&command[1..]).spawn() {
        Ok(c) => c,
        Err(e) => {
            if let Some(restore) = restore.take() {
                restore();
            }
            return Err(e).with_context(|| format!("launching {}", command[0]));
        }
    };
    let pid = child.id() as libc::pid_t;

    let child_running = Arc::new(AtomicBool::new(true));
    let signal_running = child_running.clone();
    let signal_handler = install_signal_handler(move |signal| {
        if signal_running.load(Ordering::Acquire) {
            unsafe {
                libc::kill(pid, signal);
            }
        }
    })
    .context("installing signal handlers")?;

    let status = child.wait().context("waiting for game to exit");
    child_running.store(false, Ordering::Release);

    drop(signal_handler);
    if let Some(restore) = restore.take() {
        restore();
    }

    Ok(status?.code().unwrap_or(1))
}

/// Decide the saturation to apply, in priority order:
/// explicit `--saturation` > named `--profile` > match the command's exe.
/// A profile that is off, named or matched, gives normal colors (1.0): the
/// same rule the daemon follows. No profile at all gives `None`.
fn resolve_saturation(profiles: &Profiles, args: &RunArgs) -> Result<Option<ResolvedSaturation>> {
    if let Some(s) = args.saturation {
        return Ok(Some(ResolvedSaturation {
            saturation: Saturation::try_new(s)?,
            profile: None,
        }));
    }
    if let Some(name) = &args.profile {
        let p = profiles
            .by_name(name)
            .with_context(|| format!("no profile named '{name}' (see `satur8 profile list`)"))?;
        say_if_off(p);
        return Ok(Some(ResolvedSaturation {
            saturation: p.shown_saturation(),
            profile: Some(p.clone()),
        }));
    }
    // Try to auto-match by the launched executable's basename.
    let exe = exe_basename(&args.command[0]);
    if let Some(p) = profiles.match_exe(&exe) {
        eprintln!("satur8: matched profile '{}' by exe '{exe}'", p.name);
        say_if_off(p);
        return Ok(Some(ResolvedSaturation {
            saturation: p.shown_saturation(),
            profile: Some(p.clone()),
        }));
    }
    Ok(None)
}

fn say_if_off(profile: &Profile) {
    if !profile.enabled {
        eprintln!(
            "satur8: profile '{}' is disabled, so the game gets normal colors \
             (turn it on with `satur8 profile enable {}`)",
            profile.name, profile.name
        );
    }
}

fn exe_basename(cmd: &str) -> String {
    Path::new(cmd)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| cmd.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use satur8_core::{Backend, BackendError, CostNote, MatchRule, Output, Profile};

    #[derive(Default)]
    struct FakeBackend {
        applied_outputs: Vec<String>,
    }

    impl Backend for FakeBackend {
        fn name(&self) -> &'static str {
            "fake"
        }

        fn cost(&self) -> CostNote {
            CostNote::ZeroCost
        }

        fn outputs(&self) -> Vec<Output> {
            Vec::new()
        }

        fn apply(&mut self, output: &Output, _: Saturation) -> Result<(), BackendError> {
            self.applied_outputs.push(output.id.clone());
            Ok(())
        }

        fn reset(&mut self, _: &Output) -> Result<(), BackendError> {
            Ok(())
        }
    }

    fn profiles() -> Profiles {
        Profiles {
            default_saturation: 1.0,
            profiles: vec![
                Profile {
                    name: "named".into(),
                    enabled: true,
                    saturation: 1.4,
                    match_rule: MatchRule {
                        exe: Some("named-exe".into()),
                        window_class: None,
                        steam_app_id: None,
                    },
                    outputs: vec!["DP-1".into(), "HDMI-A-1".into()],
                },
                Profile {
                    name: "auto".into(),
                    enabled: true,
                    saturation: 1.8,
                    match_rule: MatchRule {
                        exe: Some("auto-game".into()),
                        window_class: None,
                        steam_app_id: None,
                    },
                    outputs: vec![],
                },
            ],
            ..Profiles::default()
        }
    }

    fn args(profile: Option<&str>, saturation: Option<f32>, command: &str) -> RunArgs {
        RunArgs {
            profile: profile.map(str::to_string),
            saturation,
            via: None,
            gamescope_args: vec![],
            command: vec![command.into()],
        }
    }

    #[test]
    fn resolve_saturation_prefers_explicit_saturation() {
        let args = args(Some("named"), Some(2.2), "/games/auto-game");
        let resolved = resolve_saturation(&profiles(), &args).unwrap().unwrap();
        assert_eq!(resolved.saturation.get(), 2.2);
        assert!(resolved.outputs().is_empty());
    }

    #[test]
    fn resolve_saturation_uses_named_profile_before_exe_match() {
        let args = args(Some("named"), None, "/games/auto-game");
        let resolved = resolve_saturation(&profiles(), &args).unwrap().unwrap();
        assert_eq!(resolved.saturation.get(), 1.4);
        assert_eq!(resolved.outputs(), ["DP-1", "HDMI-A-1"]);
    }

    #[test]
    fn resolve_saturation_auto_matches_command_exe() {
        let args = args(None, None, "/games/auto-game");
        let resolved = resolve_saturation(&profiles(), &args).unwrap().unwrap();
        assert_eq!(resolved.saturation.get(), 1.8);
    }

    #[test]
    fn a_profile_that_is_off_gives_normal_colors_named_or_matched() {
        let mut profiles = profiles();
        profiles.default_saturation = 1.3;
        for profile in &mut profiles.profiles {
            profile.enabled = false;
        }
        // Named with --profile: still that profile, with its outputs, at 1.0.
        let named = args(Some("named"), None, "/games/other");
        let resolved = resolve_saturation(&profiles, &named).unwrap().unwrap();
        assert_eq!(resolved.saturation, Saturation::IDENTITY);
        assert_eq!(resolved.outputs(), ["DP-1", "HDMI-A-1"]);
        assert_eq!(
            launch_picture(&profiles, &resolved, false).levels,
            Levels::uniform(Saturation::IDENTITY)
        );
        // Matched by the command's program: the same.
        let matched = args(None, None, "/games/auto-game");
        let resolved = resolve_saturation(&profiles, &matched).unwrap().unwrap();
        assert_eq!(resolved.saturation, Saturation::IDENTITY);
        assert_eq!(
            launch_picture(&profiles, &resolved, true).levels,
            Levels::uniform(Saturation::IDENTITY)
        );
        // An explicit --saturation is not a profile and is used as given.
        let explicit = args(Some("named"), Some(2.2), "/games/auto-game");
        let resolved = resolve_saturation(&profiles, &explicit).unwrap().unwrap();
        assert_eq!(resolved.saturation.get(), 2.2);
        // A program with no profile is still no match.
        let none = args(None, None, "/games/missing");
        assert!(resolve_saturation(&profiles, &none).unwrap().is_none());
    }

    #[test]
    fn resolve_saturation_returns_none_without_match() {
        let args = args(None, None, "/games/missing");
        assert!(resolve_saturation(&profiles(), &args).unwrap().is_none());
    }

    #[test]
    fn resolve_saturation_errors_for_missing_named_profile() {
        let args = args(Some("missing"), None, "/games/auto-game");
        assert!(resolve_saturation(&profiles(), &args).is_err());
    }

    #[test]
    fn exe_basename_strips_paths_and_keeps_plain_names() {
        assert_eq!(exe_basename("/home/noah/Games/cs2"), "cs2");
        assert_eq!(exe_basename("cs2"), "cs2");
    }

    /// Rewritten for per-output support, like the daemon's test of the same
    /// name: a backend that shows one saturation on every output gets the
    /// profile once, on every output, and the user is told.
    #[test]
    fn a_profile_naming_outputs_changes_every_output_on_a_uniform_backend() {
        let mut backend = FakeBackend::default();
        let args = args(Some("named"), None, "/games/auto-game");
        let resolved = resolve_saturation(&profiles(), &args).unwrap().unwrap();

        let picture = launch_picture(&profiles(), &resolved, backend.per_output());
        backend.apply_levels(&picture.levels).unwrap();

        assert_eq!(backend.applied_outputs, ["all"]);
        assert_eq!(
            picture.notes,
            [satur8_core::Note::ProfileChangesEveryOutput {
                profile: "named".into()
            }]
        );
    }

    #[test]
    fn an_explicit_saturation_changes_every_output() {
        let args = args(None, Some(2.2), "/games/auto-game");
        let resolved = resolve_saturation(&profiles(), &args).unwrap().unwrap();
        let picture = launch_picture(&profiles(), &resolved, true);
        assert_eq!(picture.levels, Levels::uniform(Saturation::new(2.2)));
        assert!(picture.notes.is_empty());
    }

    #[test]
    fn the_gamescope_modes_say_when_a_profile_asks_for_one_output() {
        let profiles = profiles();
        let named = resolve_saturation(&profiles, &args(Some("named"), None, "/games/x"))
            .unwrap()
            .unwrap();
        assert_eq!(
            one_level_notes(&profiles, Some(&named)),
            [Note::ProfileChangesEveryOutput {
                profile: "named".into()
            }]
        );

        let mut focused = profiles.clone();
        focused.profile_outputs = satur8_core::ProfileOutputs::Focused;
        let auto = resolve_saturation(&focused, &args(None, None, "/games/auto-game"))
            .unwrap()
            .unwrap();
        assert_eq!(
            one_level_notes(&focused, Some(&auto)),
            [Note::ProfileChangesEveryOutput {
                profile: "auto".into()
            }]
        );
    }

    #[test]
    fn the_gamescope_modes_say_nothing_extra_without_output_settings() {
        let profiles = profiles();
        let auto = resolve_saturation(&profiles, &args(None, None, "/games/auto-game"))
            .unwrap()
            .unwrap();
        assert!(one_level_notes(&profiles, Some(&auto)).is_empty());
        let explicit = resolve_saturation(&profiles, &args(None, Some(2.0), "/games/x"))
            .unwrap()
            .unwrap();
        assert!(one_level_notes(&profiles, Some(&explicit)).is_empty());
        assert!(one_level_notes(&profiles, None).is_empty());
    }
}
