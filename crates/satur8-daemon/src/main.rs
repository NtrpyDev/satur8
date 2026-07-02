//! `satur8-daemon` - event-driven saturation changes on window focus.
//!
//! The companion KWin script forwards activations over D-Bus. Other callers
//! can use the same interface, and backend selection is shared with every
//! Satur8 frontend.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use satur8_backend::{apply_to_outputs, install_signal_handler, select_backend, RestoreGuard};
use satur8_core::{BackendError, Profiles, Saturation};
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

fn decide_focus_action(
    current: Option<&str>,
    window_class: &str,
    profiles: &Profiles,
) -> FocusAction {
    match profiles.match_window_class(window_class) {
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
        }
    }

    #[cfg(test)]
    fn with_profiles(profiles: Profiles, backend: Box<dyn satur8_core::Backend>) -> Self {
        let restore_to = profiles.default_saturation();
        Self {
            profiles,
            backend: Arc::new(Mutex::new(BackendState::with_backend(backend, restore_to))),
            current: None,
        }
    }

    fn react(&mut self, class: &str) {
        self.backend
            .lock()
            .expect("backend state mutex poisoned")
            .try_detect(self.profiles.default_saturation());
        match decide_focus_action(self.current.as_deref(), class, &self.profiles) {
            FocusAction::Apply {
                profile,
                saturation,
                outputs,
            } => {
                eprintln!(
                    "satur8-daemon: '{class}' -> profile '{profile}' ({:.2})",
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

    #[cfg(test)]
    fn invalidate_current(&mut self) {
        self.current = None;
    }

    fn reload_profiles(&mut self) {
        self.profiles = load_profiles(&self.profiles);
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
        let Some(profile) = self.profiles.by_name(&current) else {
            eprintln!(
                "satur8-daemon: active profile '{current}' no longer exists -> restoring default"
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
    /// Called on every window activation.
    fn window_activated(&mut self, class: String, _caption: String) {
        self.react(&class);
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
    let connection = zbus::blocking::connection::Builder::session()
        .context("connecting to the session bus")?
        .name(SERVICE)
        .context("claiming the org.satur8.Daemon bus name (already running?)")?
        .serve_at(PATH, daemon)
        .context("publishing the daemon interface")?
        .build()
        .context("starting the daemon service")?;

    let running = Arc::new(AtomicBool::new(true));
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
        calls: Arc<Mutex<Vec<Call>>>,
        fail_apply: Arc<AtomicBool>,
    }

    impl Backend for FakeBackend {
        fn name(&self) -> &'static str {
            "fake"
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
        let calls = Arc::new(Mutex::new(Vec::new()));
        let fail_apply = Arc::new(AtomicBool::new(false));
        let backend = FakeBackend {
            calls: calls.clone(),
            fail_apply: fail_apply.clone(),
        };
        (
            Daemon::with_profiles(profiles(), Box::new(backend)),
            calls,
            fail_apply,
        )
    }

    #[test]
    fn applies_on_focus_match() {
        let (mut daemon, calls, _) = setup();
        daemon.react("game.class");
        assert_eq!(daemon.current.as_deref(), Some("game"));
        assert_eq!(*calls.lock().unwrap(), [Call::Apply("all", 1.5)]);
    }

    #[test]
    fn restores_on_focus_loss() {
        let (mut daemon, calls, _) = setup();
        daemon.react("game.class");
        daemon.react("desktop");
        assert_eq!(daemon.current, None);
        assert_eq!(
            *calls.lock().unwrap(),
            [Call::Apply("all", 1.5), Call::Reset]
        );
    }

    #[test]
    fn same_profile_is_a_no_op_while_healthy() {
        let (mut daemon, calls, _) = setup();
        daemon.react("game.class");
        daemon.react("game.class");
        assert_eq!(*calls.lock().unwrap(), [Call::Apply("all", 1.5)]);
    }

    #[test]
    fn failed_apply_is_retried_on_next_focus() {
        let (mut daemon, calls, fail_apply) = setup();
        fail_apply.store(true, Ordering::Release);
        daemon.react("game.class");
        assert_eq!(daemon.current, None);
        fail_apply.store(false, Ordering::Release);
        daemon.react("game.class");
        assert_eq!(daemon.current.as_deref(), Some("game"));
        assert_eq!(
            *calls.lock().unwrap(),
            [Call::Apply("all", 1.5), Call::Apply("all", 1.5)]
        );
    }

    #[test]
    fn invalidation_reapplies_the_same_profile() {
        let (mut daemon, calls, _) = setup();
        daemon.react("game.class");
        daemon.invalidate_current();
        daemon.react("game.class");
        assert_eq!(
            *calls.lock().unwrap(),
            [Call::Apply("all", 1.5), Call::Apply("all", 1.5)]
        );
    }

    #[test]
    fn applies_profile_to_each_configured_output() {
        let (mut daemon, calls, _) = setup();
        daemon.profiles.profiles[0].outputs = vec!["DP-1".into(), "HDMI-A-1".into()];

        daemon.react("game.class");

        assert_eq!(
            *calls.lock().unwrap(),
            [Call::Apply("DP-1", 1.5), Call::Apply("HDMI-A-1", 1.5)]
        );
    }
}
