//! Automatic per-game switching on the real system: the read-only probe and
//! [`RealSystem`], which carries out the steps planned by
//! `satur8_core::switching`. Used by the desktop app's Status page toggle and
//! by `satur8 auto`.

use std::path::PathBuf;
use std::process::Command;

use satur8_core::switching::{
    focus_enabled_in, kde_wayland, manager_steps, parse_unit_state, unit_state_from_manager,
    Action, ManagerStep, SwitchFacts, System, UnitState, DAEMON_UNIT, FOCUS_KEY, FOCUS_SCRIPT,
};
use satur8_core::Environment;
use zbus::blocking::Connection;

/// The background service's name on the session bus.
pub const DAEMON_BUS: &str = "org.satur8.Daemon";
const KWIN_BUS: &str = "org.kde.KWin";
const SYSTEMD_BUS: &str = "org.freedesktop.systemd1";
const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
const SYSTEMD_MANAGER: &str = "org.freedesktop.systemd1.Manager";

/// The real thing. Spawns systemctl and kwriteconfig6 (system tools, not a
/// satur8 binary) and talks to KWin over D-Bus.
pub struct RealSystem;

impl System for RealSystem {
    fn run(&mut self, action: &Action) -> Result<String, String> {
        match action {
            Action::RepairEffect => satur8_kwin::repair::repair().map(|outcome| outcome.describe()),
            Action::Systemctl(args) => {
                if on_path("systemctl") {
                    let mut cmd = Command::new("systemctl");
                    cmd.arg("--user").args(args).arg(DAEMON_UNIT);
                    run_command(cmd)
                } else {
                    // Steam starts the app inside its runtime container,
                    // which has no systemctl; the user's systemd is still on
                    // the session bus.
                    let conn = session()?;
                    for step in manager_steps(args) {
                        run_manager_step(&conn, step)?;
                    }
                    Ok(String::new())
                }
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

/// Whether `name` has an owner on the bus; `None` if the bus did not answer.
pub fn name_has_owner(conn: &Connection, name: &str) -> Option<bool> {
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

/// KWin's isScriptLoaded for the focus script; `None` if KWin did not answer.
pub fn script_loaded(conn: &Connection) -> Option<bool> {
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

/// `systemctl --user is-enabled satur8-daemon.service`, or the same question
/// put to systemd over the session bus where there is no systemctl.
pub fn unit_state() -> UnitState {
    match Command::new("systemctl")
        .args(["--user", "is-enabled", DAEMON_UNIT])
        .output()
    {
        Ok(out) => parse_unit_state(
            &String::from_utf8_lossy(&out.stdout),
            &String::from_utf8_lossy(&out.stderr),
        ),
        // No systemctl to run: Steam's runtime container, where the app runs
        // when it is started from the Steam library. Ask systemd itself.
        Err(_) => {
            let answer = session().and_then(|conn| {
                call::<String>(
                    &conn,
                    SYSTEMD_BUS,
                    SYSTEMD_PATH,
                    SYSTEMD_MANAGER,
                    "GetUnitFileState",
                    &(DAEMON_UNIT,),
                )
            });
            unit_state_from_manager(answer.as_deref().map_err(String::as_str))
        }
    }
}

fn run_manager_step(conn: &Connection, step: ManagerStep) -> Result<(), String> {
    type Changes = Vec<(String, String, String)>;
    fn manager<R>(
        conn: &Connection,
        method: &str,
        body: &(impl serde::Serialize + zbus::zvariant::DynamicType),
    ) -> Result<R, String>
    where
        R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
    {
        call(
            conn,
            SYSTEMD_BUS,
            SYSTEMD_PATH,
            SYSTEMD_MANAGER,
            method,
            body,
        )
    }
    match step {
        ManagerStep::Enable => {
            manager::<(bool, Changes)>(conn, "EnableUnitFiles", &(vec![DAEMON_UNIT], false, false))
                .map(|_| ())
        }
        ManagerStep::Disable => {
            manager::<Changes>(conn, "DisableUnitFiles", &(vec![DAEMON_UNIT], false)).map(|_| ())
        }
        ManagerStep::Reload => manager::<()>(conn, "Reload", &()),
        ManagerStep::Start => {
            manager::<zbus::zvariant::OwnedObjectPath>(conn, "StartUnit", &(DAEMON_UNIT, "replace"))
                .map(|_| ())
        }
        ManagerStep::Stop => {
            manager::<zbus::zvariant::OwnedObjectPath>(conn, "StopUnit", &(DAEMON_UNIT, "replace"))
                .map(|_| ())
        }
    }
}

/// Whether a program of this name is on `PATH`.
fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
}

/// kwinrc [Plugins] satur8-focusEnabled; false when kwinrc cannot be read.
pub fn script_enabled() -> bool {
    config_home()
        .and_then(|d| std::fs::read_to_string(d.join("kwinrc")).ok())
        .map(|text| focus_enabled_in(&text))
        .unwrap_or(false)
}

/// The installed focus script's main.js: the per-user data folder first,
/// then the system ones.
pub fn find_script() -> Option<PathBuf> {
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

/// Gather [`SwitchFacts`] for this session. Read-only: D-Bus queries,
/// `systemctl --user is-enabled`, and file reads.
pub fn probe() -> SwitchFacts {
    let env = Environment::detect();
    let backend = crate::select_backend().ok().map(|b| b.name().to_string());
    let conn = session().ok();
    let script_loaded = if kde_wayland(&env) {
        conn.as_ref().and_then(script_loaded)
    } else {
        None
    };
    SwitchFacts {
        env,
        backend,
        daemon_running: conn.as_ref().and_then(|c| name_has_owner(c, DAEMON_BUS)),
        unit: unit_state(),
        script_enabled: script_enabled(),
        script_loaded,
        script_path: find_script(),
    }
}
