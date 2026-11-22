//! Status page logic: read-only health checks, the automatic per-game
//! switching toggle, the repair plan, the "Right now" view, and the system
//! information used by "Copy system info" and "Report a bug".
//!
//! Everything that changes the system goes through
//! [`System`](satur8_core::switching::System), so the plans can be tested
//! with a fake. [`RealSystem`] is the only implementation that touches the
//! user's session, and it only runs after a click.

use std::path::PathBuf;

use satur8_core::Environment;
#[cfg(test)]
use satur8_core::{Desktop, SessionType};
use satur8_kwin::repair::Step as RepairStep;
use zbus::blocking::Connection;

// The switching toggle's state and steps are shared with `satur8 auto`: the
// pure part lives in satur8_core::switching, the system part in
// satur8_backend::switching.
use satur8_backend::switching::{
    find_script, name_has_owner, script_enabled, script_loaded, unit_state, DAEMON_BUS,
};
pub use satur8_backend::switching::{wait_for_daemon, RealSystem};
use satur8_core::switching::{self as switching, SwitchFacts};
pub use satur8_core::switching::{
    execute, kde_wayland, plan_starts_daemon, Action, SwitchState, UnitState,
};
#[cfg(test)]
use satur8_core::switching::{
    focus_enabled_in, manager_steps, parse_unit_state, unit_state_from_manager, ManagerStep, System,
};

const DAEMON_PATH: &str = "/org/satur8/Daemon";
const KWIN_BUS: &str = "org.kde.KWin";

// ------------------------------------------------------------------ facts

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
    /// KWin version of a per-user effect file that is for neither the running
    /// nor the installed KWin, from `satur8_kwin::repair::stale_user_file`.
    /// KWin can still accept the copy it loaded earlier, so the row must not
    /// say the effect matches.
    pub stale_user: Option<String>,
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
    /// Steam Game Mode: gamescope's color management is turned off
    /// (`GAMESCOPE_COLOR_MANAGEMENT_DISABLE`), so it ignores every color
    /// setting, Satur8's table included.
    pub color_management_off: bool,
}

/// What the page says wherever gamescope's color management is off: the
/// "Right now" card's small line and the Color control row.
pub const COLOR_MANAGEMENT_OFF: &str =
    "gamescope's color management is off, so Satur8's setting is not shown";

impl Facts {
    /// The part of the facts automatic per-game switching depends on.
    pub fn switching(&self) -> SwitchFacts {
        SwitchFacts {
            env: self.env,
            backend: self.backend.clone(),
            daemon_running: self.daemon_running,
            unit: self.unit,
            script_enabled: self.script_enabled,
            script_loaded: self.script_loaded,
            script_path: self.script_path.clone(),
        }
    }
}

/// Steam Game Mode; see [`satur8_core::switching::game_mode`].
pub fn game_mode(facts: &Facts) -> bool {
    switching::game_mode(facts.backend.as_deref())
}

/// KDE Plasma on Wayland, where the KWin effect and focus script apply.
fn kde_desktop(facts: &Facts) -> bool {
    kde_wayland(&facts.env) && !game_mode(facts)
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

/// Whether the user has asked for per-game switching in any way. In Game Mode
/// only the service counts: the KWin script setting belongs to Desktop Mode.
fn switching_wanted(facts: &Facts) -> bool {
    if game_mode(facts) {
        return facts.unit == UnitState::Enabled;
    }
    facts.unit == UnitState::Enabled || facts.script_enabled
}

fn backend_detail(name: &str) -> String {
    match name {
        "gamescope" => "Working through Steam Game Mode (gamescope)".into(),
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
    let kde = kde_desktop(facts);
    let game = game_mode(facts);
    let mut rows = Vec::with_capacity(4);

    // 1. Color backend.
    rows.push(match &facts.backend {
        Some(_) if game && facts.color_management_off => {
            row("Color control", Level::Problem, COLOR_MANAGEMENT_OFF)
        }
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
            Some(e) if e.reachable && e.supported && e.stale_user.is_none() => {
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
                    Some(RepairStep::DropUserCopy) => ("Repair can build it", "Repair removes it"),
                    _ => ("Repair can build it", "Repair rebuilds it"),
                };
                // A stale per-user file is wrong for the running KDE even
                // where no development headers name the installed version.
                let needed = e.headers.as_deref().or(if e.stale_user.is_some() {
                    facts.kwin_version.as_deref()
                } else {
                    None
                });
                let detail = match (built, needed) {
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
        (Some(false), _) if !kde && !game => row(
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

    // 4. Focus script. In Game Mode the service reads gamescope's focus itself.
    rows.push(if game {
        row(
            "Game detection",
            if facts.daemon_running == Some(true) {
                Level::Ok
            } else {
                Level::NotApplicable
            },
            "Built into the service in Game Mode",
        )
    } else if !kde {
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

/// Derive the toggle from the real system state (never from a stored flag).
pub fn switch_state(facts: &Facts) -> SwitchState {
    switching::switch_state(&facts.switching())
}

// ------------------------------------------------------------------ actions

/// The installer's three steps, plus loading the script into the running
/// KWin. In Game Mode only the service step applies.
pub fn plan_enable(facts: &Facts) -> Vec<Action> {
    switching::plan_enable(&facts.switching())
}

/// The reverse of [`plan_enable`].
pub fn plan_disable(facts: &Facts) -> Vec<Action> {
    switching::plan_disable(&facts.switching())
}

/// `satur8 repair`, plus starting the daemon and loading the focus script when
/// those rows are failing. In Game Mode there is no KWin effect or script, so
/// only starting the service applies.
pub fn plan_repair(facts: &Facts) -> Vec<Action> {
    let mut plan = Vec::new();
    if kde_desktop(facts) {
        plan.push(Action::RepairEffect);
    }
    if facts.daemon_running == Some(false)
        && switching_wanted(facts)
        && matches!(facts.unit, UnitState::Enabled | UnitState::Disabled)
    {
        plan.push(Action::Systemctl(vec!["start"]));
    }
    if kde_desktop(facts) && facts.script_enabled && facts.script_loaded == Some(false) {
        if let Some(path) = &facts.script_path {
            plan.push(Action::LoadScript(path.clone()));
        }
    }
    plan
}

/// The rows the Game Mode layout lists: what is checked there. The KDE
/// effect and the KDE focus script belong to Desktop Mode, so their rows are
/// left out while they do not apply; the background service always shows.
pub fn game_mode_rows(rows: Vec<HealthRow>) -> Vec<HealthRow> {
    rows.into_iter()
        .filter(|row| row.level != Level::NotApplicable || row.label == "Background service")
        .collect()
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

/// Pull "KWin version: x.y.z" out of KWin's support information.
pub fn kwin_version_in(support: &str) -> Option<String> {
    support.lines().find_map(|line| {
        line.trim()
            .strip_prefix("KWin version:")
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    })
}

/// The effect facts for a reachable KWin, from what repair reads (`state`),
/// what KWin says (`supported`, `loaded`) and the running KWin's version.
/// Repair's decision and the stale-file check need `running`: KWin keeps
/// accepting an effect it loaded earlier after the file on disk was replaced
/// by one for another KWin. With `running` unknown, KWin's answer is trusted.
pub fn effect_facts(
    state: satur8_kwin::repair::EffectState,
    supported: bool,
    loaded: bool,
    running: Option<String>,
) -> EffectFacts {
    let state = satur8_kwin::repair::EffectState {
        supported,
        running,
        ..state
    };
    let repair = Some(satur8_kwin::repair::decide(&state));
    let stale_user = satur8_kwin::repair::stale_user_file(&state).map(String::from);
    EffectFacts {
        reachable: true,
        supported,
        loaded,
        headers: state.headers,
        system: state.system,
        user: state.user,
        repair,
        stale_user,
    }
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
        // The running KWin's version, from the support information repair and
        // doctor read too. Asked once: it serves the effect check and the
        // version shown on the page.
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
        let effect = match satur8_kwin::KwinBackend::connect() {
            Some(kwin) => {
                let supported = kwin.is_supported().unwrap_or(false);
                effect_facts(
                    satur8_kwin::repair::inspect(supported),
                    supported,
                    kwin.is_loaded().unwrap_or(false),
                    version.clone(),
                )
            }
            None => EffectFacts::default(),
        };
        (Some(effect), version, conn.as_ref().and_then(script_loaded))
    } else {
        (None, None, None)
    };

    let script_enabled = script_enabled();
    // Steam Game Mode: whether gamescope is ignoring its color settings.
    let color_management_off = backend.as_deref() == Some("gamescope")
        && satur8_gamescope::session::find_session()
            .and_then(|(_, root)| satur8_gamescope::session::read_color_state(&root).ok())
            .is_some_and(|state| state.color_management_disabled);

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
        color_management_off,
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
    /// Saturation the KWin effect reports (1.0 when it is not loaded), or
    /// in Steam Game Mode the one gamescope's loaded table says; `None` when
    /// that cannot be read on this session.
    pub applied: Option<f32>,
    /// Steam Game Mode: gamescope's own session was found.
    pub game_mode: bool,
    /// Steam Game Mode: gamescope's color management is turned off, so it
    /// ignores every color setting, Satur8's table included.
    pub color_management_off: bool,
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
    } else if let Some((_, root)) = satur8_gamescope::session::find_session() {
        // Steam Game Mode: the table gamescope has loaded says it.
        live.game_mode = true;
        let state = satur8_gamescope::session::read_color_state(&root).ok();
        live.color_management_off = state
            .as_ref()
            .is_some_and(|state| state.color_management_disabled);
        live.applied = state.as_ref().and_then(applied_in_game_mode);
    }
    live
}

/// The saturation on screen in Steam Game Mode. While gamescope's color
/// management is turned off (`GAMESCOPE_COLOR_MANAGEMENT_DISABLE`) it ignores
/// every color setting, so the screen shows normal colors whatever table is
/// loaded; otherwise the loaded look says it.
pub fn applied_in_game_mode(state: &satur8_gamescope::session::ColorState) -> Option<f32> {
    if state.color_management_disabled {
        return Some(1.0);
    }
    applied_from_look(&state.look)
}

/// What the "applied" value's small line says.
pub fn applied_note(saturation: f32, color_management_off: bool) -> &'static str {
    if color_management_off {
        COLOR_MANAGEMENT_OFF
    } else if (saturation - 1.0).abs() < 1e-3 {
        "normal colors, read from the compositor"
    } else {
        "read from the compositor"
    }
}

/// The saturation gamescope shows, from the look it has loaded: Satur8's
/// table says its own value, no table is normal colors, and a table something
/// else loaded cannot be read as a saturation.
pub fn applied_from_look(look: &satur8_gamescope::session::Look) -> Option<f32> {
    use satur8_gamescope::session::Look;
    match look {
        Look::None => Some(1.0),
        Look::Satur8(saturation) => Some(saturation.get()),
        Look::Foreign(_) => None,
    }
}

/// "Active profile" line. `off` says the active profile is turned off: it is
/// still its game's profile, and the game shows normal colors.
pub fn describe_active(live: &Live, game_mode: bool, off: bool) -> String {
    match live.active_profile.as_deref() {
        Some(name) if !name.is_empty() && off => format!("{name}, off: normal colors"),
        Some(name) if !name.is_empty() => name.to_string(),
        // Steam Game Mode has no desktop: its first row is "General".
        _ if game_mode => "None, General".to_string(),
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
                ## Diagnostics\n\n<!-- Only if you have a terminal at hand: the output of `satur8 doctor` and `satur8 status`. -->\n";
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
                stale_user: None,
            }),
            kwin_version: Some("6.8.3".into()),
            daemon_running: Some(true),
            unit: UnitState::Enabled,
            script_enabled: true,
            script_loaded: Some(true),
            script_path: Some(PathBuf::from(
                "/usr/share/kwin/scripts/satur8-focus/contents/code/main.js",
            )),
            color_management_off: false,
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
            color_management_off: false,
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
            stale_user: None,
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
            stale_user: None,
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

    fn stale_detail(headers: Option<&str>, repair: RepairStep) -> (Level, String) {
        // KWin still accepts the 6.7.3 copy it loaded, but the per-user file on
        // disk was replaced by one for KWin 6.7.9.
        let mut facts = healthy();
        facts.kwin_version = Some("6.7.3".into());
        facts.effect = Some(EffectFacts {
            reachable: true,
            supported: true,
            loaded: true,
            headers: headers.map(Into::into),
            system: None,
            user: Some("6.7.9".into()),
            repair: Some(repair),
            stale_user: Some("6.7.9".into()),
        });
        let row = &health_rows(&facts)[1];
        (row.level, row.detail.clone())
    }

    #[test]
    fn a_stale_per_user_file_kwin_still_accepts_is_not_reported_as_matching() {
        assert_eq!(
            stale_detail(Some("6.7.3"), RepairStep::InstallFromSet),
            (
                Level::Problem,
                "Built for KDE 6.7.9, but KDE 6.7.3 is installed. Repair replaces it".into()
            )
        );
        // No development headers: the running version names what is needed.
        assert_eq!(
            stale_detail(None, RepairStep::InstallFromSet).1,
            "Built for KDE 6.7.9, but KDE 6.7.3 is installed. Repair replaces it"
        );
        // A good system copy: repair deletes the per-user file.
        assert_eq!(
            stale_detail(Some("6.7.3"), RepairStep::DropUserCopy).1,
            "Built for KDE 6.7.9, but KDE 6.7.3 is installed. Repair removes it"
        );
        let (_, detail) = stale_detail(Some("6.7.3"), RepairStep::CannotRepair);
        assert!(detail.ends_with("Repair cannot fix it"), "{detail}");
        // The same file without the stale flag is the healthy case.
        let mut facts = healthy();
        if let Some(effect) = facts.effect.as_mut() {
            effect.user = Some("6.8.3".into());
        }
        assert_eq!(health_rows(&facts)[1].level, Level::Ok);
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
        // Plain words, not the command behind the step.
        assert_eq!(
            log[3].1,
            "Turn off the background service, now and at every start"
        );
        assert_eq!(log[0].1, "Turn off game detection in KDE's settings");
        for (_, line) in &log {
            assert!(!line.contains("systemctl") && !line.contains("kwriteconfig"));
        }
        assert_eq!(
            Action::Systemctl(vec!["enable", "--now"]).describe(),
            "Turn on the background service, now and at every start"
        );
        assert_eq!(
            Action::Systemctl(vec!["start"]).describe(),
            "Start the background service"
        );
        assert_eq!(
            Action::SetFocusEnabled(true).describe(),
            "Turn on game detection in KDE's settings"
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
        assert_eq!(
            describe_active(&live, false, false),
            "None, desktop default"
        );
        assert_eq!(describe_focused(&live), None);
        live.active_profile = Some("cs2".into());
        live.last_focused = Some(String::new());
        assert_eq!(describe_active(&live, false, false), "cs2");
        assert_eq!(
            describe_active(&live, false, true),
            "cs2, off: normal colors"
        );
        assert_eq!(
            describe_focused(&live).as_deref(),
            Some("Desktop or nothing reported yet")
        );
    }

    /// What probe() hands effect_facts() on the Deck: KWin 6.7.3 running and
    /// installed, the per-user file replaced by one for 6.7.9, the set holding
    /// steamos-6.7.3, and KWin still answering supported from memory.
    fn deck_state_with_a_wrong_file() -> satur8_kwin::repair::EffectState {
        satur8_kwin::repair::EffectState {
            headers: Some("6.7.3".into()),
            user: Some("6.7.9".into()),
            user_source_current: true,
            set_match: Some(satur8_kwin::repair::SetPick {
                key: "steamos-6.7.3".into(),
                other_system: false,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn effect_row_uses_the_running_version_for_a_file_kwin_still_accepts() {
        let mut facts = healthy();
        facts.kwin_version = Some("6.7.3".into());
        facts.effect = Some(effect_facts(
            deck_state_with_a_wrong_file(),
            true,
            true,
            Some("6.7.3".into()),
        ));
        let row = &health_rows(&facts)[1];
        assert_eq!(row.level, Level::Problem);
        assert_eq!(
            row.detail,
            "Built for KDE 6.7.9, but KDE 6.7.3 is installed. Repair replaces it"
        );
        let effect = facts
            .effect
            .as_ref()
            .map(|e| (e.repair, e.stale_user.clone()));
        assert_eq!(
            effect,
            Some((Some(RepairStep::InstallFromSet), Some("6.7.9".into())))
        );

        // Running version unknown: KWin's answer is trusted, as before.
        facts.effect = Some(effect_facts(
            deck_state_with_a_wrong_file(),
            true,
            true,
            None,
        ));
        let row = &health_rows(&facts)[1];
        assert_eq!(row.level, Level::Ok);
        assert!(
            row.detail.starts_with("Installed and matches"),
            "{}",
            row.detail
        );
    }

    // ------------------------------- pinned: Desktop Mode text and plans as they are

    fn details(facts: &Facts) -> Vec<String> {
        health_rows(facts)
            .into_iter()
            .map(|row| row.detail)
            .collect()
    }

    fn script_path() -> PathBuf {
        PathBuf::from("/usr/share/kwin/scripts/satur8-focus/contents/code/main.js")
    }

    #[test]
    fn kde_rows_toggle_and_plans_are_pinned() {
        assert_eq!(
            details(&healthy()),
            [
                "Working through KDE Plasma's compositor (kwin)",
                "Installed and matches this KDE version (6.8.3); idle until a profile needs it",
                "Running",
                "On: KDE tells Satur8 which window is focused",
            ]
        );
        assert_eq!(
            switch_state(&healthy()),
            SwitchState {
                available: true,
                on: true,
                detail: "Each game's profile applies when its window is focused".into(),
            }
        );
        assert_eq!(
            plan_disable(&healthy()),
            vec![
                Action::SetFocusEnabled(false),
                Action::KwinReconfigure,
                Action::UnloadScript,
                Action::Systemctl(vec!["disable", "--now"]),
            ]
        );

        // Switching off on purpose.
        let mut off = healthy();
        off.daemon_running = Some(false);
        off.unit = UnitState::Disabled;
        off.script_enabled = false;
        off.script_loaded = Some(false);
        assert_eq!(
            details(&off)[2..],
            ["Off; only needed for automatic per-game switching", "Off"]
        );
        assert_eq!(
            switch_state(&off).detail,
            "Off. Game profiles are not applied automatically"
        );
        assert_eq!(
            plan_enable(&off),
            vec![
                Action::Systemctl(vec!["enable", "--now"]),
                Action::SetFocusEnabled(true),
                Action::KwinReconfigure,
                Action::LoadScript(script_path()),
            ]
        );
        assert_eq!(plan_repair(&off), vec![Action::RepairEffect]);

        // Wanted, but the service and the script are not running.
        let mut partly = healthy();
        partly.daemon_running = Some(false);
        partly.script_loaded = Some(false);
        assert_eq!(
            details(&partly)[2..],
            [
                "Not running, so games are not switched automatically",
                "Turned on, but not running in this session. Repair starts it",
            ]
        );
        assert_eq!(
            switch_state(&partly).detail,
            "Partly on (service starts at login, detection turned on). Turn on to finish"
        );
        assert_eq!(
            plan_repair(&partly),
            vec![
                Action::RepairEffect,
                Action::Systemctl(vec!["start"]),
                Action::LoadScript(script_path()),
            ]
        );
        assert_eq!(
            Action::RepairEffect.describe(),
            "Check and repair the saturation effect"
        );
    }

    fn desktop(session: SessionType, desktop: Desktop, gpu: Gpu, backend: &str) -> Facts {
        Facts {
            env: Environment {
                session,
                desktop,
                gpu,
            },
            backend: Some(backend.into()),
            effect: None,
            kwin_version: None,
            daemon_running: Some(false),
            unit: UnitState::Disabled,
            script_enabled: false,
            script_loaded: None,
            script_path: None,
            color_management_off: false,
        }
    }

    #[test]
    fn gnome_hyprland_and_nvidia_rows_toggle_and_plans_are_pinned() {
        for (facts, color) in [
            (
                desktop(
                    SessionType::Wayland,
                    Desktop::Gnome,
                    Gpu::Intel,
                    "gnome-shell",
                ),
                "Working through the GNOME Shell extension (gnome-shell)",
            ),
            (
                desktop(
                    SessionType::Wayland,
                    Desktop::Hyprland,
                    Gpu::Amd,
                    "hyprland",
                ),
                "Working through a Hyprland screen shader (hyprland)",
            ),
            (
                desktop(SessionType::X11, Desktop::Kde, Gpu::Nvidia, "nv-control"),
                "Working through the NVIDIA driver setting (nv-control)",
            ),
        ] {
            assert_eq!(
                details(&facts),
                [
                    color,
                    "Only used on KDE Plasma (Wayland)",
                    "Only needed for per-game switching on KDE Plasma (Wayland)",
                    "Only available on KDE Plasma (Wayland)",
                ]
            );
            assert_eq!(
                levels(&facts)[1..],
                [
                    Level::NotApplicable,
                    Level::NotApplicable,
                    Level::NotApplicable
                ]
            );
            assert_eq!(
                switch_state(&facts),
                SwitchState {
                    available: false,
                    on: false,
                    detail: format!(
                        "Needs KDE Plasma on Wayland; this session is {} / {}",
                        facts.env.session, facts.env.desktop
                    ),
                }
            );
            assert!(plan_repair(&facts).is_empty());
            let mut running = facts.clone();
            running.daemon_running = Some(true);
            running.unit = UnitState::Enabled;
            assert_eq!(details(&running)[2], "Running");
            assert!(plan_repair(&running).is_empty());
        }
    }

    #[test]
    fn game_mode_names_the_first_row_when_no_profile_is_active() {
        let mut live = Live {
            daemon_running: true,
            active_profile: Some(String::new()),
            ..Live::default()
        };
        assert_eq!(describe_active(&live, true, false), "None, General");
        assert_eq!(
            describe_active(&live, false, false),
            "None, desktop default"
        );
        // No active profile is never "off", whatever the caller passes.
        assert_eq!(describe_active(&live, true, true), "None, General");
        live.active_profile = Some("ELDEN RING".into());
        assert_eq!(describe_active(&live, true, false), "ELDEN RING");
        assert_eq!(
            describe_active(&live, true, true),
            "ELDEN RING, off: normal colors"
        );
    }

    #[test]
    fn the_loaded_gamescope_table_says_what_is_applied() {
        use satur8_gamescope::session::Look;
        assert_eq!(applied_from_look(&Look::None), Some(1.0));
        assert_eq!(
            applied_from_look(&Look::Satur8(satur8_core::Saturation::new(1.75))),
            Some(1.75)
        );
        assert_eq!(
            applied_from_look(&Look::Foreign("/x/steam.cube".into())),
            None
        );
    }

    /// With `GAMESCOPE_COLOR_MANAGEMENT_DISABLE` set, gamescope ignores the
    /// look, so a loaded Satur8 table is not what the screen shows.
    #[test]
    fn a_loaded_table_is_not_applied_while_color_management_is_off() {
        use satur8_gamescope::session::{ColorState, Look};
        let state = |disabled: bool, look: Look| ColorState {
            pid: Some(6366),
            color_management_disabled: disabled,
            look,
            sdr_gamut_wideness: None,
            virtual_white: None,
            night_mode: None,
        };
        let table = || Look::Satur8(satur8_core::Saturation::new(1.75));
        // Color management on: the table's value, as before.
        assert_eq!(applied_in_game_mode(&state(false, table())), Some(1.75));
        assert_eq!(applied_in_game_mode(&state(false, Look::None)), Some(1.0));
        assert_eq!(
            applied_in_game_mode(&state(false, Look::Foreign("/x/steam.cube".into()))),
            None
        );
        // Off: normal colors, whatever is loaded.
        assert_eq!(applied_in_game_mode(&state(true, table())), Some(1.0));
        assert_eq!(applied_in_game_mode(&state(true, Look::None)), Some(1.0));
        assert_eq!(
            applied_in_game_mode(&state(true, Look::Foreign("/x/steam.cube".into()))),
            Some(1.0)
        );
        // And the line under the value says why.
        assert_eq!(
            applied_note(1.0, true),
            "gamescope's color management is off, so Satur8's setting is not shown"
        );
        assert_eq!(
            applied_note(1.0, false),
            "normal colors, read from the compositor"
        );
        assert_eq!(applied_note(1.75, false), "read from the compositor");
    }

    /// With gamescope's color management off, Satur8's table changes nothing
    /// on screen, so the first Health row must not say it is working.
    #[test]
    fn color_control_is_not_working_while_gamescopes_color_management_is_off() {
        let mut facts = game(Some(true), UnitState::Enabled);
        let color = |facts: &Facts| health_rows(facts).remove(0);
        // On: as before.
        let row = color(&facts);
        assert_eq!(row.label, "Color control");
        assert_eq!(row.level, Level::Ok);
        assert_eq!(row.detail, "Working through Steam Game Mode (gamescope)");

        facts.color_management_off = true;
        let row = color(&facts);
        assert_eq!(row.label, "Color control");
        assert_eq!(row.level, Level::Problem);
        assert!(!row.detail.contains("Working"));
        assert_eq!(
            row.detail,
            "gamescope's color management is off, so Satur8's setting is not shown"
        );
        // The same words as the line under the "Right now" value.
        assert_eq!(row.detail, applied_note(1.0, true));
        // The Game Mode page lists the row.
        assert_eq!(game_mode_rows(health_rows(&facts))[0], row);

        // The flag is gamescope's; a desktop session's row does not change.
        let mut kde = healthy();
        kde.color_management_off = true;
        assert_eq!(color(&kde).level, Level::Ok);
    }

    #[test]
    fn the_game_mode_page_keeps_the_service_row_and_drops_kdes() {
        let labels = |facts: &Facts| -> Vec<&'static str> {
            game_mode_rows(health_rows(facts))
                .into_iter()
                .map(|row| row.label)
                .collect()
        };
        // Service off and not asked for: still listed, so it can be seen.
        assert_eq!(
            labels(&game(Some(false), UnitState::Disabled)),
            ["Color control", "Background service"]
        );
        assert_eq!(
            labels(&game(Some(false), UnitState::Unknown)),
            ["Color control", "Background service"]
        );
        // Running: the detection row applies and shows.
        assert_eq!(
            labels(&game(Some(true), UnitState::Enabled)),
            ["Color control", "Background service", "Game detection"]
        );
    }

    // ------------------------------------------- systemd without systemctl

    /// systemd's answers on the session bus, read on 2026-11-22: the state
    /// word for an installed unit, an error for one that does not exist.
    #[test]
    fn systemds_own_answer_maps_like_systemctls() {
        assert_eq!(unit_state_from_manager(Ok("enabled")), UnitState::Enabled);
        assert_eq!(unit_state_from_manager(Ok("disabled")), UnitState::Disabled);
        assert_eq!(unit_state_from_manager(Ok("masked")), UnitState::Disabled);
        assert_eq!(
            unit_state_from_manager(Err("No such file or directory")),
            UnitState::NotInstalled
        );
        assert_eq!(
            unit_state_from_manager(Err(
                "org.freedesktop.DBus.Error.ServiceUnknown: The name is not activatable"
            )),
            UnitState::Unknown
        );
        assert_eq!(unit_state_from_manager(Ok("")), UnitState::Unknown);
    }

    #[test]
    fn the_systemctl_steps_have_the_same_calls_on_the_bus() {
        use ManagerStep::*;
        assert_eq!(manager_steps(&["enable", "--now"]), [Enable, Reload, Start]);
        assert_eq!(
            manager_steps(&["disable", "--now"]),
            [Disable, Reload, Stop]
        );
        assert_eq!(manager_steps(&["start"]), [Start]);
        assert_eq!(manager_steps(&["enable"]), [Enable, Reload]);
        // Every argument list a plan is built with is covered.
        let facts = game(Some(false), UnitState::Disabled);
        for plan in [
            plan_enable(&facts),
            plan_disable(&facts),
            plan_repair(&facts),
        ] {
            for action in plan {
                if let Action::Systemctl(args) = action {
                    assert!(!manager_steps(&args).is_empty(), "{args:?}");
                }
            }
        }
    }

    // ----------------------------------------------------------- Steam Game Mode

    /// Game Mode as the Deck reports it: the gamescope backend is selected, no
    /// KWin answers, and the KWin script is still installed from Desktop Mode.
    fn game(daemon_running: Option<bool>, unit: UnitState) -> Facts {
        Facts {
            env: Environment {
                session: SessionType::X11,
                desktop: Desktop::Other,
                gpu: Gpu::Amd,
            },
            backend: Some("gamescope".into()),
            effect: None,
            kwin_version: None,
            daemon_running,
            unit,
            script_enabled: false,
            script_loaded: None,
            script_path: Some(script_path()),
            color_management_off: false,
        }
    }

    fn rows_of(facts: &Facts) -> Vec<(Level, String)> {
        health_rows(facts)
            .into_iter()
            .map(|row| (row.level, row.detail))
            .collect()
    }

    fn r(level: Level, detail: &str) -> (Level, String) {
        (level, detail.to_string())
    }

    #[test]
    fn game_mode_with_the_service_off() {
        let facts = game(Some(false), UnitState::Disabled);
        assert!(game_mode(&facts));
        assert_eq!(
            rows_of(&facts),
            [
                r(Level::Ok, "Working through Steam Game Mode (gamescope)"),
                r(Level::NotApplicable, "Only used on KDE Plasma (Wayland)"),
                r(
                    Level::NotApplicable,
                    "Off; only needed for automatic per-game switching"
                ),
                r(Level::NotApplicable, "Built into the service in Game Mode"),
            ]
        );
        assert_eq!(
            switch_state(&facts),
            SwitchState {
                available: true,
                on: false,
                detail: "Off. Game profiles are not applied automatically".into(),
            }
        );
        assert!(plan_repair(&facts).is_empty());
    }

    #[test]
    fn game_mode_with_the_service_enabled_but_not_running() {
        let facts = game(Some(false), UnitState::Enabled);
        let rows = rows_of(&facts);
        assert_eq!(
            rows[2],
            r(
                Level::Problem,
                "Not running, so games are not switched automatically"
            )
        );
        assert_eq!(
            rows[3],
            r(Level::NotApplicable, "Built into the service in Game Mode")
        );
        assert_eq!(
            switch_state(&facts),
            SwitchState {
                available: true,
                on: false,
                detail: "Partly on (service starts at login). Turn on to finish".into(),
            }
        );
        assert_eq!(plan_repair(&facts), vec![Action::Systemctl(vec!["start"])]);
        assert!(plan_starts_daemon(&plan_repair(&facts)));
    }

    #[test]
    fn game_mode_with_the_service_running() {
        let facts = game(Some(true), UnitState::Enabled);
        assert_eq!(
            rows_of(&facts),
            [
                r(Level::Ok, "Working through Steam Game Mode (gamescope)"),
                r(Level::NotApplicable, "Only used on KDE Plasma (Wayland)"),
                r(Level::Ok, "Running"),
                r(Level::Ok, "Built into the service in Game Mode"),
            ]
        );
        assert_eq!(
            switch_state(&facts),
            SwitchState {
                available: true,
                on: true,
                detail: "Each game's profile applies when its window is focused".into(),
            }
        );
        assert!(plan_repair(&facts).is_empty());

        // Running now, but not set to start with the session.
        let facts = game(Some(true), UnitState::Disabled);
        assert_eq!(
            rows_of(&facts)[2],
            r(
                Level::Ok,
                "Running, but it will not start at the next login"
            )
        );
        assert_eq!(
            switch_state(&facts).detail,
            "Partly on (service running). Turn on to finish"
        );
    }

    #[test]
    fn game_mode_without_the_service_installed() {
        let facts = game(Some(false), UnitState::NotInstalled);
        assert_eq!(
            rows_of(&facts)[2],
            r(
                Level::NotApplicable,
                "Not installed (no satur8-daemon service found)"
            )
        );
        let switch = switch_state(&facts);
        assert!(!switch.available && !switch.on);
        assert_eq!(
            switch.detail,
            "The background service is not installed; reinstall Satur8"
        );
    }

    #[test]
    fn game_mode_toggle_is_only_the_service() {
        let facts = game(Some(false), UnitState::Disabled);
        assert_eq!(
            plan_enable(&facts),
            vec![Action::Systemctl(vec!["enable", "--now"])]
        );
        assert!(plan_starts_daemon(&plan_enable(&facts)));
        let facts = game(Some(true), UnitState::Enabled);
        assert_eq!(
            plan_disable(&facts),
            vec![Action::Systemctl(vec!["disable", "--now"])]
        );
    }

    #[test]
    fn game_mode_repair_never_touches_the_kwin_effect_or_script() {
        // The KWin script setting from Desktop Mode does not count as wanting
        // switching in Game Mode, and a KDE-looking environment does not bring
        // the KWin steps back.
        for env_kde in [false, true] {
            let mut facts = game(Some(false), UnitState::Disabled);
            facts.script_enabled = true;
            facts.script_loaded = Some(false);
            if env_kde {
                facts.env = kde();
            }
            assert!(plan_repair(&facts).is_empty(), "{facts:?}");
            facts.unit = UnitState::Enabled;
            assert_eq!(plan_repair(&facts), vec![Action::Systemctl(vec!["start"])]);
            assert_eq!(
                plan_enable(&facts),
                vec![Action::Systemctl(vec!["enable", "--now"])]
            );
        }
    }
}
