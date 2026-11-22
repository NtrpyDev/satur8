//! Automatic per-game switching: whether it is on, and the steps that turn it
//! on or off. Shared by the desktop app's Status page toggle and
//! `satur8 auto`. Everything here is pure; the steps are carried out by a
//! [`System`] (the real one is `satur8_backend::switching::RealSystem`).
//!
//! On KDE Plasma, switching is three parts: the background service, the KWin
//! focus script turned on in kwinrc, and that script running in KWin. In Steam
//! Game Mode the service follows gamescope's focus by itself, so switching is
//! only the service.

use std::path::PathBuf;

use crate::{Desktop, Environment, SessionType};

/// The daemon's systemd user unit (installed by install.sh and the packages).
pub const DAEMON_UNIT: &str = "satur8-daemon.service";
/// The KWin script id (assets/kwin-script/metadata.json).
pub const FOCUS_SCRIPT: &str = "satur8-focus";
/// kwinrc key that enables the script at login (packaging/install.sh).
pub const FOCUS_KEY: &str = "satur8-focusEnabled";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitState {
    Enabled,
    Disabled,
    NotInstalled,
    /// systemctl could not answer (no user manager reachable).
    Unknown,
}

/// What switching depends on, gathered read-only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchFacts {
    pub env: Environment,
    /// Name of the backend Satur8 would use, if any.
    pub backend: Option<String>,
    /// Whether org.satur8.Daemon has an owner; `None` if the bus is unreachable.
    pub daemon_running: Option<bool>,
    pub unit: UnitState,
    /// kwinrc [Plugins] satur8-focusEnabled.
    pub script_enabled: bool,
    /// KWin's isScriptLoaded; `None` if KWin did not answer or this is not KDE.
    pub script_loaded: Option<bool>,
    /// The installed focus script's main.js, if found.
    pub script_path: Option<PathBuf>,
}

pub fn kde_wayland(env: &Environment) -> bool {
    env.session == SessionType::Wayland && env.desktop == Desktop::Kde
}

/// Steam Game Mode: the backend Satur8 selected is gamescope. Selection only
/// reaches gamescope when no desktop backend answered, so this is never true
/// in a KDE Plasma, GNOME, Hyprland or NVIDIA X11 session. There the daemon
/// follows gamescope's focus by itself: no KWin effect, no KWin script.
pub fn game_mode(backend: Option<&str>) -> bool {
    backend == Some("gamescope")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchState {
    /// Switching can be turned on or off in this session.
    pub available: bool,
    /// Every part of per-game switching is on right now.
    pub on: bool,
    /// One line saying why or what is done.
    pub detail: String,
}

/// The line when switching is not fully on: what is done.
fn partly_detail(done: &[&str]) -> String {
    if done.is_empty() {
        "Off. Game profiles are not applied automatically".to_string()
    } else {
        format!("Partly on ({}). Turn on to finish", done.join(", "))
    }
}

/// Steam Game Mode, where switching is only the user service.
fn game_mode_switch_state(facts: &SwitchFacts) -> SwitchState {
    if facts.unit == UnitState::NotInstalled {
        return SwitchState {
            available: false,
            on: false,
            detail: "The background service is not installed; reinstall Satur8".into(),
        };
    }
    let running = facts.daemon_running == Some(true);
    if running && facts.unit != UnitState::Disabled {
        return SwitchState {
            available: true,
            on: true,
            detail: "Each game's profile applies when its window is focused".into(),
        };
    }
    let mut done = Vec::new();
    if running {
        done.push("service running");
    }
    if facts.unit == UnitState::Enabled {
        done.push("service starts at login");
    }
    SwitchState {
        available: true,
        on: false,
        detail: partly_detail(&done),
    }
}

/// Derive the state from the real system (never from a stored flag).
pub fn switch_state(facts: &SwitchFacts) -> SwitchState {
    if game_mode(facts.backend.as_deref()) {
        return game_mode_switch_state(facts);
    }
    if !kde_wayland(&facts.env) {
        return SwitchState {
            available: false,
            on: false,
            detail: format!(
                "Needs KDE Plasma on Wayland; this session is {} / {}",
                facts.env.session, facts.env.desktop
            ),
        };
    }
    if facts.script_path.is_none() {
        return SwitchState {
            available: false,
            on: false,
            detail: "The focus script is not installed; reinstall Satur8".into(),
        };
    }
    if facts.unit == UnitState::NotInstalled {
        return SwitchState {
            available: false,
            on: false,
            detail: "The background service is not installed; reinstall Satur8".into(),
        };
    }

    let running = facts.daemon_running == Some(true);
    let at_login = facts.unit != UnitState::Disabled;
    let loaded = facts.script_loaded == Some(true);
    let on = running && at_login && facts.script_enabled && loaded;
    if on {
        return SwitchState {
            available: true,
            on: true,
            detail: "Each game's profile applies when its window is focused".into(),
        };
    }

    let mut done = Vec::new();
    if running {
        done.push("service running");
    }
    if facts.unit == UnitState::Enabled {
        done.push("service starts at login");
    }
    if facts.script_enabled {
        done.push("detection turned on");
    }
    if loaded {
        done.push("detection running");
    }
    SwitchState {
        available: true,
        on: false,
        detail: partly_detail(&done),
    }
}

/// One system-changing step. Only a real [`System`] executes these for real.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// What `satur8 repair` does (satur8_kwin::repair::repair).
    RepairEffect,
    /// `systemctl --user <args> satur8-daemon.service`.
    Systemctl(Vec<&'static str>),
    /// kwinrc [Plugins] satur8-focusEnabled = value, via kwriteconfig6.
    SetFocusEnabled(bool),
    /// Ask the running KWin to reread its configuration.
    KwinReconfigure,
    /// Load and start the focus script in the running KWin, unless loaded.
    LoadScript(PathBuf),
    /// Unload the focus script from the running KWin.
    UnloadScript,
}

impl Action {
    /// The step in plain words: a line of the app's activity list and of
    /// `satur8 auto`. The commands behind the steps are in the README.
    pub fn describe(&self) -> String {
        match self {
            Action::RepairEffect => "Check and repair the saturation effect".into(),
            Action::Systemctl(args) => {
                let now = args.contains(&"--now");
                match args.first().copied() {
                    Some("enable") if now => {
                        "Turn on the background service, now and at every start".into()
                    }
                    Some("disable") if now => {
                        "Turn off the background service, now and at every start".into()
                    }
                    Some("start") => "Start the background service".into(),
                    Some("stop") => "Stop the background service".into(),
                    _ => format!("Background service: {}", args.join(" ")),
                }
            }
            Action::SetFocusEnabled(true) => "Turn on game detection in KDE's settings".into(),
            Action::SetFocusEnabled(false) => "Turn off game detection in KDE's settings".into(),
            Action::KwinReconfigure => "Ask KDE to reload its settings".into(),
            Action::LoadScript(_) => "Start game detection in KDE".into(),
            Action::UnloadScript => "Stop game detection in KDE".into(),
        }
    }
}

/// The installer's three steps, plus loading the script into the running
/// KWin (reconfigure alone does not hot-load it). In Game Mode only the
/// service step applies.
pub fn plan_enable(facts: &SwitchFacts) -> Vec<Action> {
    if game_mode(facts.backend.as_deref()) {
        return vec![Action::Systemctl(vec!["enable", "--now"])];
    }
    let mut plan = vec![
        Action::Systemctl(vec!["enable", "--now"]),
        Action::SetFocusEnabled(true),
        Action::KwinReconfigure,
    ];
    if facts.script_loaded != Some(true) {
        if let Some(path) = &facts.script_path {
            plan.push(Action::LoadScript(path.clone()));
        }
    }
    plan
}

/// The reverse of [`plan_enable`].
pub fn plan_disable(facts: &SwitchFacts) -> Vec<Action> {
    if game_mode(facts.backend.as_deref()) {
        return vec![Action::Systemctl(vec!["disable", "--now"])];
    }
    vec![
        Action::SetFocusEnabled(false),
        Action::KwinReconfigure,
        Action::UnloadScript,
        Action::Systemctl(vec!["disable", "--now"]),
    ]
}

/// Whether a plan starts the background service. `systemctl start` returns
/// once the process is forked, before the daemon has claimed its bus name, so
/// a re-check right after it would still report "Not running".
pub fn plan_starts_daemon(plan: &[Action]) -> bool {
    plan.iter().any(|action| match action {
        Action::Systemctl(args) => {
            args.contains(&"start") || (args.contains(&"enable") && args.contains(&"--now"))
        }
        _ => false,
    })
}

/// Something that can carry out an [`Action`].
pub trait System {
    fn run(&mut self, action: &Action) -> Result<String, String>;
}

/// Run every step, in order, and return one log line per step. A failed step
/// does not stop the later ones; each one reports for itself.
pub fn execute(system: &mut dyn System, plan: &[Action]) -> Vec<(bool, String)> {
    plan.iter()
        .map(|action| match system.run(action) {
            Ok(note) if note.is_empty() => (true, action.describe()),
            Ok(note) => (true, format!("{}: {note}", action.describe())),
            Err(error) => (false, format!("{} failed: {error}", action.describe())),
        })
        .collect()
}

/// Map `systemctl --user is-enabled` output to a [`UnitState`].
pub fn parse_unit_state(stdout: &str, stderr: &str) -> UnitState {
    match stdout.trim() {
        "enabled" | "enabled-runtime" | "static" | "linked" | "linked-runtime" | "alias" => {
            UnitState::Enabled
        }
        "disabled" | "indirect" | "masked" | "masked-runtime" => UnitState::Disabled,
        "not-found" => UnitState::NotInstalled,
        // Older systemd: "Failed to get unit file state for X: No such file
        // or directory". A failed connection to the user manager also says
        // "No such file or directory", so only this exact phrase counts.
        _ if stderr.contains("Failed to get unit file state") => UnitState::NotInstalled,
        _ => UnitState::Unknown,
    }
}

/// Map systemd's own answer to `GetUnitFileState` to a [`UnitState`]: the
/// state word `systemctl is-enabled` prints, or its error for a unit that
/// does not exist ("No such file or directory").
pub fn unit_state_from_manager(answer: Result<&str, &str>) -> UnitState {
    match answer {
        Ok(state) => parse_unit_state(state, ""),
        Err(error) if error.contains("No such file") => UnitState::NotInstalled,
        Err(_) => UnitState::Unknown,
    }
}

/// One call to the user's systemd over the session bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManagerStep {
    Enable,
    Disable,
    /// Reread unit files, as `systemctl enable` and `disable` do afterwards.
    Reload,
    Start,
    Stop,
}

/// The calls that do what `systemctl --user <args> satur8-daemon.service`
/// does, for the argument lists [`Action::Systemctl`] is built with.
pub fn manager_steps(args: &[&str]) -> Vec<ManagerStep> {
    let now = args.contains(&"--now");
    let mut steps = Vec::new();
    for arg in args {
        match *arg {
            "enable" => {
                steps.extend([ManagerStep::Enable, ManagerStep::Reload]);
                if now {
                    steps.push(ManagerStep::Start);
                }
            }
            "disable" => {
                steps.extend([ManagerStep::Disable, ManagerStep::Reload]);
                if now {
                    steps.push(ManagerStep::Stop);
                }
            }
            "start" => steps.push(ManagerStep::Start),
            "stop" => steps.push(ManagerStep::Stop),
            _ => {}
        }
    }
    steps
}

/// Read kwinrc [Plugins] satur8-focusEnabled.
pub fn focus_enabled_in(kwinrc: &str) -> bool {
    let mut in_plugins = false;
    for line in kwinrc.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_plugins = line == "[Plugins]";
            continue;
        }
        if !in_plugins {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            if key.trim() == FOCUS_KEY {
                return value.trim().eq_ignore_ascii_case("true");
            }
        }
    }
    false
}

/// `satur8 auto status`: the state in one line.
pub fn status_line(state: &SwitchState) -> String {
    let word = if !state.available {
        "not available"
    } else if state.on {
        "on"
    } else {
        "off"
    };
    // The detail is the line under the app's switch, which starts "Off. "
    // when switching is off; the word before it already says that.
    let detail = state.detail.strip_prefix("Off. ").unwrap_or(&state.detail);
    format!("automatic per-game switching: {word} ({detail})")
}
