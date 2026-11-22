//! Status page logic: read-only health checks, the automatic per-game
//! switching toggle, the repair plan, the "Right now" view, and the system
//! information used by "Copy system info" and "Report a bug".
//!
//! Everything that changes the system goes through [`System`], so the plans
//! can be tested with a fake. [`RealSystem`] is the only implementation that
//! touches the user's session, and it only runs after a click.

use std::path::PathBuf;
use std::process::Command;

use satur8_core::{Desktop, Environment, SessionType};
use satur8_kwin::repair::Step as RepairStep;
use zbus::blocking::Connection;

/// The daemon's systemd user unit (installed by install.sh and the packages).
pub const DAEMON_UNIT: &str = "satur8-daemon.service";
/// The KWin script id (assets/kwin-script/metadata.json).
pub const FOCUS_SCRIPT: &str = "satur8-focus";
/// kwinrc key that enables the script at login (packaging/install.sh:267).
const FOCUS_KEY: &str = "satur8-focusEnabled";
const DAEMON_BUS: &str = "org.satur8.Daemon";
const DAEMON_PATH: &str = "/org/satur8/Daemon";
const KWIN_BUS: &str = "org.kde.KWin";

// ------------------------------------------------------------------ facts

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitState {
    Enabled,
    Disabled,
    NotInstalled,
    /// systemctl could not answer (no user manager reachable).
    Unknown,
}

/// What the KWin effect check found. Only filled on KDE Plasma Wayland.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EffectFacts {
    /// KWin answered on the session bus.
    pub reachable: bool,
    /// The running KWin accepts the installed effect.
    pub supported: bool,
    /// The effect is loaded right now (only while saturation is not 0%).
    pub loaded: bool,
    /// KWin version of the installed development headers.
    pub headers: Option<String>,
    /// KWin version the system-wide effect was built for.
    pub system: Option<String>,
    /// KWin version the per-user effect was built for.
    pub user: Option<String>,
    /// What Repair would do here, from the same decision `satur8 repair`
    /// makes; `None` when it was not worked out.
    pub repair: Option<RepairStep>,
}

/// Everything the Status page knows about the system. Gathered read-only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facts {
    pub env: Environment,
    /// Name of the backend Satur8 would use, if any.
    pub backend: Option<String>,
    /// KWin effect facts; `None` off KDE Plasma Wayland.
    pub effect: Option<EffectFacts>,
    /// Version of the running KWin, from its support information.
    pub kwin_version: Option<String>,
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

// ------------------------------------------------------------- health rows

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Ok,
    Problem,
    NotApplicable,
}

impl Level {
    pub fn tag(self) -> &'static str {
        match self {
            Level::Ok => "ok",
            Level::Problem => "problem",
            Level::NotApplicable => "n/a",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HealthRow {
    pub label: &'static str,
    pub detail: String,
    pub level: Level,
}

fn row(label: &'static str, level: Level, detail: impl Into<String>) -> HealthRow {
    HealthRow {
        label,
        detail: detail.into(),
        level,
    }
}

/// Whether the user has asked for per-game switching in any way.
fn switching_wanted(facts: &Facts) -> bool {
    facts.unit == UnitState::Enabled || facts.script_enabled
}

fn backend_detail(name: &str) -> String {
    match name {
        "kwin" => "Working through KDE Plasma's compositor (kwin)".into(),
        "gnome-shell" => "Working through the GNOME Shell extension (gnome-shell)".into(),
        "hyprland" => "Working through a Hyprland screen shader (hyprland)".into(),
        "nv-control" => "Working through the NVIDIA driver setting (nv-control)".into(),
        "drm-ctm" => "Working through the display's color hardware (drm-ctm)".into(),
        other => format!("Working ({other})"),
    }
}

/// Map the facts to the four Health rows, in display order.
pub fn health_rows(facts: &Facts) -> Vec<HealthRow> {
    let kde = kde_wayland(&facts.env);
    let mut rows = Vec::with_capacity(4);

    // 1. Color backend.
    rows.push(match &facts.backend {
        Some(name) => row("Color control", Level::Ok, backend_detail(name)),
        None if kde => row(
            "Color control",
            Level::Problem,
            "Cannot change colors right now; see Saturation effect below",
        ),
        None => row(
            "Color control",
            Level::Problem,
            "No way to change colors was found in this session",
        ),
    });

    // 2. KWin effect.
    rows.push(if !kde {
        row(
            "Saturation effect",
            Level::NotApplicable,
            "Only used on KDE Plasma (Wayland)",
        )
    } else {
        match &facts.effect {
            Some(e) if e.reachable && e.supported => {
                let version = facts
                    .kwin_version
                    .as_deref()
                    .or(e.headers.as_deref())
                    .map(|v| format!(" ({v})"))
                    .unwrap_or_default();
                let active = if e.loaded {
                    "active now"
                } else {
                    "idle until a profile needs it"
                };
                row(
                    "Saturation effect",
                    Level::Ok,
                    format!("Installed and matches this KDE version{version}; {active}"),
                )
            }
            Some(e) if e.reachable => {
                let built = e.user.as_deref().or(e.system.as_deref());
                // Promise only what Repair can do on this system.
                let (add, fix) = match e.repair {
                    Some(RepairStep::InstallFromSet) => {
                        ("Repair can install it", "Repair replaces it")
                    }
                    Some(RepairStep::CannotRepair) => {
                        ("Repair cannot add it", "Repair cannot fix it")
                    }
                    _ => ("Repair can build it", "Repair rebuilds it"),
                };
                let detail = match (built, e.headers.as_deref()) {
                    (None, _) => format!("Not installed. {add}"),
                    (Some(built), Some(needed)) if built != needed => {
                        format!("Built for KDE {built}, but KDE {needed} is installed. {fix}")
                    }
                    (Some(built), _) => {
                        format!(
                            "Built for KDE {built}, but the running KDE will not load it. {fix}"
                        )
                    }
                };
                row("Saturation effect", Level::Problem, detail)
            }
            _ => row(
                "Saturation effect",
                Level::Problem,
                "KDE's compositor did not answer, so the effect could not be checked",
            ),
        }
    });

    // 3. Daemon.
    let wanted = switching_wanted(facts);
    rows.push(match (facts.daemon_running, facts.unit) {
        (None, _) => row(
            "Background service",
            Level::Problem,
            "Could not reach the session bus to check it",
        ),
        (Some(true), UnitState::Disabled) => row(
            "Background service",
            Level::Ok,
            "Running, but it will not start at the next login",
        ),
        (Some(true), _) => row("Background service", Level::Ok, "Running"),
        (Some(false), _) if !kde => row(
            "Background service",
            Level::NotApplicable,
            "Only needed for per-game switching on KDE Plasma (Wayland)",
        ),
        (Some(false), UnitState::NotInstalled) => row(
            "Background service",
            if wanted {
                Level::Problem
            } else {
                Level::NotApplicable
            },
            "Not installed (no satur8-daemon service found)",
        ),
        (Some(false), _) if wanted => row(
            "Background service",
            Level::Problem,
            "Not running, so games are not switched automatically",
        ),
        (Some(false), _) => row(
            "Background service",
            Level::NotApplicable,
            "Off; only needed for automatic per-game switching",
        ),
    });

    // 4. Focus script.
    rows.push(if !kde {
        row(
            "Game detection",
            Level::NotApplicable,
            "Only available on KDE Plasma (Wayland)",
        )
    } else {
        match (
            facts.script_loaded,
            facts.script_enabled,
            &facts.script_path,
        ) {
            (Some(true), _, _) => row(
                "Game detection",
                Level::Ok,
                "On: KDE tells Satur8 which window is focused",
            ),
            (None, _, _) => row(
                "Game detection",
                Level::Problem,
                "KDE's compositor did not answer, so it could not be checked",
            ),
            (Some(false), true, None) => row(
                "Game detection",
                Level::Problem,
                "Turned on, but the focus script is not installed",
            ),
            (Some(false), true, Some(_)) => row(
                "Game detection",
                Level::Problem,
                "Turned on, but not running in this session. Repair starts it",
            ),
            (Some(false), false, _) => row("Game detection", Level::NotApplicable, "Off"),
        }
    });

    rows
}

// ------------------------------------------------- per-game switching toggle

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchState {
    /// The toggle can be used in this session.
    pub available: bool,
    /// Every part of per-game switching is on right now.
    pub on: bool,
    /// One line under the toggle.
    pub detail: String,
}

/// Derive the toggle from the real system state (never from a stored flag).
pub fn switch_state(facts: &Facts) -> SwitchState {
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
    let detail = if done.is_empty() {
        "Off. Game profiles are not applied automatically".to_string()
    } else {
        format!("Partly on ({}). Turn on to finish", done.join(", "))
    };
    SwitchState {
        available: true,
        on: false,
        detail,
    }
}

// ------------------------------------------------------------------ actions

/// One system-changing step. Only [`RealSystem`] executes these for real.
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
    pub fn describe(&self) -> String {
        match self {
            Action::RepairEffect => "Check and repair the saturation effect".into(),
            Action::Systemctl(args) => format!("systemctl --user {} {DAEMON_UNIT}", args.join(" ")),
            Action::SetFocusEnabled(on) => {
                format!("kwriteconfig6 --file kwinrc --group Plugins --key {FOCUS_KEY} {on}")
            }
            Action::KwinReconfigure => "Ask KDE to reload its settings".into(),
            Action::LoadScript(_) => "Start game detection in KDE".into(),
            Action::UnloadScript => "Stop game detection in KDE".into(),
        }
    }
}

/// The installer's three steps, plus loading the script into the running
/// KWin (reconfigure alone does not hot-load it).
pub fn plan_enable(facts: &Facts) -> Vec<Action> {
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
pub fn plan_disable(_facts: &Facts) -> Vec<Action> {
    vec![
        Action::SetFocusEnabled(false),
        Action::KwinReconfigure,
        Action::UnloadScript,
        Action::Systemctl(vec!["disable", "--now"]),
    ]
}

/// `satur8 repair`, plus starting the daemon and loading the focus script when
/// those rows are failing.
pub fn plan_repair(facts: &Facts) -> Vec<Action> {
    let mut plan = Vec::new();
    if kde_wayland(&facts.env) {
        plan.push(Action::RepairEffect);
    }
    if facts.daemon_running == Some(false)
        && switching_wanted(facts)
        && matches!(facts.unit, UnitState::Enabled | UnitState::Disabled)
    {
        plan.push(Action::Systemctl(vec!["start"]));
    }
    if kde_wayland(&facts.env) && facts.script_enabled && facts.script_loaded == Some(false) {
        if let Some(path) = &facts.script_path {
            plan.push(Action::LoadScript(path.clone()));
        }
    }
    plan
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

/// Wait until org.satur8.Daemon has an owner, up to `timeout`. Read-only.
pub fn wait_for_daemon(timeout: std::time::Duration) -> bool {
    let Ok(conn) = session() else {
        return false;
    };
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if name_has_owner(&conn, DAEMON_BUS) == Some(true) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
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

/// The real thing. Spawns systemctl and kwriteconfig6 (system tools, not a
/// satur8 binary) and talks to KWin over D-Bus.
pub struct RealSystem;

impl System for RealSystem {
    fn run(&mut self, action: &Action) -> Result<String, String> {
        match action {
            Action::RepairEffect => satur8_kwin::repair::repair().map(|outcome| outcome.describe()),
            Action::Systemctl(args) => {
                let mut cmd = Command::new("systemctl");
                cmd.arg("--user").args(args).arg(DAEMON_UNIT);
                run_command(cmd)
            }
            Action::SetFocusEnabled(on) => {
                let mut cmd = Command::new("kwriteconfig6");
                cmd.args(["--file", "kwinrc", "--group", "Plugins", "--key", FOCUS_KEY])
                    .arg(if *on { "true" } else { "false" });
                run_command(cmd)
            }
            Action::KwinReconfigure => {
                let conn = session()?;
                // reconfigure is declared Q_NOREPLY; a normal call would wait
                // for a reply that never comes.
                let msg = zbus::Message::method_call("/KWin", "reconfigure")
                    .and_then(|b| b.destination(KWIN_BUS))
                    .and_then(|b| b.interface("org.kde.KWin"))
                    .and_then(|b| b.with_flags(zbus::message::Flags::NoReplyExpected))
                    .and_then(|b| b.build(&()))
                    .map_err(|e| e.to_string())?;
                conn.send(&msg).map_err(|e| e.to_string())?;
                Ok(String::new())
            }
            Action::LoadScript(path) => {
                let conn = session()?;
                if script_loaded(&conn) == Some(true) {
                    return Ok("already running".into());
                }
                let id: i32 = call(
                    &conn,
                    KWIN_BUS,
                    "/Scripting",
                    "org.kde.kwin.Scripting",
                    "loadScript",
                    &(path.to_string_lossy().as_ref(), FOCUS_SCRIPT),
                )?;
                if id < 0 {
                    return Err(format!("KDE refused to load the script (id {id})"));
                }
                call::<()>(
                    &conn,
                    KWIN_BUS,
                    "/Scripting",
                    "org.kde.kwin.Scripting",
                    "start",
                    &(),
                )?;
                Ok(String::new())
            }
            Action::UnloadScript => {
                let conn = session()?;
                let _: bool = call(
                    &conn,
                    KWIN_BUS,
                    "/Scripting",
                    "org.kde.kwin.Scripting",
                    "unloadScript",
                    &(FOCUS_SCRIPT,),
                )?;
                Ok(String::new())
            }
        }
    }
}

fn run_command(mut cmd: Command) -> Result<String, String> {
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::new())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

// ------------------------------------------------------------ read-only probe

fn session() -> Result<Connection, String> {
    Connection::session().map_err(|e| format!("session bus: {e}"))
}

fn call<R>(
    conn: &Connection,
    dest: &str,
    path: &str,
    iface: &str,
    method: &str,
    body: &(impl serde::Serialize + zbus::zvariant::DynamicType),
) -> Result<R, String>
where
    R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
{
    let reply = conn
        .call_method(Some(dest), path, Some(iface), method, body)
        .map_err(|e| e.to_string())?;
    reply.body().deserialize::<R>().map_err(|e| e.to_string())
}

fn name_has_owner(conn: &Connection, name: &str) -> Option<bool> {
    call(
        conn,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "NameHasOwner",
        &(name,),
    )
    .ok()
}

fn script_loaded(conn: &Connection) -> Option<bool> {
    call(
        conn,
        KWIN_BUS,
        "/Scripting",
        "org.kde.kwin.Scripting",
        "isScriptLoaded",
        &(FOCUS_SCRIPT,),
    )
    .ok()
}

fn get_property<R>(conn: &Connection, dest: &str, path: &str, iface: &str, name: &str) -> Option<R>
where
    R: TryFrom<zbus::zvariant::OwnedValue>,
{
    let value: zbus::zvariant::OwnedValue = call(
        conn,
        dest,
        path,
        "org.freedesktop.DBus.Properties",
        "Get",
        &(iface, name),
    )
    .ok()?;
    R::try_from(value).ok()
}

fn unit_state() -> UnitState {
    let Ok(out) = Command::new("systemctl")
        .args(["--user", "is-enabled", DAEMON_UNIT])
        .output()
    else {
        return UnitState::Unknown;
    };
    parse_unit_state(
        &String::from_utf8_lossy(&out.stdout),
        &String::from_utf8_lossy(&out.stderr),
    )
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

fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
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

fn find_script() -> Option<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(data) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        dirs.push(PathBuf::from(data));
    } else if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share"));
    }
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    dirs.extend(system.split(':').map(PathBuf::from));
    dirs.into_iter()
        .map(|d| {
            d.join("kwin/scripts")
                .join(FOCUS_SCRIPT)
                .join("contents/code/main.js")
        })
        .find(|p| p.is_file())
}

/// Pull "KWin version: x.y.z" out of KWin's support information.
pub fn kwin_version_in(support: &str) -> Option<String> {
    support.lines().find_map(|line| {
        line.trim()
            .strip_prefix("KWin version:")
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    })
}

/// Gather [`Facts`]. Read-only: D-Bus queries, `systemctl --user is-enabled`,
/// and file reads. Can take a moment, so call it off the UI thread.
pub fn probe() -> Facts {
    let env = Environment::detect();
    let kde = kde_wayland(&env);
    let backend = satur8_backend::select_backend()
        .ok()
        .map(|b| b.name().to_string());
    let conn = session().ok();

    let (effect, kwin_version, script_loaded) = if kde {
        let effect = match satur8_kwin::KwinBackend::connect() {
            Some(kwin) => {
                let supported = kwin.is_supported().unwrap_or(false);
                let state = satur8_kwin::repair::inspect(supported);
                let repair = Some(satur8_kwin::repair::decide(&state));
                EffectFacts {
                    reachable: true,
                    supported,
                    loaded: kwin.is_loaded().unwrap_or(false),
                    headers: state.headers,
                    system: state.system,
                    user: state.user,
                    repair,
                }
            }
            None => EffectFacts::default(),
        };
        let version = conn.as_ref().and_then(|c| {
            call::<String>(
                c,
                KWIN_BUS,
                "/KWin",
                "org.kde.KWin",
                "supportInformation",
                &(),
            )
            .ok()
            .and_then(|text| kwin_version_in(&text))
        });
        (Some(effect), version, conn.as_ref().and_then(script_loaded))
    } else {
        (None, None, None)
    };

    let script_enabled = config_home()
        .and_then(|d| std::fs::read_to_string(d.join("kwinrc")).ok())
        .map(|text| focus_enabled_in(&text))
        .unwrap_or(false);

    Facts {
        env,
        backend,
        effect,
        kwin_version,
        daemon_running: conn.as_ref().and_then(|c| name_has_owner(c, DAEMON_BUS)),
        unit: unit_state(),
        script_enabled,
        script_loaded,
        script_path: find_script(),
    }
}

// ----------------------------------------------------------------- right now

/// What the daemon and the compositor report right now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Live {
    pub daemon_running: bool,
    /// Daemon ActiveProfile; empty string means no profile.
    pub active_profile: Option<String>,
    /// Daemon LastFocused; `None` when the daemon lacks that property.
    pub last_focused: Option<String>,
    /// Saturation the KWin effect reports (1.0 when it is not loaded);
    /// `None` when that cannot be read on this session.
    pub applied: Option<f32>,
}

/// Light, read-only poll for the "Right now" card.
pub fn probe_live() -> Live {
    let mut live = Live::default();
    let Ok(conn) = session() else {
        return live;
    };
    live.daemon_running = name_has_owner(&conn, DAEMON_BUS) == Some(true);
    if live.daemon_running {
        live.active_profile =
            get_property::<String>(&conn, DAEMON_BUS, DAEMON_PATH, DAEMON_BUS, "ActiveProfile");
        live.last_focused =
            get_property::<String>(&conn, DAEMON_BUS, DAEMON_PATH, DAEMON_BUS, "LastFocused");
    }
    if kde_wayland(&Environment::detect()) {
        if let Some(kwin) = satur8_kwin::KwinBackend::connect() {
            live.applied = match kwin.is_loaded() {
                Ok(true) => kwin.current_saturation().ok().map(|s| s.get()),
                Ok(false) => Some(1.0),
                Err(_) => None,
            };
        }
    }
    live
}

/// "Active profile" line.
pub fn describe_active(live: &Live) -> String {
    match live.active_profile.as_deref() {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => "None, desktop default".to_string(),
    }
}

/// "Last focused window" line, or `None` to hide the row.
pub fn describe_focused(live: &Live) -> Option<String> {
    live.last_focused.as_ref().map(|w| {
        if w.is_empty() {
            "Desktop or nothing reported yet".to_string()
        } else {
            w.clone()
        }
    })
}

// --------------------------------------------------------------- system info

pub struct SysInfo<'a> {
    pub version: &'a str,
    pub facts: &'a Facts,
    pub rows: &'a [HealthRow],
    pub distro: Option<&'a str>,
}

/// The plain-text block for "Copy system info" and the bug report.
pub fn system_info_text(info: &SysInfo) -> String {
    let f = info.facts;
    let mut text = String::new();
    text.push_str(&format!("Satur8 v{}\n", info.version));
    text.push_str(&format!("Session: {}\n", f.env.session));
    text.push_str(&format!("Desktop: {}\n", f.env.desktop));
    text.push_str(&format!("GPU: {}\n", f.env.gpu));
    text.push_str(&format!(
        "KWin (Plasma) version: {}\n",
        f.kwin_version.as_deref().unwrap_or("n/a")
    ));
    text.push_str(&format!("Distro: {}\n", info.distro.unwrap_or("unknown")));
    text.push_str(&format!(
        "Backend: {}\n",
        f.backend.as_deref().unwrap_or("none")
    ));
    text.push_str("Health:\n");
    for row in info.rows {
        text.push_str(&format!(
            "- [{}] {}: {}\n",
            row.level.tag(),
            row.label,
            row.detail
        ));
    }
    text
}

/// Replace the home directory with `~` and drop the user and host names.
pub fn sanitize(text: &str, home: Option<&str>, user: Option<&str>, host: Option<&str>) -> String {
    let mut out = text.to_string();
    if let Some(home) = home
        .map(|h| h.trim_end_matches('/'))
        .filter(|h| h.len() > 1)
    {
        out = out.replace(home, "~");
    }
    for (name, stand_in) in [(user, "<user>"), (host, "<host>")] {
        if let Some(name) = name.filter(|n| n.len() >= 2) {
            out = replace_word(&out, name, stand_in);
        }
    }
    out
}

/// Replace `word` where it is not part of a longer alphanumeric word.
fn replace_word(text: &str, word: &str, with: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find(word) {
        let before = rest[..pos].chars().next_back();
        let after = rest[pos + word.len()..].chars().next();
        let boundary = |c: Option<char>| !c.is_some_and(|c| c.is_alphanumeric());
        out.push_str(&rest[..pos]);
        if boundary(before) && boundary(after) {
            out.push_str(with);
        } else {
            out.push_str(word);
        }
        rest = &rest[pos + word.len()..];
    }
    out.push_str(rest);
    out
}

/// "PRETTY_NAME" (or NAME + VERSION_ID) from os-release text.
pub fn distro_from_os_release(text: &str) -> Option<String> {
    let field = |key: &str| {
        text.lines().find_map(|line| {
            line.strip_prefix(key)
                .and_then(|v| v.strip_prefix('='))
                .map(|v| v.trim().trim_matches('"').to_string())
                .filter(|v| !v.is_empty())
        })
    };
    field("PRETTY_NAME").or_else(|| {
        let name = field("NAME")?;
        Some(match field("VERSION_ID") {
            Some(version) => format!("{name} {version}"),
            None => name,
        })
    })
}

pub fn distro() -> Option<String> {
    ["/etc/os-release", "/usr/lib/os-release"]
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
        .and_then(|text| distro_from_os_release(&text))
}

/// Sanitized system info for this machine.
pub fn system_info_for_this_machine(facts: &Facts, rows: &[HealthRow]) -> String {
    let distro = distro();
    let text = system_info_text(&SysInfo {
        version: env!("CARGO_PKG_VERSION"),
        facts,
        rows,
        distro: distro.as_deref(),
    });
    let home = std::env::var("HOME").ok();
    let user = std::env::var("USER")
        .ok()
        .or_else(|| std::env::var("LOGNAME").ok());
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|h| h.trim().to_string());
    sanitize(&text, home.as_deref(), user.as_deref(), host.as_deref())
}

// ---------------------------------------------------------------- bug report

const NEW_ISSUE: &str = "https://github.com/NtrpyDev/satur8/issues/new";

/// The new-issue page with a body that follows .github/ISSUE_TEMPLATE/
/// bug_report.md. The address carries no information about this machine: a
/// browser sends the whole address to GitHub as soon as the page opens, before
/// the user has submitted anything. The system info goes on the clipboard
/// instead, for the user to paste after reading it. Only the documented `body`
/// parameter is used, so the prefill does not depend on how GitHub combines
/// `template` with `body`.
pub fn bug_report_url() -> String {
    let body = "## What happened\n\n\n\n## What you expected\n\n\n\n## Steps to reproduce\n\n1.\n2.\n\n\
                ## Environment\n\nInstall method: \n\n\
                <!-- Paste here: Report a bug copied your system info to the clipboard. -->\n\n\
                ## Diagnostics\n\n<!-- If you can, paste the output of `satur8 doctor` and `satur8 status`. -->\n";
    format!("{NEW_ISSUE}?body={}", percent_encode(body))
}

fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use satur8_core::Gpu;

    fn kde() -> Environment {
        Environment {
            session: SessionType::Wayland,
            desktop: Desktop::Kde,
            gpu: Gpu::Amd,
        }
    }

    fn healthy() -> Facts {
        Facts {
            env: kde(),
            backend: Some("kwin".into()),
            effect: Some(EffectFacts {
                reachable: true,
                supported: true,
                loaded: false,
                headers: Some("6.8.3".into()),
                system: Some("6.8.3".into()),
                user: None,
                repair: Some(RepairStep::Healthy),
            }),
            kwin_version: Some("6.8.3".into()),
            daemon_running: Some(true),
            unit: UnitState::Enabled,
            script_enabled: true,
            script_loaded: Some(true),
            script_path: Some(PathBuf::from(
                "/usr/share/kwin/scripts/satur8-focus/contents/code/main.js",
            )),
        }
    }

    fn levels(facts: &Facts) -> Vec<Level> {
        health_rows(facts).iter().map(|r| r.level).collect()
    }

    #[test]
    fn healthy_kde_session_is_all_ok() {
        assert_eq!(levels(&healthy()), vec![Level::Ok; 4]);
        assert!(switch_state(&healthy()).on);
        assert_eq!(plan_repair(&healthy()), vec![Action::RepairEffect]);
    }

    #[test]
    fn non_kde_marks_kwin_rows_not_applicable_not_failed() {
        let facts = Facts {
            env: Environment {
                session: SessionType::Wayland,
                desktop: Desktop::Gnome,
                gpu: Gpu::Intel,
            },
            backend: Some("gnome-shell".into()),
            effect: None,
            kwin_version: None,
            daemon_running: Some(false),
            unit: UnitState::Disabled,
            script_enabled: false,
            script_loaded: None,
            script_path: None,
        };
        assert_eq!(
            levels(&facts),
            vec![
                Level::Ok,
                Level::NotApplicable,
                Level::NotApplicable,
                Level::NotApplicable
            ]
        );
        let switch = switch_state(&facts);
        assert!(!switch.available && !switch.on);
        assert!(switch.detail.contains("KDE Plasma on Wayland"));
        assert!(plan_repair(&facts).is_empty());
    }

    #[test]
    fn stranded_effect_is_a_problem_that_names_both_versions() {
        let mut facts = healthy();
        facts.backend = None;
        facts.effect = Some(EffectFacts {
            reachable: true,
            supported: false,
            loaded: false,
            headers: Some("6.8.3".into()),
            system: Some("6.8.2".into()),
            user: None,
            repair: Some(RepairStep::Build),
        });
        let rows = health_rows(&facts);
        assert_eq!(rows[0].level, Level::Problem);
        assert_eq!(rows[1].level, Level::Problem);
        assert!(rows[1].detail.contains("6.8.2") && rows[1].detail.contains("6.8.3"));
        assert!(rows[1].detail.ends_with("Repair rebuilds it"));
    }

    fn effect_detail(user: Option<&str>, headers: &str, repair: RepairStep) -> String {
        let mut facts = healthy();
        facts.effect = Some(EffectFacts {
            reachable: true,
            supported: false,
            loaded: false,
            headers: Some(headers.into()),
            system: None,
            user: user.map(Into::into),
            repair: Some(repair),
        });
        health_rows(&facts)[1].detail.clone()
    }

    #[test]
    fn effect_row_promises_only_what_repair_can_do_here() {
        // The Deck after an OS update: the set has the new effect.
        assert_eq!(
            effect_detail(Some("6.7.3"), "6.8.2", RepairStep::InstallFromSet),
            "Built for KDE 6.7.3, but KDE 6.8.2 is installed. Repair replaces it"
        );
        assert_eq!(
            effect_detail(None, "6.7.3", RepairStep::InstallFromSet),
            "Not installed. Repair can install it"
        );
        // No set entry and no build tools: no promise of a rebuild.
        for detail in [
            effect_detail(Some("6.8.2"), "6.9.0", RepairStep::CannotRepair),
            effect_detail(Some("6.9.0"), "6.9.0", RepairStep::CannotRepair),
            effect_detail(None, "6.9.0", RepairStep::CannotRepair),
        ] {
            assert!(!detail.contains("build"), "{detail}");
            assert!(detail.contains("cannot"), "{detail}");
        }
        assert_eq!(
            effect_detail(None, "6.8.3", RepairStep::Build),
            "Not installed. Repair can build it"
        );
    }

    #[test]
    fn wanted_but_stopped_daemon_and_unloaded_script_are_problems_repair_fixes() {
        let mut facts = healthy();
        facts.daemon_running = Some(false);
        facts.script_loaded = Some(false);
        let rows = health_rows(&facts);
        assert_eq!(rows[2].level, Level::Problem);
        assert_eq!(rows[3].level, Level::Problem);
        let path = facts.script_path.clone().unwrap();
        assert_eq!(
            plan_repair(&facts),
            vec![
                Action::RepairEffect,
                Action::Systemctl(vec!["start"]),
                Action::LoadScript(path)
            ]
        );
        let switch = switch_state(&facts);
        assert!(switch.available && !switch.on);
        assert!(switch.detail.starts_with("Partly on"));
    }

    #[test]
    fn switching_off_on_purpose_is_muted_not_a_problem() {
        let mut facts = healthy();
        facts.daemon_running = Some(false);
        facts.unit = UnitState::Disabled;
        facts.script_enabled = false;
        facts.script_loaded = Some(false);
        let rows = health_rows(&facts);
        assert_eq!(rows[2].level, Level::NotApplicable);
        assert_eq!(rows[3].level, Level::NotApplicable);
        assert_eq!(plan_repair(&facts), vec![Action::RepairEffect]);
        let switch = switch_state(&facts);
        assert!(switch.available && !switch.on);
        assert!(switch.detail.starts_with("Off"));
    }

    #[test]
    fn toggle_needs_the_service_to_start_at_login() {
        let mut facts = healthy();
        facts.unit = UnitState::Disabled;
        assert!(!switch_state(&facts).on);
        // systemctl unreachable: judge by what is running.
        facts.unit = UnitState::Unknown;
        assert!(switch_state(&facts).on);
    }

    #[test]
    fn toggle_is_unavailable_without_the_script_or_the_unit() {
        let mut facts = healthy();
        facts.script_path = None;
        assert!(!switch_state(&facts).available);
        let mut facts = healthy();
        facts.unit = UnitState::NotInstalled;
        assert!(!switch_state(&facts).available);
    }

    #[test]
    fn enable_plan_is_the_installer_steps_plus_hot_loading_the_script() {
        let mut facts = healthy();
        facts.script_loaded = Some(false);
        let path = facts.script_path.clone().unwrap();
        assert_eq!(
            plan_enable(&facts),
            vec![
                Action::Systemctl(vec!["enable", "--now"]),
                Action::SetFocusEnabled(true),
                Action::KwinReconfigure,
                Action::LoadScript(path),
            ]
        );
        assert_eq!(
            plan_disable(&facts),
            vec![
                Action::SetFocusEnabled(false),
                Action::KwinReconfigure,
                Action::UnloadScript,
                Action::Systemctl(vec!["disable", "--now"]),
            ]
        );
    }

    struct Fake {
        ran: Vec<Action>,
        fail: Option<Action>,
    }

    impl System for Fake {
        fn run(&mut self, action: &Action) -> Result<String, String> {
            self.ran.push(action.clone());
            if self.fail.as_ref() == Some(action) {
                Err("boom".into())
            } else {
                Ok(String::new())
            }
        }
    }

    #[test]
    fn re_check_waits_only_after_plans_that_start_the_service() {
        let mut facts = healthy();
        facts.daemon_running = Some(false);
        assert!(plan_starts_daemon(&plan_repair(&facts)));
        assert!(plan_starts_daemon(&plan_enable(&facts)));
        assert!(!plan_starts_daemon(&plan_disable(&facts)));
        assert!(!plan_starts_daemon(&plan_repair(&healthy())));
    }

    #[test]
    fn execute_runs_every_step_and_reports_failures_per_step() {
        let plan = plan_disable(&healthy());
        let mut fake = Fake {
            ran: Vec::new(),
            fail: Some(Action::KwinReconfigure),
        };
        let log = execute(&mut fake, &plan);
        assert_eq!(fake.ran, plan);
        assert_eq!(log.len(), 4);
        assert!(log[0].0 && !log[1].0 && log[2].0 && log[3].0);
        assert!(log[1].1.ends_with("failed: boom"));
        assert_eq!(
            log[3].1,
            "systemctl --user disable --now satur8-daemon.service"
        );
    }

    #[test]
    fn parses_systemctl_and_kwinrc_and_support_info() {
        assert_eq!(parse_unit_state("enabled\n", ""), UnitState::Enabled);
        assert_eq!(parse_unit_state("disabled\n", ""), UnitState::Disabled);
        assert_eq!(parse_unit_state("not-found\n", ""), UnitState::NotInstalled);
        assert_eq!(
            parse_unit_state(
                "",
                "Failed to connect to user scope bus via local transport: No such file or directory"
            ),
            UnitState::Unknown
        );
        assert_eq!(
            parse_unit_state(
                "",
                "Failed to get unit file state for satur8-daemon.service: No such file or directory"
            ),
            UnitState::NotInstalled
        );
        let rc = "[Compositing]\nsatur8-focusEnabled=true\n[Plugins]\nblurEnabled=false\nsatur8-focusEnabled=true\n";
        assert!(focus_enabled_in(rc));
        assert!(!focus_enabled_in("[Plugins]\nsatur8-focusEnabled=false\n"));
        assert!(!focus_enabled_in(
            "[Compositing]\nsatur8-focusEnabled=true\n"
        ));
        assert_eq!(
            kwin_version_in("Version\n=======\nKWin version: 6.8.3\nQt Version: 6.12.0\n"),
            Some("6.8.3".into())
        );
    }

    #[test]
    fn system_info_lists_everything_and_is_sanitized() {
        let mut facts = healthy();
        facts.script_path = Some(PathBuf::from(
            "/home/alice/.local/share/kwin/scripts/satur8-focus/contents/code/main.js",
        ));
        let rows = vec![HealthRow {
            label: "Game detection",
            detail: "loaded from /home/alice/.local/share on alicebox for alice".into(),
            level: Level::Ok,
        }];
        let text = system_info_text(&SysInfo {
            version: "0.3.6",
            facts: &facts,
            rows: &rows,
            distro: Some("CachyOS Linux"),
        });
        for needle in [
            "Satur8 v0.3.6",
            "Session: Wayland",
            "Desktop: KDE Plasma",
            "GPU: AMD",
            "KWin (Plasma) version: 6.8.3",
            "Distro: CachyOS Linux",
            "Backend: kwin",
            "- [ok] Game detection:",
        ] {
            assert!(text.contains(needle), "missing {needle:?} in\n{text}");
        }
        let clean = sanitize(&text, Some("/home/alice"), Some("alice"), Some("alicebox"));
        assert!(clean.contains("loaded from ~/.local/share on <host> for <user>"));
        assert!(!clean.contains("alice"));
        // Whole words only: "alicebox" is the host, not "<user>box".
        assert_eq!(
            sanitize("alicebox alice", None, Some("alice"), None),
            "alicebox <user>"
        );
    }

    #[test]
    fn reads_the_distro_from_os_release() {
        assert_eq!(
            distro_from_os_release("NAME=\"CachyOS Linux\"\nPRETTY_NAME=\"CachyOS\"\n"),
            Some("CachyOS".into())
        );
        assert_eq!(
            distro_from_os_release("NAME=Fedora\nVERSION_ID=44\n"),
            Some("Fedora 44".into())
        );
    }

    #[test]
    fn bug_report_url_prefills_the_template_and_carries_nothing_about_the_machine() {
        let url = bug_report_url();
        assert!(url.starts_with("https://github.com/NtrpyDev/satur8/issues/new?body="));
        let query = url.split_once("?body=").unwrap().1;
        assert!(!query.contains(' ') && !query.contains('\n') && !query.contains('&'));
        assert!(query.contains("%23%23%20What%20happened"));
        assert!(query.contains("Paste%20here"));
        // The address is the same on every machine, so it cannot leak anything.
        assert_eq!(url, bug_report_url());
        assert!(!query.contains("Satur8%20v"));
    }

    #[test]
    fn live_descriptions() {
        let mut live = Live::default();
        assert_eq!(describe_active(&live), "None, desktop default");
        assert_eq!(describe_focused(&live), None);
        live.active_profile = Some("cs2".into());
        live.last_focused = Some(String::new());
        assert_eq!(describe_active(&live), "cs2");
        assert_eq!(
            describe_focused(&live).as_deref(),
            Some("Desktop or nothing reported yet")
        );
    }
}
