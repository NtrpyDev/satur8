//! Self-repair for the KWin effect.
//!
//! KWin refuses to load an effect plugin unless it was built for exactly the
//! KWin version that is running. Every KWin update therefore strands the
//! installed `satur8.so`, and before this module the only cure was a manual
//! reinstall.
//!
//! The repair runs inside the user's session (the daemon's unit calls it at
//! start), so it never needs root. It asks the running KWin whether it accepts
//! the effect, and when it does not, rebuilds the effect from the installed
//! source against the installed KWin headers and places the result in the
//! per-user Qt plugin directory. KWin searches that directory ahead of the
//! system one when `QT_PLUGIN_PATH` names it, which the installer and the
//! packages arrange through an `environment.d` file.
//!
//! Asking KWin, instead of comparing version strings ourselves, is what keeps
//! this correct in the hours between a KWin package update and the next login:
//! the old compositor is still running and still accepts the old effect, so
//! nothing is touched until the new compositor actually starts.
//!
//! Before building, it looks in the set of prebuilt effects that came with
//! Satur8 (the installer copies it to `~/.local/share/satur8/kwin-effects`),
//! so a system without build tools, such as SteamOS, is repaired too. Nothing
//! here touches the network.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use satur8_core::{Desktop, Environment, SessionType};

use crate::KwinBackend;

/// The plugin's file name. KWin derives the effect id from it.
const PLUGIN_FILE: &str = "satur8.so";
/// Where KWin looks for binary effects below a Qt plugin root.
const PLUGIN_SUBDIR: &str = "kwin/effects/plugins";
/// The plugin interface id is this prefix followed by the KWin version.
const ABI_MARKER: &[u8] = b"org.kde.kwin.EffectPluginFactory";
const KWIN_CONFIG_HEADER: &str = "/usr/include/kwin/config-kwin.h";
/// Created by ostree at boot. On such systems (Bazzite, Fedora Atomic) `/usr`
/// is read-only, so build tools cannot simply be installed.
const OSTREE_BOOTED: &str = "/run/ostree-booted";
/// Version of the Satur8 effect source this binary ships with. A per-user
/// build made from an older source is treated as stale.
const SOURCE_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Below the user data dir: the installed set of prebuilt effects, one
/// `<family>-<kwin>/satur8.so` per entry (packaging/install.sh).
const STORE_SUBDIR: &str = "satur8/kwin-effects";
const OS_RELEASE_PATHS: [&str; 2] = ["/etc/os-release", "/usr/lib/os-release"];
/// Where `libQt6Core.so.6.x.y` is looked for, in order: Fedora, then Arch and
/// SteamOS, then Debian and Ubuntu. The first folder that has one decides.
const QT_LIB_DIRS: [&str; 3] = ["/usr/lib64", "/usr/lib", "/usr/lib/x86_64-linux-gnu"];
const QT_CORE_PREFIX: &str = "libQt6Core.so.6.";
/// Qt 6 defines one symbol version per minor release, such as `Qt_6.11`. A
/// plugin that requires one cannot be loaded by an older Qt.
const QT_MINOR_MARKER: &[u8] = b"Qt_6.";

/// What a repair run found or did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepairOutcome {
    /// Not a KDE Plasma Wayland session, so there is no KWin effect to repair.
    NotKde,
    /// The running KWin accepts the installed effect.
    Healthy,
    /// The effect was rebuilt and the running KWin now accepts it.
    Rebuilt { kwin: String },
    /// An effect matching the installed KWin is in place, but the running
    /// compositor will only pick it up at the next login.
    PendingLogin { kwin: String, rebuilt: bool },
    /// The effect `key` from the installed set was put in place. `ready` is
    /// whether the running KWin accepts it now, else it needs a new login;
    /// `other_system` is whether it was built for another system family.
    InstalledFromSet {
        kwin: String,
        key: String,
        other_system: bool,
        ready: bool,
    },
}

/// How to get KWin to load an effect it has not picked up yet. On SteamOS,
/// leaving Desktop Mode and coming back is a new login, so the wording matches
/// what packaging/install.sh prints there.
fn relogin_hint(family: &Family) -> &'static str {
    if *family == Family::SteamOs {
        "switch to Game Mode and back to Desktop Mode, or log out and back in, once, so KWin picks it up"
    } else {
        "log out and back in once so KWin picks it up"
    }
}

impl RepairOutcome {
    /// One plain sentence for the CLI, the daemon log, and the GUI.
    pub fn describe(&self) -> String {
        self.describe_for(&system_family())
    }

    /// [`RepairOutcome::describe`] for a system of the given family.
    pub fn describe_for(&self, family: &Family) -> String {
        let relogin = relogin_hint(family);
        match self {
            RepairOutcome::NotKde => {
                "not a KDE Plasma Wayland session; there is no KWin effect to repair".into()
            }
            RepairOutcome::Healthy => "the KWin effect matches the running KWin".into(),
            RepairOutcome::Rebuilt { kwin } => {
                format!("rebuilt the KWin effect for KWin {kwin}; it is ready to use")
            }
            RepairOutcome::PendingLogin {
                kwin,
                rebuilt: true,
            } => format!("rebuilt the KWin effect for KWin {kwin}; {relogin}"),
            RepairOutcome::PendingLogin {
                kwin,
                rebuilt: false,
            } => format!("the KWin effect is already built for KWin {kwin}; {relogin}"),
            RepairOutcome::InstalledFromSet {
                kwin,
                key,
                other_system,
                ready,
            } => {
                let built_for = if *other_system {
                    format!(
                        " (built for {}, another system with the same KWin)",
                        key_family(key)
                    )
                } else {
                    String::new()
                };
                let next = if *ready {
                    "it is ready to use"
                } else {
                    relogin
                };
                format!(
                    "installed the KWin effect for KWin {kwin} from the set that came with Satur8{built_for}; {next}"
                )
            }
        }
    }
}

/// What `satur8 repair` will do about an effect the running KWin rejects, as
/// a clause for `satur8 doctor` and error messages. It names the step
/// [`decide`] chose, so nothing promises a rebuild that cannot happen.
pub fn repair_advice(step: Step, family: &Family) -> String {
    match step {
        Step::InstallFromSet => "run `satur8 repair` to install the matching effect from the set \
                                 that came with Satur8"
            .into(),
        Step::Build => "run `satur8 repair` to rebuild it for the installed KWin".into(),
        Step::CannotRepair => "this Satur8 has no effect for the installed KWin and this system \
                               cannot build one; run `satur8 repair` to see what to do"
            .into(),
        Step::WaitForLogin => format!(
            "an effect for the installed KWin is already in place; {}",
            relogin_hint(family)
        ),
        Step::DropUserCopy => {
            "run `satur8 repair` to remove the stale per-user copy that hides the system one".into()
        }
        Step::Healthy => "run `satur8 repair`".into(),
    }
}

/// [`repair_advice`] for this machine as it is now, for a KWin that rejects
/// the effect.
pub fn advice_here() -> String {
    advice_for(&inspect(false))
}

/// [`repair_advice`] for `state` on this machine's system family.
pub fn advice_for(state: &EffectState) -> String {
    repair_advice(decide(state), &system_family())
}

/// The kind of system, as far as prebuilt effects go. Effects are built per
/// family, and the family is the first half of a set key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Family {
    /// SteamOS (`ID=steamos`).
    SteamOs,
    /// Fedora or a system built from it, such as Bazzite, with its release.
    Fedora(u32),
    /// Anything else. No effects are built for it, so only another family's
    /// effect can be used, through the guarded fallback.
    Other,
}

impl Family {
    /// The family half of a set key, `steamos` or `fedora45`; `None` for
    /// [`Family::Other`].
    pub fn key_prefix(&self) -> Option<String> {
        match self {
            Family::SteamOs => Some("steamos".into()),
            Family::Fedora(release) => Some(format!("fedora{release}")),
            Family::Other => None,
        }
    }
}

/// Read the family from os-release text: `ID=steamos` is SteamOS; `ID=fedora`,
/// or an `ID_LIKE` that lists fedora, with a numeric `VERSION_ID` is Fedora.
pub fn family_from_os_release(text: &str) -> Family {
    let field = |key: &str| -> Option<String> {
        text.lines().find_map(|line| {
            let value = line.trim().strip_prefix(key)?.strip_prefix('=')?;
            Some(
                value
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string(),
            )
        })
    };
    let id = field("ID").unwrap_or_default();
    if id == "steamos" {
        return Family::SteamOs;
    }
    let like_fedora =
        field("ID_LIKE").is_some_and(|like| like.split_whitespace().any(|entry| entry == "fedora"));
    if id == "fedora" || like_fedora {
        if let Some(release) = field("VERSION_ID").and_then(|v| v.parse::<u32>().ok()) {
            return Family::Fedora(release);
        }
    }
    Family::Other
}

/// Split a set key into family and KWin version: `fedora45-6.8.3` gives
/// `("fedora45", "6.8.3")`. `None` when the key is not of that form.
pub fn split_key(key: &str) -> Option<(&str, &str)> {
    let (family, kwin) = key.rsplit_once('-')?;
    let family_ok = family.starts_with(|c: char| c.is_ascii_lowercase())
        && family
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    let kwin_ok = kwin.contains('.')
        && kwin
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
    (family_ok && kwin_ok).then_some((family, kwin))
}

fn key_family(key: &str) -> &str {
    split_key(key).map_or(key, |(family, _)| family)
}

/// One effect in the installed set, as read from `<key>/satur8.so`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetEntry {
    /// The folder name, `<family>-<kwin>`, such as `steamos-6.7.3`.
    pub key: String,
    /// The KWin version in the plugin's embedded ID.
    pub abi: Option<String>,
    /// The highest Qt 6 minor version the plugin requires.
    pub qt_minor: Option<u32>,
}

/// The set entry chosen for this system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetPick {
    /// The entry's folder name.
    pub key: String,
    /// It was built for another family with the same KWin, and is used only
    /// because the installed Qt is at least as new as the one it needs.
    pub other_system: bool,
}

/// Choose the set entry for this system and KWin version: the one built for
/// this family; else another family's one whose Qt requirement is not above
/// `installed_qt`. Entries whose embedded ID disagrees with their folder name
/// are never chosen. Ties go to the first key in sorted order.
pub fn pick_from_set(
    entries: &[SetEntry],
    family: &Family,
    kwin: &str,
    installed_qt: Option<u32>,
) -> Option<SetPick> {
    let mut usable: Vec<&SetEntry> = entries
        .iter()
        .filter(|entry| {
            split_key(&entry.key).is_some_and(|(_, key_kwin)| key_kwin == kwin)
                && entry.abi.as_deref() == Some(kwin)
        })
        .collect();
    usable.sort_by(|a, b| a.key.cmp(&b.key));
    if let Some(own) = family.key_prefix() {
        if let Some(entry) = usable.iter().find(|entry| key_family(&entry.key) == own) {
            return Some(SetPick {
                key: entry.key.clone(),
                other_system: false,
            });
        }
    }
    // Another system's effect: Qt is the only requirement checked against
    // this host. The build gate in packaging/build-kwin-effect.sh (floors in
    // packaging/abi-floors.sh) guarantees every bundled effect needs no newer
    // GLIBC, GLIBCXX or CXXABI version than the tarball's own binaries do.
    let qt = installed_qt?;
    usable
        .iter()
        .find(|entry| entry.qt_minor.is_some_and(|needed| needed <= qt))
        .map(|entry| SetPick {
            key: entry.key.clone(),
            other_system: true,
        })
}

/// The facts a repair decision is made from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EffectState {
    /// KWin version the next login will run, which is the version a rebuild
    /// would target: from the installed development headers, or, where they
    /// are not installed (Bazzite ships none), from the installed `kwin_wayland`.
    pub headers: Option<String>,
    /// KWin version the system-path effect was built for, if one is installed.
    pub system: Option<String>,
    /// KWin version the per-user effect was built for, if one is installed.
    pub user: Option<String>,
    /// Whether the per-user effect was built from this Satur8 version's source.
    pub user_source_current: bool,
    /// Whether the running KWin says it can load the effect. KWin keeps
    /// answering yes for an effect it loaded earlier even after the file on
    /// disk was replaced, so this alone does not prove the file is right.
    pub supported: bool,
    /// Version of the running KWin, from its support information; `None` when
    /// it could not be read.
    pub running: Option<String>,
    /// The installed set's effect for the installed KWin, if it has one.
    pub set_match: Option<SetPick>,
    /// The installed set entry the per-user effect is an exact copy of.
    pub user_from_set: Option<String>,
    /// Whether the effect source, cmake, a C++ compiler and the KWin
    /// development headers are all present, so a local build can run.
    pub can_build: bool,
}

/// What a repair does next, as decided by [`decide`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Nothing to do.
    Healthy,
    /// The per-user copy is redundant or stale next to a good system copy;
    /// delete it so it cannot shadow the system one.
    DropUserCopy,
    /// A matching effect is installed; only a new login will load it.
    WaitForLogin,
    /// Copy the matching effect from the installed set into the per-user
    /// directory.
    InstallFromSet,
    /// Build the effect for the installed KWin into the per-user directory.
    Build,
    /// The set has no effect for this KWin and nothing here can build one.
    CannotRepair,
}

/// The KWin version of the per-user effect file when that file is for neither
/// the running KWin nor the installed one. KWin can still report the effect
/// as supported then, because it keeps the plugin it loaded earlier in memory;
/// the wrong file only fails at the next session start. `None` when the file
/// is right, absent, or the running version is unknown.
pub fn stale_user_file(state: &EffectState) -> Option<&str> {
    let user = state.user.as_deref()?;
    let running = state.running.as_deref()?;
    (user != running && state.headers.as_deref() != Some(user)).then_some(user)
}

/// Pure decision: given what is installed and what KWin says, what to do next.
/// When a new effect is needed: the set's effect built for this system, then
/// a local build, then the set's effect built for another system, else
/// nothing can be done here. A per-user file for another KWin
/// ([`stale_user_file`]) is handled like a rejected effect even while KWin
/// says it is supported.
pub fn decide(state: &EffectState) -> Step {
    let matches_headers =
        |abi: &Option<String>| abi.is_some() && state.headers.is_some() && *abi == state.headers;
    let system_ok = matches_headers(&state.system);
    let user_ok = matches_headers(&state.user) && state.user_source_current;

    if state.supported && stale_user_file(state).is_none() {
        // KWin is happy. The only thing worth fixing is an old per-user build
        // shadowing a newer system copy of the same KWin version, which is safe
        // to swap because the running compositor accepts either file.
        if state.user.is_some()
            && !state.user_source_current
            && system_ok
            && state.user == state.system
        {
            return Step::DropUserCopy;
        }
        return Step::Healthy;
    }

    if system_ok {
        // The system copy is right for the installed KWin. A per-user copy can
        // only be getting in its way.
        if state.user.is_some() {
            return Step::DropUserCopy;
        }
        return Step::WaitForLogin;
    }
    if user_ok {
        return Step::WaitForLogin;
    }
    match &state.set_match {
        Some(pick) if !pick.other_system => Step::InstallFromSet,
        _ if state.can_build => Step::Build,
        Some(_) => Step::InstallFromSet,
        None => Step::CannotRepair,
    }
}

/// The highest `Qt_6.x` symbol version a plugin requires, as the minor `x`.
/// A plain `Qt_6` names no minor version and is ignored.
pub fn qt_minor_in_plugin(bytes: &[u8]) -> Option<u32> {
    let mut highest: Option<u32> = None;
    let mut from = 0;
    while let Some(found) = find(&bytes[from..], QT_MINOR_MARKER) {
        let start = from + found + QT_MINOR_MARKER.len();
        let digits: String = bytes[start..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .map(|b| *b as char)
            .collect();
        if let Ok(minor) = digits.parse::<u32>() {
            highest = highest.max(Some(minor));
        }
        from = start;
    }
    highest
}

/// The Qt minor version in a Qt core library file name:
/// `libQt6Core.so.6.11.1` gives 11.
pub fn qt_minor_in_lib_name(name: &str) -> Option<u32> {
    name.strip_prefix(QT_CORE_PREFIX)?
        .split('.')
        .next()?
        .parse()
        .ok()
}

/// Extract the KWin version a built plugin targets from its bytes.
pub fn abi_in_plugin(bytes: &[u8]) -> Option<String> {
    let mut from = 0;
    while let Some(found) = find(&bytes[from..], ABI_MARKER) {
        let start = from + found + ABI_MARKER.len();
        let version: String = bytes[start..]
            .iter()
            .take_while(|b| b.is_ascii_digit() || **b == b'.')
            .map(|b| *b as char)
            .collect();
        let version = version.trim_end_matches('.').to_string();
        if version.contains('.') {
            return Some(version);
        }
        from = start;
    }
    None
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Extract the KWin plugin version from `config-kwin.h`.
pub fn abi_in_header(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let rest = line
            .trim()
            .strip_prefix("#define KWIN_PLUGIN_VERSION_STRING")?;
        let version = rest.trim().trim_matches('"');
        (!version.is_empty()).then(|| version.to_string())
    })
}

/// Extract the KWin version from `kwin_wayland --version` output, `kwin 6.8.3`.
pub fn abi_in_version_output(text: &str) -> Option<String> {
    let version = text.lines().next()?.split_whitespace().last()?;
    let numeric = version.chars().all(|c| c.is_ascii_digit() || c == '.');
    let dotted = version.contains('.') && !version.starts_with('.') && !version.ends_with('.');
    (numeric && dotted).then(|| version.to_string())
}

/// Extract the running KWin's version from its support information
/// (`org.kde.KWin /KWin supportInformation`), the line `KWin version: 6.8.3`.
pub fn abi_in_support_information(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let version = line.trim().strip_prefix("KWin version:")?.trim();
        (!version.is_empty()).then(|| version.to_string())
    })
}

/// The version of the installed KWin binary, which the next login runs.
fn installed_kwin_version() -> Option<String> {
    let out = Command::new("kwin_wayland")
        .arg("--version")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    abi_in_version_output(&String::from_utf8_lossy(&out.stdout))
}

/// What to tell the user when the effect cannot be rebuilt here.
fn install_advice() -> &'static str {
    install_advice_for(Path::new(OSTREE_BOOTED).exists())
}

fn install_advice_for(ostree: bool) -> &'static str {
    if ostree {
        "/usr is read-only on this system (an image-based system such as Bazzite), so those \
         tools could only be added by layering packages with rpm-ostree. Reinstall Satur8 from \
         a release whose bundled effect matches this KWin instead"
    } else {
        "Install cmake, a C++ compiler, and the Qt 6 and KWin development files, then run \
         `satur8 repair`"
    }
}

/// The error when the set has no effect for `kwin` and nothing here can build
/// one (`blocker` says what is missing). An image-based system (SteamOS, or
/// `ostree`) is never told to install build tools.
fn cannot_repair_message(kwin: &str, family: &Family, ostree: bool, blocker: &str) -> String {
    let base = format!(
        "this Satur8 has no KWin effect for KWin {kwin}, and this system cannot build one ({blocker})."
    );
    let image_based = ostree || *family == Family::SteamOs;
    let published = family.key_prefix().is_some();
    match (published, image_based) {
        (true, true) => {
            format!("{base} Install a newer Satur8 release that includes an effect for KWin {kwin}")
        }
        (true, false) => format!(
            "{base} Install a newer Satur8 release that includes an effect for KWin {kwin}, or \
             install cmake, a C++ compiler, and the Qt 6 and KWin development files and run \
             `satur8 repair`"
        ),
        (false, true) => format!(
            "{base} No prebuilt effect is published for this system, and /usr is read-only \
             here, so the build tools could only be added by layering packages with rpm-ostree"
        ),
        (false, false) => format!(
            "{base} No prebuilt effect is published for this system. Install cmake, a C++ \
             compiler, and the Qt 6 and KWin development files, then run `satur8 repair`"
        ),
    }
}

fn plugin_abi(path: &Path) -> Option<String> {
    abi_in_plugin(&fs::read(path).ok()?)
}

/// This system's family, from os-release. `SATUR8_OS_RELEASE` names another
/// file, for tests.
fn system_family() -> Family {
    let text = match std::env::var_os("SATUR8_OS_RELEASE").filter(|v| !v.is_empty()) {
        Some(path) => fs::read_to_string(path).ok(),
        None => OS_RELEASE_PATHS
            .iter()
            .find_map(|path| fs::read_to_string(path).ok()),
    };
    text.map_or(Family::Other, |text| family_from_os_release(&text))
}

/// The installed Qt 6 minor version, from the name of the Qt core library
/// file. `SATUR8_QT_LIB_DIR` replaces the folders searched, for tests.
fn installed_qt_minor() -> Option<u32> {
    let dirs: Vec<PathBuf> = match std::env::var_os("SATUR8_QT_LIB_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => vec![PathBuf::from(dir)],
        None => QT_LIB_DIRS.iter().map(|dir| PathBuf::from(*dir)).collect(),
    };
    dirs.iter().find_map(|dir| {
        fs::read_dir(dir)
            .ok()?
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| qt_minor_in_lib_name(&entry.file_name().to_string_lossy()))
            .max()
    })
}

/// Where the installer put the set of prebuilt effects.
/// `SATUR8_EFFECT_STORE` names another folder, for tests.
pub fn store_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("SATUR8_EFFECT_STORE").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    xdg_dir("XDG_DATA_HOME", ".local/share").map(|data| data.join(STORE_SUBDIR))
}

/// The entries of the set in `dir`. Folders that are not named like a key,
/// or whose plugin cannot be read, are skipped.
fn read_store(dir: &Path) -> Vec<SetEntry> {
    let Ok(listing) = fs::read_dir(dir) else {
        return Vec::new();
    };
    listing
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let key = entry.file_name().to_str()?.to_string();
            split_key(&key)?;
            let bytes = fs::read(entry.path().join(PLUGIN_FILE)).ok()?;
            Some(SetEntry {
                abi: abi_in_plugin(&bytes),
                qt_minor: qt_minor_in_plugin(&bytes),
                key,
            })
        })
        .collect()
}

/// Why the effect cannot be built here, or `None` when everything a build
/// needs is present.
fn build_blocker() -> Option<String> {
    if source_dir().is_none() {
        return Some("the effect source is not installed".into());
    }
    for tool in ["cmake", "c++"] {
        if !on_path(tool) {
            return Some(format!("`{tool}` is not installed"));
        }
    }
    if !Path::new(KWIN_CONFIG_HEADER).is_file() {
        return Some(format!(
            "the KWin development headers ({KWIN_CONFIG_HEADER}) are not installed"
        ));
    }
    None
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

fn xdg_dir(var: &str, fallback: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(fallback)))
}

/// The per-user Qt plugin root that the installer puts on `QT_PLUGIN_PATH`.
pub fn user_plugin_root() -> Option<PathBuf> {
    home().map(|h| h.join(".local/lib/qt6/plugins"))
}

fn user_plugin_path() -> Option<PathBuf> {
    user_plugin_root().map(|root| root.join(PLUGIN_SUBDIR).join(PLUGIN_FILE))
}

/// Records which Satur8 version's source the per-user plugin was built from.
fn user_stamp_path() -> Option<PathBuf> {
    xdg_dir("XDG_STATE_HOME", ".local/state").map(|d| d.join("satur8/kwin-effect-source-version"))
}

fn system_plugin_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for (tool, args) in [
        ("qtpaths6", ["--query", "QT_INSTALL_PLUGINS"]),
        ("qmake6", ["-query", "QT_INSTALL_PLUGINS"]),
    ] {
        if let Ok(out) = Command::new(tool).args(args).output() {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if out.status.success() && !path.is_empty() {
                roots.push(PathBuf::from(path));
                break;
            }
        }
    }
    for fallback in ["/usr/lib/qt6/plugins", "/usr/lib64/qt6/plugins"] {
        let fallback = PathBuf::from(fallback);
        if !roots.contains(&fallback) {
            roots.push(fallback);
        }
    }
    roots
}

fn system_plugin_path() -> Option<PathBuf> {
    system_plugin_roots()
        .into_iter()
        .map(|root| root.join(PLUGIN_SUBDIR).join(PLUGIN_FILE))
        .find(|path| path.is_file())
}

/// Where the effect source is installed, newest install method first.
fn source_dir() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(dir) = std::env::var_os("SATUR8_EFFECT_SOURCE").filter(|v| !v.is_empty()) {
        candidates.push(PathBuf::from(dir));
    }
    if let Some(data) = xdg_dir("XDG_DATA_HOME", ".local/share") {
        candidates.push(data.join("satur8/kwin-effect"));
    }
    candidates.push(PathBuf::from("/usr/share/satur8/kwin-effect"));
    candidates.push(PathBuf::from("/usr/local/share/satur8/kwin-effect"));
    candidates
        .into_iter()
        .find(|dir| dir.join("CMakeLists.txt").is_file())
}

/// Read everything a repair decision depends on. `supported` is filled in by
/// the caller, since it needs the live KWin connection.
pub fn inspect(supported: bool) -> EffectState {
    let user_bytes = user_plugin_path().and_then(|p| fs::read(p).ok());
    let user = user_bytes.as_deref().and_then(abi_in_plugin);
    let stamp = user_stamp_path().and_then(|p| fs::read_to_string(p).ok());
    let headers = fs::read_to_string(KWIN_CONFIG_HEADER)
        .ok()
        .and_then(|text| abi_in_header(&text))
        .or_else(installed_kwin_version);
    let store = store_dir();
    let entries = store.as_deref().map(read_store).unwrap_or_default();
    let set_match = headers
        .as_deref()
        .and_then(|kwin| pick_from_set(&entries, &system_family(), kwin, installed_qt_minor()));
    // Same bytes as a set entry: the installer or a repair put it there.
    let user_from_set = match (&user_bytes, &store) {
        (Some(bytes), Some(dir)) => entries
            .iter()
            .find(|entry| {
                fs::read(dir.join(&entry.key).join(PLUGIN_FILE))
                    .ok()
                    .as_deref()
                    == Some(bytes.as_slice())
            })
            .map(|entry| entry.key.clone()),
        _ => None,
    };
    EffectState {
        headers,
        system: system_plugin_path().and_then(|p| plugin_abi(&p)),
        user_source_current: user.is_some()
            && stamp.as_deref().map(str::trim) == Some(SOURCE_VERSION),
        user,
        supported,
        // Needs the live KWin connection; the callers that have one fill it in.
        running: None,
        set_match,
        user_from_set,
        can_build: build_blocker().is_none(),
    }
}

/// At session start the daemon can come up a moment before KWin owns its bus
/// name, so give it a short grace period instead of reporting a false failure.
fn connect_with_grace(wait: Duration) -> Option<KwinBackend> {
    let deadline = Instant::now() + wait;
    loop {
        if let Some(backend) = KwinBackend::connect() {
            return Some(backend);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Check the effect against the running KWin and, when KWin rejects it, put a
/// matching one in place: from the installed set, or by building it. Safe to
/// call on every session start: it does nothing when all is well. It never
/// touches the network.
pub fn repair() -> Result<RepairOutcome, String> {
    let environment = Environment::detect();
    if environment.session != SessionType::Wayland || environment.desktop != Desktop::Kde {
        return Ok(RepairOutcome::NotKde);
    }
    let Some(kwin) = connect_with_grace(Duration::from_secs(15)) else {
        return Err(
            "KWin did not answer on the session bus, so the effect could not be checked".into(),
        );
    };
    let supported = |kwin: &KwinBackend| kwin.is_supported().unwrap_or(false);
    // KWin's own answer is not enough: it keeps an effect it loaded earlier,
    // so the file on disk is also checked against the running version.
    let running = kwin.running_version();
    let look = |kwin: &KwinBackend| EffectState {
        running: running.clone(),
        ..inspect(supported(kwin))
    };

    let mut state = look(&kwin);
    let mut step = decide(&state);

    if step == Step::DropUserCopy {
        if let Some(path) = user_plugin_path() {
            fs::remove_file(&path)
                .map_err(|e| format!("could not remove the stale {}: {e}", path.display()))?;
        }
        if let Some(stamp) = user_stamp_path() {
            let _ = fs::remove_file(stamp);
        }
        state = look(&kwin);
        step = decide(&state);
    }

    let headers = || {
        state.headers.clone().ok_or_else(|| {
            format!(
                "the KWin effect does not match the running KWin and cannot be rebuilt: \
                 neither the KWin development headers ({KWIN_CONFIG_HEADER}) nor \
                 kwin_wayland report the installed KWin version"
            )
        })
    };

    match step {
        Step::Healthy => Ok(RepairOutcome::Healthy),
        // A second DropUserCopy cannot happen: the copy is gone.
        Step::DropUserCopy | Step::WaitForLogin => Ok(RepairOutcome::PendingLogin {
            kwin: headers()?,
            rebuilt: false,
        }),
        Step::InstallFromSet => {
            let target = headers()?;
            let pick = state
                .set_match
                .clone()
                .ok_or("the effect set changed during the repair; run `satur8 repair` again")?;
            install_from_store(&pick.key, &target)?;
            Ok(RepairOutcome::InstalledFromSet {
                kwin: target,
                key: pick.key,
                other_system: pick.other_system,
                ready: supported(&kwin),
            })
        }
        Step::Build => {
            let target = headers()?;
            build_into_user_dir(&target)?;
            if supported(&kwin) {
                Ok(RepairOutcome::Rebuilt { kwin: target })
            } else {
                Ok(RepairOutcome::PendingLogin {
                    kwin: target,
                    rebuilt: true,
                })
            }
        }
        Step::CannotRepair => {
            let target = headers()?;
            let blocker = build_blocker().unwrap_or_else(|| "the build tools are missing".into());
            Err(cannot_repair_message(
                &target,
                &system_family(),
                Path::new(OSTREE_BOOTED).exists(),
                &blocker,
            ))
        }
    }
}

/// Put the set entry `key` in the per-user plugin directory, after checking
/// once more that it embeds KWin `target`: it is about to run inside the
/// compositor.
fn install_from_store(key: &str, target: &str) -> Result<(), String> {
    let store = store_dir().ok_or("HOME is not set")?;
    let source = store.join(key).join(PLUGIN_FILE);
    let abi = plugin_abi(&source);
    if abi.as_deref() != Some(target) {
        return Err(format!(
            "the KWin effect {} targets {} but KWin {target} is installed",
            source.display(),
            abi.as_deref().unwrap_or("an unknown version")
        ));
    }
    place_user_plugin(&source)
}

/// Copy `plugin` over the per-user effect without KWin ever seeing a
/// half-written file, and record that it matches this Satur8's source.
fn place_user_plugin(plugin: &Path) -> Result<(), String> {
    let dest = user_plugin_path().ok_or("HOME is not set")?;
    let dir = dest.parent().ok_or("bad plugin path")?;
    fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    // Copy next to the destination, then rename over it. Never write into the
    // destination in place: the running KWin may have that very file loaded,
    // and on a Steam Deck overwriting a loaded effect crashed KWin (a null
    // call inside EffectLoader::isEffectSupported). The rename gives the path
    // a new file while KWin keeps the old one it has open. A leftover staged
    // file is removed first so even it is a fresh file.
    let staged = dir.join(format!(".{PLUGIN_FILE}.new"));
    let _ = fs::remove_file(&staged);
    fs::copy(plugin, &staged).map_err(|e| format!("could not write {}: {e}", staged.display()))?;
    fs::rename(&staged, &dest).map_err(|e| format!("could not install {}: {e}", dest.display()))?;

    if let Some(stamp) = user_stamp_path() {
        if let Some(parent) = stamp.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(stamp, SOURCE_VERSION);
    }
    Ok(())
}

fn build_into_user_dir(target: &str) -> Result<(), String> {
    let source = source_dir().ok_or_else(|| {
        "the KWin effect does not match the running KWin and its source is not installed, \
         so it cannot be rebuilt; reinstall Satur8"
            .to_string()
    })?;
    for tool in ["cmake", "c++"] {
        if !on_path(tool) {
            return Err(format!(
                "the KWin effect needs rebuilding for KWin {target}, but `{tool}` is not installed. {}",
                install_advice()
            ));
        }
    }
    if !Path::new(KWIN_CONFIG_HEADER).is_file() {
        return Err(format!(
            "the KWin effect needs rebuilding for KWin {target}, but the KWin development \
             headers ({KWIN_CONFIG_HEADER}) are not installed. {}",
            install_advice()
        ));
    }
    let build = xdg_dir("XDG_CACHE_HOME", ".cache")
        .ok_or("HOME is not set")?
        .join("satur8/kwin-effect-build");
    // Start from an empty build tree: a cached configure from the previous
    // KWin version would otherwise carry its old version defines forward.
    let _ = fs::remove_dir_all(&build);
    fs::create_dir_all(&build).map_err(|e| format!("could not create {}: {e}", build.display()))?;

    run_cmake(&[
        "-S".as_ref(),
        source.as_os_str(),
        "-B".as_ref(),
        build.as_os_str(),
        "-DCMAKE_BUILD_TYPE=Release".as_ref(),
    ])?;
    run_cmake(&["--build".as_ref(), build.as_os_str()])?;

    let built = build.join(PLUGIN_FILE);
    let built_abi = plugin_abi(&built);
    if built_abi.as_deref() != Some(target) {
        return Err(format!(
            "the rebuilt KWin effect targets {} but KWin {target} is installed",
            built_abi.as_deref().unwrap_or("an unknown version")
        ));
    }

    place_user_plugin(&built)?;
    let _ = fs::remove_dir_all(&build);
    Ok(())
}

fn on_path(tool: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(tool).is_file()))
        .unwrap_or(false)
}

fn run_cmake(args: &[&std::ffi::OsStr]) -> Result<(), String> {
    let out = Command::new("cmake")
        .args(args)
        .output()
        .map_err(|e| format!("could not run cmake: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let tail: Vec<&str> = stderr.lines().rev().take(12).collect();
    Err(format!(
        "rebuilding the KWin effect failed:\n{}",
        tail.into_iter().rev().collect::<Vec<_>>().join("\n")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Option<String> {
        Some(s.to_string())
    }

    #[test]
    fn reads_the_abi_out_of_plugin_bytes() {
        let bytes = b"\0\0x%org.kde.kwin.EffectPluginFactory6.8.3\0more";
        assert_eq!(abi_in_plugin(bytes), v("6.8.3"));
    }

    #[test]
    fn skips_a_bare_marker_and_finds_the_versioned_one() {
        let bytes = b"org.kde.kwin.EffectPluginFactory\0org.kde.kwin.EffectPluginFactory6.7.4\0";
        assert_eq!(abi_in_plugin(bytes), v("6.7.4"));
    }

    #[test]
    fn plugin_without_a_marker_has_no_abi() {
        assert_eq!(abi_in_plugin(b"not a kwin plugin"), None);
    }

    #[test]
    fn reads_the_abi_out_of_the_kwin_header() {
        let header = "#pragma once\n#define KWIN_PLUGIN_VERSION_STRING \"6.8.3\"\n#define PROJECT_VERSION_MAJOR 6\n";
        assert_eq!(abi_in_header(header), v("6.8.3"));
        assert_eq!(abi_in_header("#define SOMETHING_ELSE 1\n"), None);
    }

    #[test]
    fn reads_the_version_out_of_kwin_wayland_version_output() {
        assert_eq!(abi_in_version_output("kwin 6.8.3\n"), v("6.8.3"));
        assert_eq!(abi_in_version_output("kwin_wayland 6.7.0"), v("6.7.0"));
        assert_eq!(
            abi_in_version_output("kwin 6.8.3\nsecond line 9.9\n"),
            v("6.8.3")
        );
        assert_eq!(abi_in_version_output("kwin\n"), None);
        assert_eq!(abi_in_version_output("kwin 6.8.\n"), None);
        assert_eq!(abi_in_version_output("kwin 6.8.3-beta\n"), None);
        assert_eq!(abi_in_version_output(""), None);
    }

    #[test]
    fn advice_on_an_image_based_system_does_not_say_to_install_build_tools() {
        assert!(install_advice_for(true).contains("rpm-ostree"));
        assert!(!install_advice_for(true).contains("Install cmake"));
        assert!(install_advice_for(false).contains("Install cmake"));
        assert!(install_advice_for(false).contains("satur8 repair"));
    }

    #[test]
    fn healthy_when_kwin_accepts_the_effect() {
        let state = EffectState {
            headers: v("6.8.3"),
            system: v("6.8.3"),
            supported: true,
            ..Default::default()
        };
        assert_eq!(decide(&state), Step::Healthy);
    }

    #[test]
    fn pending_kwin_update_leaves_a_working_effect_alone() {
        // KWin was updated on disk (headers 6.8.3) but the old 6.8.2 compositor
        // is still running and still accepts the 6.8.2 effect.
        let state = EffectState {
            headers: v("6.8.3"),
            system: v("6.8.2"),
            supported: true,
            ..Default::default()
        };
        assert_eq!(decide(&state), Step::Healthy);
    }

    #[test]
    fn builds_after_a_kwin_update_strands_the_system_effect() {
        let state = EffectState {
            can_build: true,
            headers: v("6.8.3"),
            system: v("6.8.2"),
            supported: false,
            ..Default::default()
        };
        assert_eq!(decide(&state), Step::Build);
    }

    #[test]
    fn rebuilds_a_per_user_effect_left_over_from_the_previous_kwin() {
        let state = EffectState {
            can_build: true,
            headers: v("6.8.3"),
            system: v("6.8.1"),
            user: v("6.8.2"),
            user_source_current: true,
            supported: false,
            ..Default::default()
        };
        assert_eq!(decide(&state), Step::Build);
    }

    #[test]
    fn builds_when_no_effect_is_installed_at_all() {
        let state = EffectState {
            can_build: true,
            headers: v("6.8.3"),
            ..Default::default()
        };
        assert_eq!(decide(&state), Step::Build);
    }

    #[test]
    fn a_matching_per_user_effect_kwin_cannot_see_yet_waits_for_login() {
        let state = EffectState {
            headers: v("6.8.3"),
            system: v("6.8.2"),
            user: v("6.8.3"),
            user_source_current: true,
            supported: false,
            ..Default::default()
        };
        assert_eq!(decide(&state), Step::WaitForLogin);
    }

    #[test]
    fn a_per_user_effect_from_an_older_satur8_is_rebuilt() {
        let state = EffectState {
            can_build: true,
            headers: v("6.8.3"),
            system: v("6.8.2"),
            user: v("6.8.3"),
            user_source_current: false,
            supported: false,
            ..Default::default()
        };
        assert_eq!(decide(&state), Step::Build);
    }

    #[test]
    fn stale_per_user_effect_shadowing_a_good_system_one_is_dropped() {
        let state = EffectState {
            headers: v("6.8.3"),
            system: v("6.8.3"),
            user: v("6.8.2"),
            user_source_current: true,
            supported: false,
            ..Default::default()
        };
        assert_eq!(decide(&state), Step::DropUserCopy);
    }

    #[test]
    fn old_source_per_user_effect_yields_to_a_same_version_system_one() {
        let state = EffectState {
            headers: v("6.8.3"),
            system: v("6.8.3"),
            user: v("6.8.3"),
            user_source_current: false,
            supported: true,
            ..Default::default()
        };
        assert_eq!(decide(&state), Step::DropUserCopy);
    }

    #[test]
    fn working_per_user_effect_is_kept_while_a_kwin_update_is_pending() {
        // Running 6.8.2 with a per-user 6.8.2 effect; the packages already
        // moved the system copy and headers to 6.8.3. Dropping the per-user
        // copy now would break the running session.
        let state = EffectState {
            headers: v("6.8.3"),
            system: v("6.8.3"),
            user: v("6.8.2"),
            user_source_current: false,
            supported: true,
            ..Default::default()
        };
        assert_eq!(decide(&state), Step::Healthy);
    }

    #[test]
    fn good_system_effect_kwin_rejects_waits_for_login() {
        let state = EffectState {
            headers: v("6.8.3"),
            system: v("6.8.3"),
            supported: false,
            ..Default::default()
        };
        assert_eq!(decide(&state), Step::WaitForLogin);
    }

    #[test]
    fn without_headers_nothing_counts_as_matching() {
        let state = EffectState {
            can_build: true,
            headers: None,
            system: v("6.8.2"),
            supported: false,
            ..Default::default()
        };
        assert_eq!(decide(&state), Step::Build);
    }

    // ------------------------------------------------ the set of prebuilt effects

    #[test]
    fn reads_the_family_out_of_os_release() {
        let steamos =
            "NAME=\"SteamOS\"\nID=steamos\nID_LIKE=arch\nVERSION_ID=3.9.5\nVARIANT_ID=steamdeck\n";
        assert_eq!(family_from_os_release(steamos), Family::SteamOs);
        let bazzite = "NAME=\"Bazzite\"\nID=bazzite\nID_LIKE=\"fedora\"\nVERSION_ID=45\n";
        assert_eq!(family_from_os_release(bazzite), Family::Fedora(45));
        let fedora = "NAME=\"Fedora Linux\"\nID=fedora\nVERSION_ID=44\n";
        assert_eq!(family_from_os_release(fedora), Family::Fedora(44));
        let like_many = "ID=nobara\nID_LIKE=\"rhel centos fedora\"\nVERSION_ID='43'\n";
        assert_eq!(family_from_os_release(like_many), Family::Fedora(43));
        assert_eq!(
            family_from_os_release("ID=arch\nBUILD_ID=rolling\n"),
            Family::Other
        );
        assert_eq!(
            family_from_os_release("ID=fedora\nVERSION_ID=rawhide\n"),
            Family::Other
        );
        assert_eq!(family_from_os_release(""), Family::Other);
        assert_eq!(Family::SteamOs.key_prefix().as_deref(), Some("steamos"));
        assert_eq!(Family::Fedora(45).key_prefix().as_deref(), Some("fedora45"));
        assert_eq!(Family::Other.key_prefix(), None);
    }

    #[test]
    fn reads_the_highest_qt_minor_out_of_plugin_bytes() {
        let bytes = b"\0Qt_6\0libQt6Core.so.6\0Qt_6.11\0Qt_6\0Qt_6.9\0";
        assert_eq!(qt_minor_in_plugin(bytes), Some(11));
        assert_eq!(qt_minor_in_plugin(b"\0Qt_6.12\0Qt_6.12\0"), Some(12));
        assert_eq!(qt_minor_in_plugin(b"\0Qt_6\0Qt_6\0"), None);
        assert_eq!(qt_minor_in_plugin(b"\0Qt_6.\0"), None);
        assert_eq!(qt_minor_in_plugin(b""), None);
    }

    #[test]
    fn reads_the_installed_qt_minor_out_of_the_library_file_name() {
        assert_eq!(qt_minor_in_lib_name("libQt6Core.so.6.11.1"), Some(11));
        assert_eq!(qt_minor_in_lib_name("libQt6Core.so.6.12.0"), Some(12));
        assert_eq!(qt_minor_in_lib_name("libQt6Core.so.6"), None);
        assert_eq!(qt_minor_in_lib_name("libQt6Core.so.6.x"), None);
        assert_eq!(qt_minor_in_lib_name("libQt6Gui.so.6.11.1"), None);
    }

    #[test]
    fn splits_set_keys() {
        assert_eq!(split_key("steamos-6.7.3"), Some(("steamos", "6.7.3")));
        assert_eq!(split_key("fedora45-6.8.3"), Some(("fedora45", "6.8.3")));
        assert_eq!(split_key("steamos-6"), None);
        assert_eq!(split_key("Steamos-6.7.3"), None);
        assert_eq!(split_key("45-6.7.3"), None);
        assert_eq!(split_key("steamos-6.7."), None);
        assert_eq!(split_key("steamos"), None);
    }

    fn entry(key: &str, abi: &str, qt: u32) -> SetEntry {
        SetEntry {
            key: key.into(),
            abi: v(abi),
            qt_minor: Some(qt),
        }
    }

    /// The set the v0.4.1 tarball is planned to carry, plus one entry whose
    /// folder name and embedded ID disagree.
    fn set() -> Vec<SetEntry> {
        vec![
            entry("steamos-6.7.3", "6.7.3", 11),
            entry("steamos-6.8.2", "6.8.2", 12),
            entry("fedora45-6.8.3", "6.8.3", 11),
            entry("steamos-6.8.4", "6.8.5", 12),
        ]
    }

    fn own(key: &str) -> Option<SetPick> {
        Some(SetPick {
            key: key.into(),
            other_system: false,
        })
    }

    fn other(key: &str) -> Option<SetPick> {
        Some(SetPick {
            key: key.into(),
            other_system: true,
        })
    }

    #[test]
    fn picks_the_effect_built_for_this_system() {
        let set = set();
        assert_eq!(
            pick_from_set(&set, &Family::SteamOs, "6.7.3", Some(11)),
            own("steamos-6.7.3")
        );
        // Bazzite reads as Fedora 45.
        assert_eq!(
            pick_from_set(&set, &Family::Fedora(45), "6.8.3", Some(11)),
            own("fedora45-6.8.3")
        );
        // The family's own effect needs no Qt check, even with Qt unknown.
        assert_eq!(
            pick_from_set(&set, &Family::SteamOs, "6.8.2", None),
            own("steamos-6.8.2")
        );
        assert_eq!(
            pick_from_set(&set, &Family::SteamOs, "6.9.0", Some(12)),
            None
        );
    }

    #[test]
    fn uses_another_systems_effect_only_with_a_new_enough_qt() {
        let set = set();
        assert_eq!(
            pick_from_set(&set, &Family::Other, "6.8.3", Some(12)),
            other("fedora45-6.8.3")
        );
        assert_eq!(
            pick_from_set(&set, &Family::Other, "6.8.3", Some(11)),
            other("fedora45-6.8.3")
        );
        assert_eq!(pick_from_set(&set, &Family::Other, "6.8.3", Some(10)), None);
        assert_eq!(pick_from_set(&set, &Family::Other, "6.8.3", None), None);
        // Fedora 44 with the same KWin is another family too.
        assert_eq!(
            pick_from_set(&set, &Family::Fedora(44), "6.8.3", Some(11)),
            other("fedora45-6.8.3")
        );
        let unknown_qt = vec![SetEntry {
            key: "fedora45-6.8.3".into(),
            abi: v("6.8.3"),
            qt_minor: None,
        }];
        assert_eq!(
            pick_from_set(&unknown_qt, &Family::Other, "6.8.3", Some(12)),
            None
        );
    }

    #[test]
    fn never_picks_an_entry_whose_id_disagrees_with_its_folder() {
        let set = set();
        assert_eq!(
            pick_from_set(&set, &Family::SteamOs, "6.8.4", Some(12)),
            None
        );
        assert_eq!(pick_from_set(&set, &Family::Other, "6.8.4", Some(12)), None);
        assert_eq!(
            pick_from_set(&set, &Family::SteamOs, "6.8.5", Some(12)),
            None
        );
        assert_eq!(pick_from_set(&set, &Family::Other, "6.8.5", Some(12)), None);
    }

    #[test]
    fn the_set_wins_over_a_build() {
        let state = EffectState {
            headers: v("6.7.3"),
            set_match: own("steamos-6.7.3"),
            can_build: true,
            ..Default::default()
        };
        assert_eq!(decide(&state), Step::InstallFromSet);
    }

    #[test]
    fn a_build_wins_over_another_systems_effect() {
        let state = EffectState {
            headers: v("6.8.3"),
            set_match: other("fedora45-6.8.3"),
            can_build: true,
            ..Default::default()
        };
        assert_eq!(decide(&state), Step::Build);
        let state = EffectState {
            can_build: false,
            ..state
        };
        assert_eq!(decide(&state), Step::InstallFromSet);
    }

    #[test]
    fn no_tools_and_no_match_cannot_be_repaired_here() {
        let state = EffectState {
            headers: v("6.9.0"),
            user: v("6.7.3"),
            user_source_current: true,
            ..Default::default()
        };
        assert_eq!(decide(&state), Step::CannotRepair);
    }

    #[test]
    fn a_stale_per_user_copy_is_replaced_from_the_set() {
        // The Deck after an OS update: per-user 6.7.3 from the set, KWin now 6.8.2.
        let state = EffectState {
            headers: v("6.8.2"),
            user: v("6.7.3"),
            user_source_current: true,
            user_from_set: Some("steamos-6.7.3".into()),
            set_match: own("steamos-6.8.2"),
            ..Default::default()
        };
        assert_eq!(decide(&state), Step::InstallFromSet);
    }

    #[test]
    fn the_set_does_not_change_the_pending_update_cases() {
        // As pending_kwin_update_leaves_a_working_effect_alone and
        // working_per_user_effect_is_kept_while_a_kwin_update_is_pending, with
        // a matching set entry and build tools present.
        let state = EffectState {
            headers: v("6.8.3"),
            system: v("6.8.2"),
            supported: true,
            set_match: own("fedora45-6.8.3"),
            can_build: true,
            ..Default::default()
        };
        assert_eq!(decide(&state), Step::Healthy);
        let state = EffectState {
            headers: v("6.8.3"),
            system: v("6.8.3"),
            user: v("6.8.2"),
            user_source_current: false,
            supported: true,
            set_match: own("fedora45-6.8.3"),
            can_build: true,
            ..Default::default()
        };
        assert_eq!(decide(&state), Step::Healthy);
    }

    #[test]
    fn the_cannot_repair_message_never_tells_an_image_based_system_to_install_cmake() {
        let blocker = "`cmake` is not installed";
        for (family, ostree) in [
            (Family::SteamOs, false),
            (Family::Fedora(45), true),
            (Family::Other, true),
        ] {
            let message = cannot_repair_message("6.9.0", &family, ostree, blocker);
            assert!(
                !message.to_lowercase().contains("install cmake"),
                "{family:?}: {message}"
            );
            assert!(message.contains("KWin 6.9.0"), "{message}");
        }
        let deck = cannot_repair_message("6.9.0", &Family::SteamOs, false, blocker);
        assert!(deck.contains("Install a newer Satur8 release"), "{deck}");
        let bazzite = cannot_repair_message("6.8.4", &Family::Fedora(45), true, blocker);
        assert!(
            bazzite.contains("Install a newer Satur8 release"),
            "{bazzite}"
        );
        let fedora = cannot_repair_message("6.8.4", &Family::Fedora(45), false, blocker);
        assert!(fedora.contains("newer Satur8 release") && fedora.contains("install cmake"));
        let arch = cannot_repair_message("6.8.4", &Family::Other, false, blocker);
        assert!(arch.contains("No prebuilt effect is published for this system"));
        assert!(arch.contains("Install cmake") && arch.contains("satur8 repair"));
        let kalpa = cannot_repair_message("6.8.4", &Family::Other, true, blocker);
        assert!(kalpa.contains("No prebuilt effect is published") && kalpa.contains("rpm-ostree"));
    }

    #[test]
    fn describes_an_effect_installed_from_the_set() {
        let ready = RepairOutcome::InstalledFromSet {
            kwin: "6.8.2".into(),
            key: "steamos-6.8.2".into(),
            other_system: false,
            ready: true,
        };
        assert_eq!(
            ready.describe_for(&Family::SteamOs),
            "installed the KWin effect for KWin 6.8.2 from the set that came with Satur8; it is ready to use"
        );
        let later = RepairOutcome::InstalledFromSet {
            kwin: "6.8.3".into(),
            key: "fedora45-6.8.3".into(),
            other_system: true,
            ready: false,
        };
        assert_eq!(
            later.describe_for(&Family::Other),
            "installed the KWin effect for KWin 6.8.3 from the set that came with Satur8 \
             (built for fedora45, another system with the same KWin); log out and back in \
             once so KWin picks it up"
        );
    }

    #[test]
    fn steamos_is_told_to_switch_modes_to_pick_up_an_effect() {
        let pending = RepairOutcome::PendingLogin {
            kwin: "6.7.3".into(),
            rebuilt: false,
        };
        assert_eq!(
            pending.describe_for(&Family::SteamOs),
            "the KWin effect is already built for KWin 6.7.3; switch to Game Mode and back to \
             Desktop Mode, or log out and back in, once, so KWin picks it up"
        );
        assert_eq!(
            pending.describe_for(&Family::Fedora(45)),
            "the KWin effect is already built for KWin 6.7.3; log out and back in once so KWin \
             picks it up"
        );
        let from_set = RepairOutcome::InstalledFromSet {
            kwin: "6.8.2".into(),
            key: "steamos-6.8.2".into(),
            other_system: false,
            ready: false,
        };
        assert!(from_set
            .describe_for(&Family::SteamOs)
            .ends_with("; switch to Game Mode and back to Desktop Mode, or log out and back in, once, so KWin picks it up"));
    }

    #[test]
    fn repair_advice_names_what_repair_will_actually_do() {
        let other = Family::Other;
        assert!(repair_advice(Step::Build, &other).contains("rebuild it"));
        let from_set = repair_advice(Step::InstallFromSet, &Family::SteamOs);
        assert!(from_set.contains("from the set that came with Satur8"));
        assert!(!from_set.contains("build"));
        let cannot = repair_advice(Step::CannotRepair, &Family::SteamOs);
        assert!(cannot.contains("cannot build one") && !cannot.contains("rebuild"));
        assert!(repair_advice(Step::WaitForLogin, &Family::SteamOs).contains("Game Mode"));
        assert!(repair_advice(Step::WaitForLogin, &other).contains("log out and back in once"));
        for step in [
            Step::Healthy,
            Step::DropUserCopy,
            Step::InstallFromSet,
            Step::Build,
            Step::CannotRepair,
        ] {
            assert!(
                repair_advice(step, &other).contains("satur8 repair"),
                "{step:?}"
            );
        }
    }

    // ---------------------- KWin says supported, but the file on disk is wrong

    #[test]
    fn reads_the_running_version_out_of_support_information() {
        let text = "KWin Support Information:\n\nVersion\n=======\nKWin version: 6.7.3\nQt Version: 6.11.1\n";
        assert_eq!(abi_in_support_information(text), v("6.7.3"));
        assert_eq!(abi_in_support_information("KWin version:\n"), None);
        assert_eq!(abi_in_support_information(""), None);
    }

    /// The Deck, measured: the right effect is loaded in the running KWin
    /// 6.7.3, then the per-user file is replaced by one for KWin 6.7.9. KWin
    /// keeps answering supported from memory.
    fn deck_with_a_wrong_file_on_disk() -> EffectState {
        EffectState {
            headers: v("6.7.3"),
            running: v("6.7.3"),
            user: v("6.7.9"),
            supported: true,
            ..Default::default()
        }
    }

    #[test]
    fn a_wrong_per_user_file_kwin_still_accepts_is_replaced_from_the_set() {
        let state = EffectState {
            set_match: own("steamos-6.7.3"),
            ..deck_with_a_wrong_file_on_disk()
        };
        assert_eq!(stale_user_file(&state), Some("6.7.9"));
        assert_eq!(decide(&state), Step::InstallFromSet);
    }

    #[test]
    fn a_wrong_per_user_file_follows_the_rejected_effect_order() {
        let build = EffectState {
            can_build: true,
            ..deck_with_a_wrong_file_on_disk()
        };
        assert_eq!(decide(&build), Step::Build);
        let system = EffectState {
            system: v("6.7.3"),
            ..deck_with_a_wrong_file_on_disk()
        };
        assert_eq!(decide(&system), Step::DropUserCopy);
        assert_eq!(
            decide(&deck_with_a_wrong_file_on_disk()),
            Step::CannotRepair
        );
    }

    #[test]
    fn a_matching_per_user_file_kwin_accepts_is_left_alone() {
        let state = EffectState {
            user: v("6.7.3"),
            user_source_current: true,
            set_match: own("steamos-6.7.3"),
            can_build: true,
            ..deck_with_a_wrong_file_on_disk()
        };
        assert_eq!(stale_user_file(&state), None);
        assert_eq!(decide(&state), Step::Healthy);
    }

    #[test]
    fn a_system_effect_with_no_per_user_file_kwin_accepts_is_left_alone() {
        let state = EffectState {
            headers: v("6.7.3"),
            running: v("6.7.3"),
            system: v("6.7.3"),
            supported: true,
            set_match: own("steamos-6.7.3"),
            can_build: true,
            ..Default::default()
        };
        assert_eq!(stale_user_file(&state), None);
        assert_eq!(decide(&state), Step::Healthy);
    }

    #[test]
    fn a_per_user_file_for_the_running_or_the_installed_kwin_is_not_stale() {
        // A KWin update is pending: running 6.8.2, installed 6.8.3.
        let for_running = EffectState {
            headers: v("6.8.3"),
            running: v("6.8.2"),
            user: v("6.8.2"),
            supported: true,
            set_match: own("fedora45-6.8.3"),
            ..Default::default()
        };
        assert_eq!(decide(&for_running), Step::Healthy);
        let for_installed = EffectState {
            user: v("6.8.3"),
            ..for_running.clone()
        };
        assert_eq!(stale_user_file(&for_installed), None);
        assert_eq!(decide(&for_installed), Step::Healthy);
        // Without the running version KWin's answer is all there is.
        let unknown = EffectState {
            running: None,
            ..deck_with_a_wrong_file_on_disk()
        };
        assert_eq!(stale_user_file(&unknown), None);
        assert_eq!(decide(&unknown), Step::Healthy);
    }
}
