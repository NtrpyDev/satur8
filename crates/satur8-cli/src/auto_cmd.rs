//! `satur8 auto on|off|status` - turn automatic per-game switching on or off
//! with the same steps as the desktop app's Status page toggle: the
//! background service, and on KDE Plasma the KWin focus script. In Steam Game
//! Mode it only controls the service, as the app does.

use anyhow::{bail, Result};
use clap::Subcommand;
use satur8_backend::switching::{self as system, RealSystem};
use satur8_core::switching::{self, Action, SwitchFacts, SwitchState};

#[derive(Subcommand, Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoCmd {
    /// Turn automatic per-game switching on: start the background service
    /// now and at login, and on KDE Plasma turn on and start the focus script.
    On,
    /// Turn it off again: the reverse of `on`.
    Off,
    /// Say whether it is on.
    Status,
}

/// What `on` or `off` does with the state found.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Decision {
    /// It cannot be turned on or off in this session; the line says why.
    Unavailable(String),
    /// Nothing to do; the line says so.
    Nothing(String),
    /// Run these steps.
    Run(Vec<Action>),
}

fn decide(on: bool, facts: &SwitchFacts, state: &SwitchState) -> Decision {
    if !state.available {
        return Decision::Unavailable(format!(
            "satur8: automatic per-game switching cannot be changed here: {}",
            state.detail
        ));
    }
    if on && state.on {
        return Decision::Nothing("automatic per-game switching is already on".to_string());
    }
    // Off always runs: a partly-on state still has parts to turn off.
    Decision::Run(if on {
        switching::plan_enable(facts)
    } else {
        switching::plan_disable(facts)
    })
}

/// One printed line per step, as the app's activity log shows them; a failed
/// step's line already ends in "failed: why".
fn step_line(ok: bool, line: &str) -> String {
    if ok {
        format!("  done: {line}")
    } else {
        format!("  {line}")
    }
}

pub fn run(cmd: AutoCmd) -> Result<()> {
    let facts = system::probe();
    let state = switching::switch_state(&facts);
    let on = match cmd {
        AutoCmd::Status => {
            println!("{}", switching::status_line(&state));
            return Ok(());
        }
        AutoCmd::On => true,
        AutoCmd::Off => false,
    };
    let plan = match decide(on, &facts, &state) {
        Decision::Unavailable(line) => bail!(line),
        Decision::Nothing(line) => {
            println!("{line}");
            return Ok(());
        }
        Decision::Run(plan) => plan,
    };
    for (ok, line) in switching::execute(&mut RealSystem, &plan) {
        println!("{}", step_line(ok, &line));
    }
    // `systemctl start` returns before the service is on the bus; the app
    // waits the same 20 seconds before it checks again.
    if switching::plan_starts_daemon(&plan) {
        system::wait_for_daemon(std::time::Duration::from_secs(20));
    }
    let after = switching::switch_state(&system::probe());
    println!("{}", switching::status_line(&after));
    if after.on != on {
        bail!(
            "satur8: automatic per-game switching did not turn {}",
            if on { "on" } else { "off" }
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use satur8_core::switching::UnitState;
    use satur8_core::{Desktop, Environment, Gpu, SessionType};
    use std::path::PathBuf;

    fn kde_facts() -> SwitchFacts {
        SwitchFacts {
            env: Environment {
                session: SessionType::Wayland,
                desktop: Desktop::Kde,
                gpu: Gpu::Amd,
            },
            backend: Some("kwin".into()),
            daemon_running: Some(false),
            unit: UnitState::Disabled,
            script_enabled: false,
            script_loaded: Some(false),
            script_path: Some(PathBuf::from(
                "/usr/share/kwin/scripts/satur8-focus/contents/code/main.js",
            )),
        }
    }

    #[test]
    fn on_runs_the_apps_enable_steps_and_off_the_disable_steps() {
        let facts = kde_facts();
        let state = switching::switch_state(&facts);
        assert_eq!(
            decide(true, &facts, &state),
            Decision::Run(switching::plan_enable(&facts))
        );
        assert_eq!(
            decide(false, &facts, &state),
            Decision::Run(switching::plan_disable(&facts))
        );
        assert!(matches!(
            decide(true, &facts, &state),
            Decision::Run(plan) if plan.contains(&Action::KwinReconfigure)
        ));
    }

    #[test]
    fn on_when_already_on_does_nothing() {
        let facts = SwitchFacts {
            daemon_running: Some(true),
            unit: UnitState::Enabled,
            script_enabled: true,
            script_loaded: Some(true),
            ..kde_facts()
        };
        let state = switching::switch_state(&facts);
        assert!(state.on);
        assert_eq!(
            decide(true, &facts, &state),
            Decision::Nothing("automatic per-game switching is already on".into())
        );
        assert!(matches!(decide(false, &facts, &state), Decision::Run(_)));
    }

    #[test]
    fn game_mode_only_touches_the_service() {
        let facts = SwitchFacts {
            env: Environment {
                session: SessionType::Unknown,
                desktop: Desktop::Other,
                gpu: Gpu::Amd,
            },
            backend: Some("gamescope".into()),
            script_loaded: None,
            script_path: None,
            ..kde_facts()
        };
        let state = switching::switch_state(&facts);
        assert_eq!(
            decide(true, &facts, &state),
            Decision::Run(vec![Action::Systemctl(vec!["enable", "--now"])])
        );
        assert_eq!(
            decide(false, &facts, &state),
            Decision::Run(vec![Action::Systemctl(vec!["disable", "--now"])])
        );
    }

    #[test]
    fn unavailable_session_says_why_and_changes_nothing() {
        let facts = SwitchFacts {
            env: Environment {
                session: SessionType::Wayland,
                desktop: Desktop::Gnome,
                gpu: Gpu::Intel,
            },
            backend: Some("gnome-shell".into()),
            ..kde_facts()
        };
        let state = switching::switch_state(&facts);
        match decide(true, &facts, &state) {
            Decision::Unavailable(line) => {
                assert!(line.starts_with("satur8: automatic per-game switching cannot"));
                assert!(line.contains("Needs KDE Plasma on Wayland"));
            }
            other => panic!("expected unavailable, got {other:?}"),
        }
    }

    #[test]
    fn step_lines_say_done_or_failed() {
        assert_eq!(
            step_line(true, "Ask KDE to reload its settings"),
            "  done: Ask KDE to reload its settings"
        );
        assert_eq!(
            step_line(false, "Start game detection in KDE failed: no bus"),
            "  Start game detection in KDE failed: no bus"
        );
    }

    #[test]
    fn status_line_names_the_state() {
        let state = switching::switch_state(&kde_facts());
        assert_eq!(
            switching::status_line(&state),
            "automatic per-game switching: off (Game profiles are not applied automatically)"
        );
    }
}
