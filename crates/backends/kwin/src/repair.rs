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
}

impl RepairOutcome {
    /// One plain sentence for the CLI, the daemon log, and the GUI.
    pub fn describe(&self) -> String {
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
            } => format!(
                "rebuilt the KWin effect for KWin {kwin}; log out and back in once so KWin picks it up"
            ),
            RepairOutcome::PendingLogin {
                kwin,
                rebuilt: false,
            } => format!(
                "the KWin effect is already built for KWin {kwin}; log out and back in once so KWin picks it up"
            ),
        }
    }
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
    /// Whether the running KWin says it can load the effect.
    pub supported: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Nothing to do.
    Healthy,
    /// The per-user copy is redundant or stale next to a good system copy;
    /// delete it so it cannot shadow the system one.
    DropUserCopy,
    /// A matching effect is installed; only a new login will load it.
    WaitForLogin,
    /// Build the effect for the installed KWin into the per-user directory.
    Build,
}

/// Pure decision: given what is installed and what KWin says, what to do next.
fn decide(state: &EffectState) -> Step {
    let matches_headers =
        |abi: &Option<String>| abi.is_some() && state.headers.is_some() && *abi == state.headers;
    let system_ok = matches_headers(&state.system);
    let user_ok = matches_headers(&state.user) && state.user_source_current;

    if state.supported {
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
    Step::Build
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

fn plugin_abi(path: &Path) -> Option<String> {
    abi_in_plugin(&fs::read(path).ok()?)
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
    let user = user_plugin_path().and_then(|p| plugin_abi(&p));
    let stamp = user_stamp_path().and_then(|p| fs::read_to_string(p).ok());
    EffectState {
        headers: fs::read_to_string(KWIN_CONFIG_HEADER)
            .ok()
            .and_then(|text| abi_in_header(&text))
            .or_else(installed_kwin_version),
        system: system_plugin_path().and_then(|p| plugin_abi(&p)),
        user_source_current: user.is_some()
            && stamp.as_deref().map(str::trim) == Some(SOURCE_VERSION),
        user,
        supported,
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

/// Check the effect against the running KWin and rebuild it when KWin rejects
/// it. Safe to call on every session start: it does nothing when all is well.
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

    let mut state = inspect(supported(&kwin));
    let mut step = decide(&state);

    if step == Step::DropUserCopy {
        if let Some(path) = user_plugin_path() {
            fs::remove_file(&path)
                .map_err(|e| format!("could not remove the stale {}: {e}", path.display()))?;
        }
        if let Some(stamp) = user_stamp_path() {
            let _ = fs::remove_file(stamp);
        }
        state = inspect(supported(&kwin));
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
    }
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
    let dest = user_plugin_path().ok_or("HOME is not set")?;
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

    let dir = dest.parent().ok_or("bad plugin path")?;
    fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    // Copy next to the destination, then rename over it, so KWin never sees a
    // half-written plugin.
    let staged = dir.join(format!(".{PLUGIN_FILE}.new"));
    fs::copy(&built, &staged).map_err(|e| format!("could not write {}: {e}", staged.display()))?;
    fs::rename(&staged, &dest).map_err(|e| format!("could not install {}: {e}", dest.display()))?;

    if let Some(stamp) = user_stamp_path() {
        if let Some(parent) = stamp.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(stamp, SOURCE_VERSION);
    }
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
            headers: v("6.8.3"),
            system: v("6.8.1"),
            user: v("6.8.2"),
            user_source_current: true,
            supported: false,
        };
        assert_eq!(decide(&state), Step::Build);
    }

    #[test]
    fn builds_when_no_effect_is_installed_at_all() {
        let state = EffectState {
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
        };
        assert_eq!(decide(&state), Step::WaitForLogin);
    }

    #[test]
    fn a_per_user_effect_from_an_older_satur8_is_rebuilt() {
        let state = EffectState {
            headers: v("6.8.3"),
            system: v("6.8.2"),
            user: v("6.8.3"),
            user_source_current: false,
            supported: false,
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
            headers: None,
            system: v("6.8.2"),
            supported: false,
            ..Default::default()
        };
        assert_eq!(decide(&state), Step::Build);
    }
}
