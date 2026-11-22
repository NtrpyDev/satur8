//! `satur8-daemon` - event-driven saturation changes on window focus.
//!
//! The companion KWin script forwards activations over D-Bus. Other callers
//! can use the same interface, and backend selection is shared with every
//! Satur8 frontend. In Steam Game Mode a thread watches gamescope's focus
//! instead (`gamescope_focus.rs`).

mod gamescope_focus;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use satur8_backend::{apply_to_outputs, install_signal_handler, select_backend, RestoreGuard};
use satur8_core::{BackendError, Profile, Profiles, Saturation};
use zbus::interface;

const SERVICE: &str = "org.satur8.Daemon";
const PATH: &str = "/org/satur8/Daemon";
const INITIAL_RETRY: Duration = Duration::from_millis(250);
const MAX_RETRY: Duration = Duration::from_secs(8);

#[derive(Debug, PartialEq)]
enum FocusAction {
    Apply {
        profile: String,
        saturation: Saturation,
        outputs: Vec<String>,
    },
    Restore,
    None,
}

/// The focused window's profile, if any. The pid's real executable is tried
/// first because it is the only thing that identifies a Proton game: every
/// non-Steam title launched through umu shares the window class `steam_app_0`,
/// so class alone cannot tell Tarkov from anything else launched the same way.
/// The class remains the fallback for windows we cannot resolve a pid for.
fn match_focused<'a>(
    profiles: &'a Profiles,
    window_class: &str,
    exe: Option<&str>,
) -> Option<&'a Profile> {
    exe.and_then(|exe| profiles.match_exe(exe))
        .or_else(|| profiles.match_window_class(window_class))
}

fn decide_focus_action(
    current: Option<&str>,
    window_class: &str,
    exe: Option<&str>,
    profiles: &Profiles,
) -> FocusAction {
    match match_focused(profiles, window_class, exe) {
        Some(profile) if current == Some(profile.name.as_str()) => FocusAction::None,
        Some(profile) => FocusAction::Apply {
            profile: profile.name.clone(),
            saturation: profile.saturation(),
            outputs: profile.outputs.clone(),
        },
        None if current.is_some() => FocusAction::Restore,
        None => FocusAction::None,
    }
}

/// The outcome of trying the first sync for a gamescope focus source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GamescopeSync {
    /// The backend is gamescope's and the focus was synced.
    Done,
    /// No backend is selected yet; try again shortly.
    NotYet,
    /// A desktop backend is selected, so gamescope focus is not ours to follow.
    NotGamescope,
}

struct BackendState {
    guard: Option<RestoreGuard>,
    retry_delay: Duration,
    next_retry: Instant,
}

impl BackendState {
    fn detect(restore_to: Saturation) -> Self {
        let mut state = Self {
            guard: None,
            retry_delay: INITIAL_RETRY,
            next_retry: Instant::now(),
        };
        state.try_detect(restore_to);
        state
    }

    /// No backend yet, and no detection attempt for an hour, so a test decides
    /// when one appears.
    #[cfg(test)]
    fn pending() -> Self {
        Self {
            guard: None,
            retry_delay: INITIAL_RETRY,
            next_retry: Instant::now() + Duration::from_secs(3600),
        }
    }

    #[cfg(test)]
    fn with_backend(backend: Box<dyn satur8_core::Backend>, restore_to: Saturation) -> Self {
        Self {
            guard: Some(RestoreGuard::new(backend, restore_to)),
            retry_delay: INITIAL_RETRY,
            next_retry: Instant::now(),
        }
    }

    fn try_detect(&mut self, restore_to: Saturation) {
        if self.guard.is_some() || Instant::now() < self.next_retry {
            return;
        }
        match select_backend() {
            Ok(backend) => {
                eprintln!("satur8-daemon: {} backend active", backend.name());
                self.guard = Some(RestoreGuard::new(backend, restore_to));
                self.retry_delay = INITIAL_RETRY;
            }
            Err(error) => {
                eprintln!("satur8-daemon: backend unavailable; will retry on focus: {error}");
                self.next_retry = Instant::now() + self.retry_delay;
                self.retry_delay = (self.retry_delay * 2).min(MAX_RETRY);
            }
        }
    }

    fn apply(
        &mut self,
        saturation: Saturation,
        outputs: &[String],
        restore_to: Saturation,
    ) -> Result<(), BackendError> {
        self.try_detect(restore_to);
        let guard = self
            .guard
            .as_mut()
            .ok_or_else(|| BackendError::Unavailable("no backend is currently reachable".into()))?;
        guard.set_restore_to(restore_to);
        let result = apply_to_outputs(guard.backend_mut(), outputs, saturation);
        if result.is_ok() {
            guard.arm();
        }
        result
    }

    fn restore(&mut self, restore_to: Saturation) -> Result<(), BackendError> {
        self.try_detect(restore_to);
        let guard = self
            .guard
            .as_mut()
            .ok_or_else(|| BackendError::Unavailable("no backend is currently reachable".into()))?;
        guard.set_restore_to(restore_to);
        guard.restore_now()
    }
}

struct Daemon {
    profiles: Profiles,
    backend: Arc<Mutex<BackendState>>,
    current: Option<String>,
    /// What the focus script last reported: the resolved executable when the
    /// pid could be resolved, otherwise the window class. Read by the GUI.
    last_focused: String,
}

impl Daemon {
    fn new(profiles: Profiles, backend: Arc<Mutex<BackendState>>) -> Self {
        eprintln!(
            "satur8-daemon: ready, {} profile(s) loaded",
            profiles.profiles.len()
        );
        Self {
            profiles,
            backend,
            current: None,
            last_focused: String::new(),
        }
    }

    #[cfg(test)]
    fn with_profiles(profiles: Profiles, backend: Box<dyn satur8_core::Backend>) -> Self {
        let restore_to = profiles.default_saturation();
        Self {
            profiles,
            backend: Arc::new(Mutex::new(BackendState::with_backend(backend, restore_to))),
            current: None,
            last_focused: String::new(),
        }
    }

    fn react(&mut self, class: &str, pid: u32) {
        self.backend
            .lock()
            .expect("backend state mutex poisoned")
            .try_detect(self.profiles.default_saturation());
        let exe = (pid != 0).then(|| satur8_core::resolve_exe(pid)).flatten();
        self.last_focused = exe.clone().unwrap_or_else(|| class.to_string());
        match decide_focus_action(
            self.current.as_deref(),
            class,
            exe.as_deref(),
            &self.profiles,
        ) {
            FocusAction::Apply {
                profile,
                saturation,
                outputs,
            } => {
                eprintln!(
                    "satur8-daemon: '{}' -> profile '{profile}' ({:.2})",
                    exe.as_deref().unwrap_or(class),
                    saturation.get()
                );
                let result = self
                    .backend
                    .lock()
                    .expect("backend state mutex poisoned")
                    .apply(saturation, &outputs, self.profiles.default_saturation());
                match result {
                    Ok(()) => self.current = Some(profile),
                    Err(error) => {
                        self.current = None;
                        eprintln!("satur8-daemon: apply failed: {error}");
                    }
                }
            }
            FocusAction::Restore => {
                eprintln!("satur8-daemon: '{class}' has no profile -> restoring default");
                let result = self
                    .backend
                    .lock()
                    .expect("backend state mutex poisoned")
                    .restore(self.profiles.default_saturation());
                self.current = None;
                if let Err(error) = result {
                    eprintln!("satur8-daemon: restore failed: {error}");
                }
            }
            FocusAction::None => {}
        }
    }

    /// Forget which profile is applied, so the next focus applies again.
    fn invalidate_current(&mut self) {
        self.current = None;
    }

    /// Re-evaluate focus from scratch when a focus source (re)connects: apply
    /// the matching profile again, or put the default back when none matches.
    /// The default also replaces a table a crashed daemon left behind, which
    /// nothing else would clear.
    fn resync(&mut self, class: &str, pid: u32) {
        self.invalidate_current();
        self.react(class, pid);
        if self.current.is_some() {
            return;
        }
        let result = self
            .backend
            .lock()
            .expect("backend state mutex poisoned")
            .restore(self.profiles.default_saturation());
        if let Err(error) = result {
            eprintln!("satur8-daemon: restore failed: {error}");
        }
    }

    /// The active backend's name, detecting one first if none is active yet.
    fn backend_name(&mut self) -> Option<&'static str> {
        let mut backend = self.backend.lock().expect("backend state mutex poisoned");
        backend.try_detect(self.profiles.default_saturation());
        let name = backend.guard.as_ref().map(|guard| guard.backend().name());
        name
    }

    /// Whether the active backend is gamescope's.
    fn uses_gamescope(&mut self) -> bool {
        self.backend_name() == Some("gamescope")
    }

    /// Whether gamescope can still become the backend: it is, or nothing is
    /// selected yet. A selected desktop backend is never replaced.
    fn may_use_gamescope(&mut self) -> bool {
        matches!(self.backend_name(), None | Some("gamescope"))
    }

    /// The first sync for a gamescope focus source, run as soon as the backend
    /// is gamescope's. The focus thread retries while this says `NotYet`, so
    /// the sync happens when the later of the connection and the backend is
    /// ready, not at the next focus change.
    fn sync_gamescope(&mut self, class: &str, pid: u32) -> GamescopeSync {
        match self.backend_name() {
            Some("gamescope") => {
                self.resync(class, pid);
                GamescopeSync::Done
            }
            None => GamescopeSync::NotYet,
            Some(_) => GamescopeSync::NotGamescope,
        }
    }

    fn reload_profiles(&mut self) {
        let profiles = load_profiles(&self.profiles);
        self.replace_profiles(profiles);
    }

    fn replace_profiles(&mut self, profiles: Profiles) {
        self.profiles = profiles;
        if let Some(guard) = self
            .backend
            .lock()
            .expect("backend state mutex poisoned")
            .guard
            .as_mut()
        {
            guard.set_restore_to(self.profiles.default_saturation());
        }
        eprintln!(
            "satur8-daemon: reloaded, {} profile(s)",
            self.profiles.profiles.len()
        );

        let Some(current) = self.current.clone() else {
            return;
        };
        let Some(profile) = self
            .profiles
            .by_name(&current)
            .filter(|profile| profile.enabled)
        else {
            eprintln!(
                "satur8-daemon: active profile '{current}' is disabled or missing -> restoring default"
            );
            let result = self
                .backend
                .lock()
                .expect("backend state mutex poisoned")
                .restore(self.profiles.default_saturation());
            self.current = None;
            if let Err(error) = result {
                eprintln!("satur8-daemon: restore failed: {error}");
            }
            return;
        };

        let saturation = profile.saturation();
        let outputs = profile.outputs.clone();
        eprintln!(
            "satur8-daemon: reapplied active profile '{current}' ({:.2})",
            saturation.get()
        );
        let result = self
            .backend
            .lock()
            .expect("backend state mutex poisoned")
            .apply(saturation, &outputs, self.profiles.default_saturation());
        if let Err(error) = result {
            self.current = None;
            eprintln!("satur8-daemon: apply failed: {error}");
        }
    }
}

#[interface(name = "org.satur8.Daemon")]
impl Daemon {
    /// Called on every window activation. Kept for older focus scripts that
    /// cannot report a pid; matching then falls back to the window class.
    fn window_activated(&mut self, class: String, _caption: String) {
        self.react(&class, 0);
    }

    /// Window activation including the window's process id. The pid arrives as
    /// a string because KWin's `callDBus` gives no control over the numeric
    /// type it marshals, and a mismatched signature is a silent no-op.
    fn window_activated_pid(&mut self, class: String, _caption: String, pid: String) {
        self.react(&class, pid.trim().parse::<u32>().unwrap_or(0));
    }

    /// Re-read the profiles file from disk.
    fn reload(&mut self) {
        self.reload_profiles();
    }

    /// The profile currently applied (empty string if none).
    #[zbus(property)]
    fn active_profile(&self) -> String {
        self.current.clone().unwrap_or_default()
    }

    /// The window the focus script last reported (executable or window class;
    /// empty when focus went to the desktop or nothing was reported yet).
    #[zbus(property)]
    fn last_focused(&self) -> String {
        self.last_focused.clone()
    }
}

fn load_profiles(previous: &Profiles) -> Profiles {
    let Some(path) = profiles_path() else {
        return previous.clone();
    };
    let source = match std::fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return previous.clone(),
        Err(error) => {
            eprintln!(
                "satur8-daemon: failed to read {}; keeping last-known-good profiles: {error}",
                path.display()
            );
            return previous.clone();
        }
    };
    match Profiles::from_toml(&source) {
        Ok(profiles) => profiles,
        Err(error) => {
            eprintln!(
                "satur8-daemon: failed to parse {}; keeping last-known-good profiles: {error}",
                path.display()
            );
            previous.clone()
        }
    }
}

fn profiles_path() -> Option<PathBuf> {
    let dir = if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if xdg.is_empty() {
            PathBuf::from(std::env::var("HOME").ok()?).join(".config")
        } else {
            PathBuf::from(xdg)
        }
    } else {
        PathBuf::from(std::env::var("HOME").ok()?).join(".config")
    };
    Some(dir.join("satur8").join("profiles.toml"))
}

fn main() -> Result<()> {
    let profiles = load_profiles(&Profiles::default());
    let backend = Arc::new(Mutex::new(BackendState::detect(
        profiles.default_saturation(),
    )));
    let daemon = Daemon::new(profiles, backend.clone());
    let running = Arc::new(AtomicBool::new(true));
    let connection = zbus::blocking::connection::Builder::session()
        .context("connecting to the session bus")?
        .name(SERVICE)
        .context("claiming the org.satur8.Daemon bus name (already running?)")?
        .serve_at(PATH, daemon)
        .context("publishing the daemon interface")?
        .build()
        .context("starting the daemon service")?;

    if let Err(error) = gamescope_focus::spawn(connection.clone(), running.clone()) {
        eprintln!("satur8-daemon: could not start the gamescope focus watcher: {error}");
    }

    let signal_running = running.clone();
    let signal_handler = install_signal_handler(move |_| {
        signal_running.store(false, Ordering::Release);
    })
    .context("installing shutdown signal handlers")?;

    eprintln!("satur8-daemon: listening on {SERVICE} {PATH}");
    while running.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(100));
    }

    eprintln!("satur8-daemon: shutting down; restoring desktop colors");
    // Hold the interface while restoring, so the gamescope focus thread cannot
    // apply a profile after the restore.
    let daemon_interface = connection.object_server().interface::<_, Daemon>(PATH).ok();
    let daemon_lock = daemon_interface.as_ref().map(|daemon| daemon.get_mut());
    if let Some(guard) = backend
        .lock()
        .expect("backend state mutex poisoned")
        .guard
        .as_mut()
    {
        if let Err(error) = guard.restore_now() {
            eprintln!("satur8-daemon: restore failed during shutdown: {error}");
        }
    }
    drop(daemon_lock);
    drop(daemon_interface);
    drop(connection);
    drop(signal_handler);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    use satur8_backend::all_outputs;
    use satur8_core::{Backend, CostNote, MatchRule, Output, Profile};

    #[derive(Debug, Clone, Copy, PartialEq)]
    enum Call {
        Apply(&'static str, f32),
        Reset,
    }

    struct FakeBackend {
        name: &'static str,
        calls: Arc<Mutex<Vec<Call>>>,
        fail_apply: Arc<AtomicBool>,
    }

    impl Backend for FakeBackend {
        fn name(&self) -> &'static str {
            self.name
        }

        fn cost(&self) -> CostNote {
            CostNote::ZeroCost
        }

        fn outputs(&self) -> Vec<Output> {
            vec![all_outputs()]
        }

        fn apply(&mut self, output: &Output, saturation: Saturation) -> Result<(), BackendError> {
            let output = match output.id.as_str() {
                "all" => "all",
                "DP-1" => "DP-1",
                "HDMI-A-1" => "HDMI-A-1",
                other => panic!("unexpected output {other}"),
            };
            self.calls
                .lock()
                .unwrap()
                .push(Call::Apply(output, saturation.get()));
            if self.fail_apply.load(Ordering::Acquire) {
                Err(BackendError::Apply("injected failure".into()))
            } else {
                Ok(())
            }
        }

        fn reset(&mut self, _output: &Output) -> Result<(), BackendError> {
            self.calls.lock().unwrap().push(Call::Reset);
            Ok(())
        }
    }

    fn profiles() -> Profiles {
        Profiles {
            default_saturation: 1.0,
            profiles: vec![Profile {
                name: "game".into(),
                enabled: true,
                saturation: 1.5,
                match_rule: MatchRule {
                    exe: None,
                    window_class: Some("game.class".into()),
                    steam_app_id: None,
                },
                outputs: vec![],
            }],
        }
    }

    fn setup() -> (Daemon, Arc<Mutex<Vec<Call>>>, Arc<AtomicBool>) {
        setup_with(profiles())
    }

    fn setup_with(profiles: Profiles) -> (Daemon, Arc<Mutex<Vec<Call>>>, Arc<AtomicBool>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let fail_apply = Arc::new(AtomicBool::new(false));
        let backend = FakeBackend {
            name: "fake",
            calls: calls.clone(),
            fail_apply: fail_apply.clone(),
        };
        (
            Daemon::with_profiles(profiles, Box::new(backend)),
            calls,
            fail_apply,
        )
    }

    /// A Steam profile for CS2 (AppID 730).
    fn steam_profiles() -> Profiles {
        Profiles {
            default_saturation: 1.0,
            profiles: vec![Profile {
                name: "cs2".into(),
                enabled: true,
                saturation: 2.0,
                match_rule: MatchRule {
                    exe: None,
                    window_class: None,
                    steam_app_id: Some(730),
                },
                outputs: vec![],
            }],
        }
    }

    fn game_mode_focus(app_id: u32) -> (String, u32) {
        gamescope_focus::activation(satur8_gamescope::session::Focus {
            app_id: Some(app_id),
            pid: None,
        })
    }

    #[test]
    fn applies_on_focus_match() {
        let (mut daemon, calls, _) = setup();
        daemon.react("game.class", 0);
        assert_eq!(daemon.current.as_deref(), Some("game"));
        assert_eq!(*calls.lock().unwrap(), [Call::Apply("all", 1.5)]);
    }

    #[test]
    fn remembers_the_last_focused_window_for_the_gui() {
        let (mut daemon, _, _) = setup();
        assert_eq!(daemon.last_focused(), "");
        daemon.react("game.class", 0);
        assert_eq!(daemon.last_focused(), "game.class");
        daemon.react("", 0);
        assert_eq!(daemon.last_focused(), "");
    }

    #[test]
    fn restores_on_focus_loss() {
        let (mut daemon, calls, _) = setup();
        daemon.react("game.class", 0);
        daemon.react("desktop", 0);
        assert_eq!(daemon.current, None);
        assert_eq!(
            *calls.lock().unwrap(),
            [Call::Apply("all", 1.5), Call::Reset]
        );
    }

    #[test]
    fn same_profile_is_a_no_op_while_healthy() {
        let (mut daemon, calls, _) = setup();
        daemon.react("game.class", 0);
        daemon.react("game.class", 0);
        assert_eq!(*calls.lock().unwrap(), [Call::Apply("all", 1.5)]);
    }

    #[test]
    fn failed_apply_is_retried_on_next_focus() {
        let (mut daemon, calls, fail_apply) = setup();
        fail_apply.store(true, Ordering::Release);
        daemon.react("game.class", 0);
        assert_eq!(daemon.current, None);
        fail_apply.store(false, Ordering::Release);
        daemon.react("game.class", 0);
        assert_eq!(daemon.current.as_deref(), Some("game"));
        assert_eq!(
            *calls.lock().unwrap(),
            [Call::Apply("all", 1.5), Call::Apply("all", 1.5)]
        );
    }

    #[test]
    fn invalidation_reapplies_the_same_profile() {
        let (mut daemon, calls, _) = setup();
        daemon.react("game.class", 0);
        daemon.invalidate_current();
        daemon.react("game.class", 0);
        assert_eq!(
            *calls.lock().unwrap(),
            [Call::Apply("all", 1.5), Call::Apply("all", 1.5)]
        );
    }

    #[test]
    fn applies_profile_to_each_configured_output() {
        let (mut daemon, calls, _) = setup();
        daemon.profiles.profiles[0].outputs = vec!["DP-1".into(), "HDMI-A-1".into()];

        daemon.react("game.class", 0);

        assert_eq!(
            *calls.lock().unwrap(),
            [Call::Apply("DP-1", 1.5), Call::Apply("HDMI-A-1", 1.5)]
        );
    }

    #[test]
    fn disabling_the_active_profile_restores_the_desktop() {
        let (mut daemon, calls, _) = setup();
        daemon.react("game.class", 0);

        let mut updated = daemon.profiles.clone();
        updated.profiles[0].enabled = false;
        daemon.replace_profiles(updated);

        assert_eq!(daemon.current, None);
        assert_eq!(
            *calls.lock().unwrap(),
            [Call::Apply("all", 1.5), Call::Reset]
        );
    }

    #[test]
    fn game_mode_focus_on_a_steam_app_applies_its_profile() {
        let (mut daemon, calls, _) = setup_with(steam_profiles());
        let (class, pid) = game_mode_focus(730);
        daemon.react(&class, pid);
        assert_eq!(daemon.current.as_deref(), Some("cs2"));
        assert_eq!(*calls.lock().unwrap(), [Call::Apply("all", 2.0)]);
    }

    #[test]
    fn game_mode_focus_back_on_the_steam_ui_restores_the_default() {
        let (mut daemon, calls, _) = setup_with(steam_profiles());
        let (class, pid) = game_mode_focus(730);
        daemon.react(&class, pid);
        let (class, pid) = game_mode_focus(769);
        daemon.react(&class, pid);
        assert_eq!(daemon.current, None);
        assert_eq!(
            *calls.lock().unwrap(),
            [Call::Apply("all", 2.0), Call::Reset]
        );
    }

    #[test]
    fn a_profile_for_the_steam_ui_applies_at_home() {
        let mut profiles = steam_profiles();
        profiles.profiles[0].match_rule.steam_app_id = Some(769);
        let (mut daemon, calls, _) = setup_with(profiles);
        let (class, pid) = game_mode_focus(769);
        daemon.react(&class, pid);
        assert_eq!(*calls.lock().unwrap(), [Call::Apply("all", 2.0)]);
    }

    #[test]
    fn reconnecting_reapplies_the_focused_profile() {
        let (mut daemon, calls, _) = setup_with(steam_profiles());
        let (class, pid) = game_mode_focus(730);
        daemon.react(&class, pid);
        daemon.resync(&class, pid);
        assert_eq!(daemon.current.as_deref(), Some("cs2"));
        assert_eq!(
            *calls.lock().unwrap(),
            [Call::Apply("all", 2.0), Call::Apply("all", 2.0)]
        );
    }

    #[test]
    fn startup_sync_without_a_profile_restores_the_default() {
        let (mut daemon, calls, _) = setup_with(steam_profiles());
        let (class, pid) = game_mode_focus(769);
        daemon.resync(&class, pid);
        assert_eq!(daemon.current, None);
        assert_eq!(*calls.lock().unwrap(), [Call::Reset]);
    }

    #[test]
    fn startup_sync_puts_a_non_identity_default_back() {
        let mut profiles = steam_profiles();
        profiles.default_saturation = 1.25;
        let (mut daemon, calls, _) = setup_with(profiles);
        let (class, pid) = game_mode_focus(769);
        daemon.resync(&class, pid);
        assert_eq!(*calls.lock().unwrap(), [Call::Apply("all", 1.25)]);
    }

    #[test]
    fn a_desktop_backend_keeps_the_gamescope_focus_thread_out() {
        let (mut daemon, _, _) = setup();
        assert!(!daemon.uses_gamescope());
        assert!(!daemon.may_use_gamescope());
    }

    #[test]
    fn game_mode_sync_waits_for_the_backend_and_runs_once_it_is_ready() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let state = Arc::new(Mutex::new(BackendState::pending()));
        let mut daemon = Daemon::new(steam_profiles(), state.clone());
        let (class, pid) = game_mode_focus(730);

        // The focus thread found gamescope first: nothing to apply through yet.
        assert_eq!(daemon.sync_gamescope(&class, pid), GamescopeSync::NotYet);
        assert!(calls.lock().unwrap().is_empty());

        // Backend detection finishes; the thread's next retry syncs at once.
        let backend = FakeBackend {
            name: "gamescope",
            calls: calls.clone(),
            fail_apply: Arc::new(AtomicBool::new(false)),
        };
        state.lock().unwrap().guard =
            Some(RestoreGuard::new(Box::new(backend), Saturation::IDENTITY));
        assert_eq!(daemon.sync_gamescope(&class, pid), GamescopeSync::Done);
        assert_eq!(daemon.current.as_deref(), Some("cs2"));
        assert_eq!(*calls.lock().unwrap(), [Call::Apply("all", 2.0)]);
    }

    #[test]
    fn game_mode_sync_steps_aside_for_a_desktop_backend() {
        let (mut daemon, calls, _) = setup_with(steam_profiles());
        let (class, pid) = game_mode_focus(730);
        assert_eq!(
            daemon.sync_gamescope(&class, pid),
            GamescopeSync::NotGamescope
        );
        assert!(calls.lock().unwrap().is_empty());
    }
}
