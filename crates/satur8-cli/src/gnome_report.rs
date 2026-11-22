//! The GNOME part of `satur8 doctor` and `satur8 status`.
//!
//! The GNOME backend only works while Satur8's GNOME Shell extension is
//! installed, loaded by GNOME Shell, enabled, and serving its D-Bus name. This
//! module reads those facts (`gather`, the only part that touches the disk or
//! the bus) and turns them into report lines and one next step with pure
//! functions, so every case is unit-tested.
//!
//! GNOME Shell facts this relies on, read in its 49.10, 50.2 and 51.1 sources:
//! - `ExtensionState` in `js/misc/extensionUtils.js` has the same numbers in
//!   all three: 1 ACTIVE, 2 INACTIVE, 3 ERROR, 4 OUT_OF_DATE, 5 DOWNLOADING,
//!   6 INITIALIZED, 7 DEACTIVATING, 8 ACTIVATING, 99 UNINSTALLED.
//! - `GetExtensionInfo` (`js/ui/shellDBus.js`) returns an empty dictionary for
//!   a UUID the shell has not loaded, and sends `state` as a double.
//! - The shell scans the extension folders once, when it starts
//!   (`_loadExtensions` in `js/ui/extensionSystem.js`): the per-user data dir
//!   first, then the system data dirs, and the first folder for a UUID wins.
//! - An extension is out of date when no `shell-version` entry starts with the
//!   running major version (`_isOutOfDate` in the same file).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use satur8_backend::{GNOME_EXTENSION_UUID, GNOME_IFACE, GNOME_PATH, GNOME_SERVICE};
use zbus::blocking::Connection;
use zbus::zvariant::{OwnedValue, Value};

/// Where the extension folder is, as GNOME Shell would find it.
#[derive(Debug, Clone, PartialEq)]
pub enum Install {
    NotInstalled,
    Installed {
        dir: PathBuf,
        per_user: bool,
        metadata: Metadata,
    },
}

/// What the folder's `metadata.json` says about supported shell versions.
#[derive(Debug, Clone, PartialEq)]
pub enum Metadata {
    Versions(Vec<String>),
    /// The folder has no `metadata.json`.
    Missing,
    /// The file exists but could not be read.
    Unreadable(String),
    /// No `shell-version` key was found (also what text that is not JSON gives).
    NoShellVersion,
    /// The `shell-version` value is not a list of strings, or the file ends early.
    Invalid,
}

/// GNOME Shell's `ExtensionState`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExtState {
    Active,
    Inactive,
    Error,
    OutOfDate,
    Downloading,
    Initialized,
    Deactivating,
    Activating,
    Uninstalled,
    Other(f64),
}

impl ExtState {
    /// GNOME Shell sends the state as a D-Bus double. Only a whole number in
    /// range names a state: it is converted to an integer once and matched.
    pub fn from_number(number: f64) -> ExtState {
        // `contains` is false for NaN, so NaN is reported as unknown too.
        if !(0.0..=255.0).contains(&number) || number.fract() > 0.0 {
            return ExtState::Other(number);
        }
        match number as u8 {
            1 => ExtState::Active,
            2 => ExtState::Inactive,
            3 => ExtState::Error,
            4 => ExtState::OutOfDate,
            5 => ExtState::Downloading,
            6 => ExtState::Initialized,
            7 => ExtState::Deactivating,
            8 => ExtState::Activating,
            99 => ExtState::Uninstalled,
            _ => ExtState::Other(number),
        }
    }

    fn name(self) -> String {
        match self {
            ExtState::Active => "ACTIVE".into(),
            ExtState::Inactive => "INACTIVE".into(),
            ExtState::Error => "ERROR".into(),
            ExtState::OutOfDate => "OUT_OF_DATE".into(),
            ExtState::Downloading => "DOWNLOADING".into(),
            ExtState::Initialized => "INITIALIZED".into(),
            ExtState::Deactivating => "DEACTIVATING".into(),
            ExtState::Activating => "ACTIVATING".into(),
            ExtState::Uninstalled => "UNINSTALLED".into(),
            ExtState::Other(n) => format!("unknown ({n})"),
        }
    }
}

/// What GNOME Shell says about the extension.
#[derive(Debug, Clone, PartialEq)]
pub enum ShellView {
    /// `GetExtensionInfo` answered with an empty dictionary (or no state): the
    /// running shell has not loaded this UUID.
    NotLoaded,
    Loaded {
        state: ExtState,
        error: Option<String>,
    },
    /// The question could not be asked or answered.
    QueryFailed(String),
}

/// Everything the GNOME report is built from.
#[derive(Debug, Clone, PartialEq)]
pub struct GnomeFacts {
    /// The running shell's version, or why it could not be read.
    pub shell: Result<String, String>,
    /// GNOME Shell's `UserExtensionsEnabled` (false when the user turned all
    /// extensions off); `None` when unknown.
    pub user_extensions_enabled: Option<bool>,
    pub install: Install,
    pub view: ShellView,
    /// Whether the extension's bus name is owned; `None` when unknown.
    pub service_owned: Option<bool>,
    /// The extension's `Saturation` property when it could be read.
    pub saturation: Option<f64>,
}

/// The one thing to do next.
#[derive(Debug, Clone, PartialEq)]
pub enum NextStep {
    Ready,
    ShellUnreachable,
    NotInstalled,
    /// Installed after the shell started.
    LogOutAndIn,
    /// The folder is there but its `metadata.json` is missing or damaged.
    Reinstall,
    UserExtensionsOff,
    Enable,
    OutOfDate {
        listed: Vec<String>,
        running: String,
    },
    LoadError,
    /// ACTIVATING, DEACTIVATING or DOWNLOADING.
    Busy,
    /// ACTIVE, but the bus name is not owned.
    ServiceMissing,
    QueryFailed(String),
}

/// The running major version, as GNOME Shell computes it: the text before the
/// first dot of its version (`51.1` gives `51`, `51.rc` gives `51`).
pub fn major_version(shell_version: &str) -> &str {
    shell_version.split('.').next().unwrap_or(shell_version)
}

/// GNOME Shell's own out-of-date rule: supported when any listed entry starts
/// with the running major version.
pub fn lists_version(listed: &[String], major: &str) -> bool {
    listed.iter().any(|entry| entry.starts_with(major))
}

pub fn next_step(facts: &GnomeFacts) -> NextStep {
    let running = match &facts.shell {
        Ok(version) => major_version(version).to_string(),
        Err(_) => return NextStep::ShellUnreachable,
    };
    let listed = match &facts.install {
        Install::Installed {
            metadata: Metadata::Versions(listed),
            ..
        } => listed.clone(),
        _ => Vec::new(),
    };
    match &facts.view {
        ShellView::Loaded { state, .. } => match state {
            ExtState::Active => match facts.service_owned {
                Some(false) => NextStep::ServiceMissing,
                _ => NextStep::Ready,
            },
            ExtState::Error => NextStep::LoadError,
            ExtState::OutOfDate => NextStep::OutOfDate { listed, running },
            ExtState::Activating | ExtState::Deactivating | ExtState::Downloading => NextStep::Busy,
            ExtState::Uninstalled => not_loaded_step(&facts.install, listed, running),
            ExtState::Inactive | ExtState::Initialized | ExtState::Other(_) => {
                if facts.user_extensions_enabled == Some(false) {
                    NextStep::UserExtensionsOff
                } else {
                    NextStep::Enable
                }
            }
        },
        ShellView::NotLoaded => not_loaded_step(&facts.install, listed, running),
        ShellView::QueryFailed(error) => match facts.install {
            Install::NotInstalled => NextStep::NotInstalled,
            Install::Installed { .. } => NextStep::QueryFailed(error.clone()),
        },
    }
}

/// The shell does not have the extension: decide from what is on disk.
fn not_loaded_step(install: &Install, listed: Vec<String>, running: String) -> NextStep {
    match install {
        Install::NotInstalled => NextStep::NotInstalled,
        Install::Installed { metadata, .. } => match metadata {
            Metadata::Versions(_) if !lists_version(&listed, &running) => {
                NextStep::OutOfDate { listed, running }
            }
            Metadata::Versions(_) => NextStep::LogOutAndIn,
            Metadata::Missing
            | Metadata::Unreadable(_)
            | Metadata::NoShellVersion
            | Metadata::Invalid => NextStep::Reinstall,
        },
    }
}

/// The text after "Next step: ". Ends without a period; the caller adds it.
pub fn advice(step: &NextStep) -> String {
    let uuid = GNOME_EXTENSION_UUID;
    match step {
        NextStep::Ready => "nothing to do; `satur8 on` works".to_string(),
        NextStep::ShellUnreachable => "run `satur8 doctor` in a terminal inside the GNOME \
             session; GNOME Shell did not answer on this session bus"
            .to_string(),
        NextStep::NotInstalled => "run Satur8's installer (`packaging/install.sh` from the \
             Satur8 release), then log out and back in"
            .to_string(),
        NextStep::LogOutAndIn => "log out and back in; GNOME Shell looks for extensions only \
             when it starts, and on Wayland it cannot restart without logging out"
            .to_string(),
        NextStep::Reinstall => "run Satur8's installer again (`packaging/install.sh`) to \
             replace the damaged extension folder, then log out and back in"
            .to_string(),
        NextStep::UserExtensionsOff => format!(
            "extensions are turned off for this user; run \
             `gsettings set org.gnome.shell disable-user-extensions false`, then \
             `gnome-extensions enable {uuid}`"
        ),
        NextStep::Enable => format!("run `gnome-extensions enable {uuid}`"),
        NextStep::OutOfDate { listed, running } if listed.is_empty() => format!(
            "this Satur8's extension does not list GNOME Shell {running}; install a newer \
             Satur8 release that supports GNOME {running}"
        ),
        NextStep::OutOfDate { listed, running } => format!(
            "this Satur8's extension lists GNOME Shell {}, not {running}; install a newer \
             Satur8 release that supports GNOME {running}",
            listed.join(", ")
        ),
        NextStep::LoadError => "GNOME Shell could not load the extension (the error is above); \
             please report it with this `satur8 doctor` output"
            .to_string(),
        NextStep::Busy => "GNOME Shell is still switching the extension on or off; run \
             `satur8 doctor` again in a moment"
            .to_string(),
        NextStep::ServiceMissing => format!(
            "run `gnome-extensions disable {uuid}` and then `gnome-extensions enable {uuid}`; \
             if the service is still missing, please report it with this `satur8 doctor` output"
        ),
        NextStep::QueryFailed(error) => format!(
            "couldn't ask GNOME Shell about the extension ({error}); run `satur8 doctor` \
             again, and report it with this output if it keeps failing"
        ),
    }
}

/// The GNOME block of `satur8 doctor`.
pub fn doctor_lines(facts: &GnomeFacts) -> Vec<String> {
    let mut lines = Vec::new();
    let running = match &facts.shell {
        Ok(version) => {
            lines.push(format!("  [ok] GNOME Shell {version} reachable over D-Bus"));
            Some(major_version(version).to_string())
        }
        Err(error) => {
            lines.push(format!(
                "  [!!] GNOME Shell not reachable over D-Bus: {error}"
            ));
            None
        }
    };

    match &facts.install {
        Install::NotInstalled => lines.push(format!(
            "  [!!] Satur8 extension not installed ({GNOME_EXTENSION_UUID})"
        )),
        Install::Installed {
            dir,
            per_user,
            metadata,
        } => {
            let scope = if *per_user { "per-user" } else { "system-wide" };
            lines.push(format!(
                "  [ok] Satur8 extension installed ({scope}): {}",
                dir.display()
            ));
            lines.push(metadata_line(metadata, running.as_deref()));
        }
    }

    match &facts.view {
        ShellView::Loaded { state, error } => {
            let mark = match state {
                ExtState::Active => "[ok]",
                ExtState::Error | ExtState::OutOfDate => "[!!]",
                _ => "[..]",
            };
            lines.push(format!(
                "  {mark} extension state in GNOME Shell: {}",
                state.name()
            ));
            if let (ExtState::Error, Some(error)) = (state, error) {
                if !error.is_empty() {
                    lines.push(format!("       error: {error}"));
                }
            }
        }
        ShellView::NotLoaded if facts.shell.is_ok() => {
            lines.push("  [..] extension state in GNOME Shell: not loaded".to_string())
        }
        ShellView::NotLoaded => {}
        ShellView::QueryFailed(error) => {
            if facts.shell.is_ok() {
                lines.push(format!(
                    "  [!!] couldn't read the extension state from GNOME Shell: {error}"
                ));
            }
        }
    }

    lines.push(match facts.service_owned {
        Some(true) => format!("  [ok] {GNOME_SERVICE} is on the session bus: Satur8 can apply"),
        Some(false) => {
            format!("  [!!] {GNOME_SERVICE} is not on the session bus: Satur8 cannot apply")
        }
        None => format!("  [..] couldn't check whether {GNOME_SERVICE} is on the session bus"),
    });
    lines.push(format!("       Next step: {}.", advice(&next_step(facts))));
    lines
}

fn metadata_line(metadata: &Metadata, running: Option<&str>) -> String {
    match metadata {
        Metadata::Versions(listed) => {
            let list = if listed.is_empty() {
                "no GNOME Shell versions".to_string()
            } else {
                format!("GNOME Shell {}", listed.join(", "))
            };
            match running {
                Some(major) if lists_version(listed, major) => {
                    format!("       metadata lists {list}; includes {major}")
                }
                Some(major) => format!("  [!!] metadata lists {list}; does not include {major}"),
                None => format!("       metadata lists {list}"),
            }
        }
        Metadata::Missing => "  [!!] metadata.json is missing from the extension folder".into(),
        Metadata::Unreadable(error) => format!("  [!!] couldn't read metadata.json: {error}"),
        Metadata::NoShellVersion => {
            "  [!!] metadata.json has no shell-version list (or is not valid JSON)".into()
        }
        Metadata::Invalid => "  [!!] metadata.json's shell-version list is not valid".into(),
    }
}

/// The `satur8 status` line for GNOME.
pub fn status_line(facts: &GnomeFacts) -> String {
    let step = next_step(facts);
    if step == NextStep::Ready {
        let version = facts.shell.as_deref().unwrap_or("unknown");
        let mut line =
            format!("backend gnome-shell: available, extension active (GNOME Shell {version})");
        if let Some(saturation) = facts.saturation {
            line.push_str(&format!(", saturation {saturation:.2}"));
        }
        return line;
    }
    let reason = match &step {
        NextStep::Ready => "ready",
        NextStep::ShellUnreachable => "GNOME Shell not reachable",
        NextStep::NotInstalled => "extension not installed",
        NextStep::LogOutAndIn => "extension installed, GNOME Shell has not loaded it yet",
        NextStep::Reinstall => "extension folder damaged",
        NextStep::UserExtensionsOff => "extensions turned off",
        NextStep::Enable => "extension disabled",
        NextStep::OutOfDate { .. } => "extension does not support this GNOME Shell",
        NextStep::LoadError => "extension failed to load",
        NextStep::Busy => "extension switching on or off",
        NextStep::ServiceMissing => "extension active but its D-Bus service is missing",
        NextStep::QueryFailed(_) => "couldn't read the extension state",
    };
    format!("backend gnome-shell: unavailable, {reason} (see `satur8 doctor`)")
}

// ---- Reading the facts (disk and bus) ----

/// The folders GNOME Shell searches for `uuid`, in its order, each with
/// whether it is the per-user one. Mirrors GLib's `g_get_user_data_dir` and
/// `g_get_system_data_dirs`: `$XDG_DATA_HOME` or `~/.local/share`, then each
/// `$XDG_DATA_DIRS` entry or `/usr/local/share` and `/usr/share`.
pub fn extension_dirs(
    data_home: Option<&str>,
    home: Option<&str>,
    data_dirs: Option<&str>,
    uuid: &str,
) -> Vec<(PathBuf, bool)> {
    let mut dirs = Vec::new();
    let user = match data_home.filter(|value| !value.is_empty()) {
        Some(data_home) => Some(PathBuf::from(data_home)),
        None => home
            .filter(|value| !value.is_empty())
            .map(|home| Path::new(home).join(".local/share")),
    };
    if let Some(user) = user {
        dirs.push((user, true));
    }
    let system = data_dirs
        .filter(|value| !value.is_empty())
        .unwrap_or("/usr/local/share/:/usr/share/");
    for entry in system.split(':').filter(|entry| !entry.is_empty()) {
        dirs.push((PathBuf::from(entry), false));
    }
    dirs.into_iter()
        .map(|(base, per_user)| (base.join("gnome-shell/extensions").join(uuid), per_user))
        .collect()
}

/// Read the `shell-version` list from `metadata.json` text without a JSON
/// library: find the `"shell-version"` key outside any string, then read the
/// array of strings after it.
pub fn parse_shell_versions(text: &str) -> Metadata {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '"' {
            i += 1;
            continue;
        }
        let Some((key, after)) = read_string(&chars, i) else {
            return Metadata::Invalid;
        };
        let colon = skip_space(&chars, after);
        if key == "shell-version" && chars.get(colon) == Some(&':') {
            return read_string_array(&chars, skip_space(&chars, colon + 1));
        }
        i = after;
    }
    Metadata::NoShellVersion
}

fn skip_space(chars: &[char], mut i: usize) -> usize {
    while chars.get(i).is_some_and(|c| c.is_whitespace()) {
        i += 1;
    }
    i
}

/// Read a JSON string starting at the opening quote `start`. Returns the text
/// and the index after the closing quote, or `None` if it never closes.
fn read_string(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut out = String::new();
    let mut i = start + 1;
    while let Some(&c) = chars.get(i) {
        match c {
            '"' => return Some((out, i + 1)),
            '\\' => {
                let escaped = *chars.get(i + 1)?;
                match escaped {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    'b' => out.push('\u{8}'),
                    'f' => out.push('\u{c}'),
                    'u' => {
                        let hex: String = chars.get(i + 2..i + 6)?.iter().collect();
                        let code = u32::from_str_radix(&hex, 16).ok()?;
                        out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                        i += 4;
                    }
                    other => out.push(other),
                }
                i += 2;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    None
}

fn read_string_array(chars: &[char], start: usize) -> Metadata {
    if chars.get(start) != Some(&'[') {
        return Metadata::Invalid;
    }
    let mut listed = Vec::new();
    let mut i = skip_space(chars, start + 1);
    if chars.get(i) == Some(&']') {
        return Metadata::Versions(listed);
    }
    loop {
        if chars.get(i) != Some(&'"') {
            return Metadata::Invalid;
        }
        let Some((entry, after)) = read_string(chars, i) else {
            return Metadata::Invalid;
        };
        listed.push(entry);
        i = skip_space(chars, after);
        match chars.get(i) {
            Some(',') => i = skip_space(chars, i + 1),
            Some(']') => return Metadata::Versions(listed),
            _ => return Metadata::Invalid,
        }
    }
}

fn read_metadata(dir: &Path) -> Metadata {
    match std::fs::read_to_string(dir.join("metadata.json")) {
        Ok(text) => parse_shell_versions(&text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Metadata::Missing,
        Err(error) => Metadata::Unreadable(error.to_string()),
    }
}

fn find_install() -> Install {
    let var = |key: &str| std::env::var(key).ok();
    let data_home = var("XDG_DATA_HOME");
    let home = var("HOME");
    let data_dirs = var("XDG_DATA_DIRS");
    let dirs = extension_dirs(
        data_home.as_deref(),
        home.as_deref(),
        data_dirs.as_deref(),
        GNOME_EXTENSION_UUID,
    );
    for (dir, per_user) in dirs {
        if dir.is_dir() {
            let metadata = read_metadata(&dir);
            return Install::Installed {
                dir,
                per_user,
                metadata,
            };
        }
    }
    Install::NotInstalled
}

/// `org.freedesktop.DBus.Properties.Get` on one property.
fn get_property(
    conn: &Connection,
    service: &str,
    path: &str,
    iface: &str,
    name: &str,
) -> Result<OwnedValue, String> {
    let reply = conn
        .call_method(
            Some(service),
            path,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &(iface, name),
        )
        .map_err(|error| error.to_string())?;
    let body = reply.body();
    let value: Value = body.deserialize().map_err(|error| error.to_string())?;
    value.try_to_owned().map_err(|error| error.to_string())
}

fn extension_view(conn: &Connection) -> ShellView {
    let reply = match conn.call_method(
        Some("org.gnome.Shell"),
        "/org/gnome/Shell",
        Some("org.gnome.Shell.Extensions"),
        "GetExtensionInfo",
        &(GNOME_EXTENSION_UUID,),
    ) {
        Ok(reply) => reply,
        Err(error) => return ShellView::QueryFailed(error.to_string()),
    };
    let body = reply.body();
    let info: HashMap<String, OwnedValue> = match body.deserialize() {
        Ok(info) => info,
        Err(error) => return ShellView::QueryFailed(error.to_string()),
    };
    let Some(state) = info
        .get("state")
        .and_then(|value| f64::try_from(value).ok())
    else {
        return ShellView::NotLoaded;
    };
    let error = info
        .get("error")
        .and_then(|value| <&str>::try_from(value).ok())
        .map(str::to_string);
    ShellView::Loaded {
        state: ExtState::from_number(state),
        error,
    }
}

fn name_has_owner(conn: &Connection, name: &str) -> Option<bool> {
    let reply = conn
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "NameHasOwner",
            &(name,),
        )
        .ok()?;
    reply.body().deserialize::<bool>().ok()
}

/// Read every fact from the session bus and the disk.
pub fn gather() -> GnomeFacts {
    let install = find_install();
    let conn = match Connection::session() {
        Ok(conn) => conn,
        Err(error) => {
            let error = format!("session bus unreachable: {error}");
            return GnomeFacts {
                shell: Err(error.clone()),
                user_extensions_enabled: None,
                install,
                view: ShellView::QueryFailed(error),
                service_owned: None,
                saturation: None,
            };
        }
    };
    let shell = get_property(
        &conn,
        "org.gnome.Shell",
        "/org/gnome/Shell",
        "org.gnome.Shell",
        "ShellVersion",
    )
    .and_then(|value| String::try_from(value).map_err(|error| error.to_string()));
    let user_extensions_enabled = get_property(
        &conn,
        "org.gnome.Shell",
        "/org/gnome/Shell",
        "org.gnome.Shell.Extensions",
        "UserExtensionsEnabled",
    )
    .ok()
    .and_then(|value| bool::try_from(value).ok());
    let view = match &shell {
        Ok(_) => extension_view(&conn),
        Err(error) => ShellView::QueryFailed(error.clone()),
    };
    let service_owned = name_has_owner(&conn, GNOME_SERVICE);
    let saturation = if service_owned == Some(true) {
        get_property(&conn, GNOME_SERVICE, GNOME_PATH, GNOME_IFACE, "Saturation")
            .ok()
            .and_then(|value| f64::try_from(value).ok())
    } else {
        None
    };
    GnomeFacts {
        shell,
        user_extensions_enabled,
        install,
        view,
        service_owned,
        saturation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn versions(list: &[&str]) -> Vec<String> {
        list.iter().map(|entry| entry.to_string()).collect()
    }

    fn installed(metadata: Metadata) -> Install {
        Install::Installed {
            dir: PathBuf::from(
                "/home/u/.local/share/gnome-shell/extensions/satur8@satur8.github.io",
            ),
            per_user: true,
            metadata,
        }
    }

    fn up_to_51() -> Install {
        installed(Metadata::Versions(versions(&[
            "45", "46", "47", "48", "49", "50", "51",
        ])))
    }

    fn up_to_50() -> Install {
        installed(Metadata::Versions(versions(&[
            "45", "46", "47", "48", "49", "50",
        ])))
    }

    fn loaded(state: ExtState) -> ShellView {
        ShellView::Loaded { state, error: None }
    }

    /// A healthy GNOME 51 session: the starting point every case changes.
    fn healthy() -> GnomeFacts {
        GnomeFacts {
            shell: Ok("51.1".to_string()),
            user_extensions_enabled: Some(true),
            install: up_to_51(),
            view: loaded(ExtState::Active),
            service_owned: Some(true),
            saturation: Some(1.0),
        }
    }

    #[test]
    fn state_numbers_match_gnome_shell() {
        let names: Vec<String> = [
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 99.0, 42.0, 1.5, -1.0,
        ]
        .iter()
        .map(|&n| ExtState::from_number(n).name())
        .collect();
        assert_eq!(
            names,
            [
                "ACTIVE",
                "INACTIVE",
                "ERROR",
                "OUT_OF_DATE",
                "DOWNLOADING",
                "INITIALIZED",
                "DEACTIVATING",
                "ACTIVATING",
                "UNINSTALLED",
                "unknown (42)",
                "unknown (1.5)",
                "unknown (-1)",
            ]
        );
    }

    #[test]
    fn active_extension_is_ready() {
        let facts = healthy();
        assert_eq!(next_step(&facts), NextStep::Ready);
        assert_eq!(
            doctor_lines(&facts),
            [
                "  [ok] GNOME Shell 51.1 reachable over D-Bus",
                "  [ok] Satur8 extension installed (per-user): \
                 /home/u/.local/share/gnome-shell/extensions/satur8@satur8.github.io",
                "       metadata lists GNOME Shell 45, 46, 47, 48, 49, 50, 51; includes 51",
                "  [ok] extension state in GNOME Shell: ACTIVE",
                "  [ok] org.satur8.GnomeShell is on the session bus: Satur8 can apply",
                "       Next step: nothing to do; `satur8 on` works.",
            ]
        );
        assert_eq!(
            status_line(&facts),
            "backend gnome-shell: available, extension active (GNOME Shell 51.1), \
             saturation 1.00"
        );
    }

    #[test]
    fn not_installed_says_to_run_the_installer() {
        let facts = GnomeFacts {
            install: Install::NotInstalled,
            view: ShellView::NotLoaded,
            service_owned: Some(false),
            saturation: None,
            ..healthy()
        };
        assert_eq!(next_step(&facts), NextStep::NotInstalled);
        let lines = doctor_lines(&facts);
        assert_eq!(
            lines[1],
            "  [!!] Satur8 extension not installed (satur8@satur8.github.io)"
        );
        assert_eq!(
            lines[2],
            "  [..] extension state in GNOME Shell: not loaded"
        );
        assert!(lines[4].starts_with("       Next step: run Satur8's installer"));
        assert_eq!(
            status_line(&facts),
            "backend gnome-shell: unavailable, extension not installed (see `satur8 doctor`)"
        );
    }

    #[test]
    fn installed_but_not_loaded_says_to_log_out() {
        let facts = GnomeFacts {
            view: ShellView::NotLoaded,
            service_owned: Some(false),
            ..healthy()
        };
        assert_eq!(next_step(&facts), NextStep::LogOutAndIn);
        assert!(doctor_lines(&facts)
            .last()
            .is_some_and(|line| line.starts_with("       Next step: log out and back in;")));
        // UNINSTALLED is the same as not loaded.
        let facts = GnomeFacts {
            view: loaded(ExtState::Uninstalled),
            ..facts
        };
        assert_eq!(next_step(&facts), NextStep::LogOutAndIn);
    }

    #[test]
    fn known_but_disabled_says_to_enable() {
        for state in [ExtState::Inactive, ExtState::Initialized] {
            let facts = GnomeFacts {
                view: loaded(state),
                service_owned: Some(false),
                ..healthy()
            };
            assert_eq!(next_step(&facts), NextStep::Enable);
            assert_eq!(
                advice(&next_step(&facts)),
                "run `gnome-extensions enable satur8@satur8.github.io`"
            );
            assert_eq!(
                status_line(&facts),
                "backend gnome-shell: unavailable, extension disabled (see `satur8 doctor`)"
            );
        }
    }

    #[test]
    fn disabled_with_all_extensions_off_says_to_turn_them_on() {
        let facts = GnomeFacts {
            user_extensions_enabled: Some(false),
            view: loaded(ExtState::Initialized),
            service_owned: Some(false),
            ..healthy()
        };
        assert_eq!(next_step(&facts), NextStep::UserExtensionsOff);
        assert!(advice(&NextStep::UserExtensionsOff)
            .contains("gsettings set org.gnome.shell disable-user-extensions false"));
    }

    #[test]
    fn out_of_date_names_the_versions() {
        let facts = GnomeFacts {
            install: up_to_50(),
            view: loaded(ExtState::OutOfDate),
            service_owned: Some(false),
            ..healthy()
        };
        let step = next_step(&facts);
        assert_eq!(
            step,
            NextStep::OutOfDate {
                listed: versions(&["45", "46", "47", "48", "49", "50"]),
                running: "51".to_string(),
            }
        );
        assert_eq!(
            advice(&step),
            "this Satur8's extension lists GNOME Shell 45, 46, 47, 48, 49, 50, not 51; \
             install a newer Satur8 release that supports GNOME 51"
        );
        let lines = doctor_lines(&facts);
        assert_eq!(
            lines[2],
            "  [!!] metadata lists GNOME Shell 45, 46, 47, 48, 49, 50; does not include 51"
        );
        assert_eq!(
            lines[3],
            "  [!!] extension state in GNOME Shell: OUT_OF_DATE"
        );
    }

    #[test]
    fn not_loaded_and_not_listed_is_out_of_date_not_a_logout() {
        let facts = GnomeFacts {
            install: up_to_50(),
            view: ShellView::NotLoaded,
            service_owned: Some(false),
            ..healthy()
        };
        assert!(matches!(next_step(&facts), NextStep::OutOfDate { .. }));
    }

    #[test]
    fn error_shows_the_shells_error_and_asks_for_a_report() {
        let facts = GnomeFacts {
            view: ShellView::Loaded {
                state: ExtState::Error,
                error: Some(
                    "Error: Could not find definition of virtual function \
                     get_static_shader_source"
                        .to_string(),
                ),
            },
            service_owned: Some(false),
            ..healthy()
        };
        assert_eq!(next_step(&facts), NextStep::LoadError);
        let lines = doctor_lines(&facts);
        assert_eq!(lines[3], "  [!!] extension state in GNOME Shell: ERROR");
        assert_eq!(
            lines[4],
            "       error: Error: Could not find definition of virtual function \
             get_static_shader_source"
        );
        assert!(lines[6].contains("please report it with this `satur8 doctor` output"));
    }

    #[test]
    fn active_without_its_bus_name_is_flagged() {
        let facts = GnomeFacts {
            service_owned: Some(false),
            saturation: None,
            ..healthy()
        };
        assert_eq!(next_step(&facts), NextStep::ServiceMissing);
        assert_eq!(
            doctor_lines(&facts)[4],
            "  [!!] org.satur8.GnomeShell is not on the session bus: Satur8 cannot apply"
        );
    }

    #[test]
    fn transitional_states_say_to_wait() {
        for state in [
            ExtState::Activating,
            ExtState::Deactivating,
            ExtState::Downloading,
        ] {
            let facts = GnomeFacts {
                view: loaded(state),
                ..healthy()
            };
            assert_eq!(next_step(&facts), NextStep::Busy);
        }
    }

    #[test]
    fn unreachable_shell_comes_first() {
        let facts = GnomeFacts {
            shell: Err("The name org.gnome.Shell was not provided".to_string()),
            user_extensions_enabled: None,
            view: ShellView::QueryFailed("The name org.gnome.Shell was not provided".into()),
            service_owned: Some(false),
            saturation: None,
            ..healthy()
        };
        assert_eq!(next_step(&facts), NextStep::ShellUnreachable);
        let lines = doctor_lines(&facts);
        assert_eq!(
            lines[0],
            "  [!!] GNOME Shell not reachable over D-Bus: The name org.gnome.Shell was not provided"
        );
        // Without a running version the metadata line only lists.
        assert_eq!(
            lines[2],
            "       metadata lists GNOME Shell 45, 46, 47, 48, 49, 50, 51"
        );
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn query_failure_is_reported() {
        let facts = GnomeFacts {
            view: ShellView::QueryFailed("timed out".to_string()),
            ..healthy()
        };
        assert_eq!(
            next_step(&facts),
            NextStep::QueryFailed("timed out".to_string())
        );
        let facts = GnomeFacts {
            install: Install::NotInstalled,
            ..facts
        };
        assert_eq!(next_step(&facts), NextStep::NotInstalled);
    }

    #[test]
    fn damaged_metadata_says_to_reinstall() {
        for metadata in [
            Metadata::Missing,
            Metadata::Unreadable("Permission denied (os error 13)".to_string()),
            Metadata::NoShellVersion,
            Metadata::Invalid,
        ] {
            let facts = GnomeFacts {
                install: installed(metadata.clone()),
                view: ShellView::NotLoaded,
                service_owned: Some(false),
                ..healthy()
            };
            assert_eq!(next_step(&facts), NextStep::Reinstall, "{metadata:?}");
            assert!(doctor_lines(&facts)[2].starts_with("  [!!] "));
        }
    }

    #[test]
    fn nan_state_is_unknown() {
        assert!(matches!(
            ExtState::from_number(f64::NAN),
            ExtState::Other(_)
        ));
        assert!(matches!(ExtState::from_number(1e9), ExtState::Other(_)));
    }

    #[test]
    fn major_version_matches_gnome_shells_rule() {
        assert_eq!(major_version("51.1"), "51");
        assert_eq!(major_version("51.rc"), "51");
        assert_eq!(major_version("50"), "50");
        assert!(lists_version(&versions(&["50", "51"]), "51"));
        assert!(lists_version(&versions(&["51.1"]), "51"));
        assert!(!lists_version(&versions(&["45", "50"]), "51"));
        assert!(!lists_version(&[], "51"));
    }

    #[test]
    fn parses_the_shipped_metadata() {
        let text = r#"{
    "uuid": "satur8@satur8.github.io",
    "name": "Satur8",
    "description": "Applies after the game renders (never injected).",
    "shell-version": ["45", "46", "47", "48", "49", "50", "51"],
    "url": "https://github.com/NtrpyDev/satur8",
    "session-modes": ["user"]
}"#;
        assert_eq!(
            parse_shell_versions(text),
            Metadata::Versions(versions(&["45", "46", "47", "48", "49", "50", "51"]))
        );
    }

    #[test]
    fn parser_tolerates_layout_and_ignores_the_key_inside_strings() {
        let text = "{\"description\":\"see \\\"shell-version\\\": [\\\"1\\\"]\",\n\
                    \"shell-version\"\n:\n[ \"50\" ,\n\"51\"\t]}";
        assert_eq!(
            parse_shell_versions(text),
            Metadata::Versions(versions(&["50", "51"]))
        );
        assert_eq!(
            parse_shell_versions(r#"{"shell-version": []}"#),
            Metadata::Versions(Vec::new())
        );
        assert_eq!(
            parse_shell_versions(r#"{"shell-version": ["5\u0031"]}"#),
            Metadata::Versions(versions(&["51"]))
        );
    }

    #[test]
    fn parser_reports_missing_and_invalid_lists() {
        assert_eq!(parse_shell_versions(""), Metadata::NoShellVersion);
        assert_eq!(
            parse_shell_versions("this is not json at all"),
            Metadata::NoShellVersion
        );
        assert_eq!(
            parse_shell_versions(r#"{"uuid": "satur8@satur8.github.io"}"#),
            Metadata::NoShellVersion
        );
        assert_eq!(
            parse_shell_versions(r#"{"shell-version": "51"}"#),
            Metadata::Invalid
        );
        assert_eq!(
            parse_shell_versions(r#"{"shell-version": [51]}"#),
            Metadata::Invalid
        );
        assert_eq!(
            parse_shell_versions(r#"{"shell-version": ["50", "51""#),
            Metadata::Invalid
        );
        assert_eq!(
            parse_shell_versions(r#"{"shell-version": ["50"#),
            Metadata::Invalid
        );
        assert_eq!(
            parse_shell_versions(r#"{"uuid": "satur8"#),
            Metadata::Invalid
        );
    }

    #[test]
    fn extension_dirs_follow_glib_and_gnome_shell_order() {
        let uuid = "satur8@satur8.github.io";
        let paths = |dirs: Vec<(PathBuf, bool)>| -> Vec<(String, bool)> {
            dirs.into_iter()
                .map(|(dir, per_user)| (dir.display().to_string(), per_user))
                .collect()
        };
        assert_eq!(
            paths(extension_dirs(None, Some("/home/u"), None, uuid)),
            [
                (
                    "/home/u/.local/share/gnome-shell/extensions/satur8@satur8.github.io"
                        .to_string(),
                    true
                ),
                (
                    "/usr/local/share/gnome-shell/extensions/satur8@satur8.github.io".to_string(),
                    false
                ),
                (
                    "/usr/share/gnome-shell/extensions/satur8@satur8.github.io".to_string(),
                    false
                ),
            ]
        );
        assert_eq!(
            paths(extension_dirs(
                Some("/data/u"),
                Some("/home/u"),
                Some("/opt/share::/usr/share"),
                uuid
            )),
            [
                (
                    "/data/u/gnome-shell/extensions/satur8@satur8.github.io".to_string(),
                    true
                ),
                (
                    "/opt/share/gnome-shell/extensions/satur8@satur8.github.io".to_string(),
                    false
                ),
                (
                    "/usr/share/gnome-shell/extensions/satur8@satur8.github.io".to_string(),
                    false
                ),
            ]
        );
        // Empty variables count as unset, as in GLib.
        assert_eq!(
            extension_dirs(Some(""), Some("/home/u"), Some(""), uuid).len(),
            3
        );
        assert_eq!(
            extension_dirs(None, None, Some("/usr/share"), uuid).len(),
            1
        );
    }
}
