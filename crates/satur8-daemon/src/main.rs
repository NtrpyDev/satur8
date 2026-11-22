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
use satur8_backend::{install_signal_handler, select_backend, RestoreGuard};
use satur8_core::levels::{desktop_picture, focus_picture, profile_outputs_not_connected};
use satur8_core::{Backend, BackendError, Levels, Note, Profile, ProfileOutputs, Profiles};
use zbus::interface;

const SERVICE: &str = "org.satur8.Daemon";
const PATH: &str = "/org/satur8/Daemon";
const INITIAL_RETRY: Duration = Duration::from_millis(250);
const MAX_RETRY: Duration = Duration::from_secs(8);

#[derive(Debug, PartialEq)]
enum FocusAction {
    Apply(Profile),
    Restore,
    None,
}

/// The focused window's profile, if any. The pid's real executable is tried
/// first because it is the only thing that identifies a Proton game: every
/// non-Steam title launched through umu shares the window class `steam_app_0`,
/// so class alone cannot tell Tarkov from anything else launched the same way.
/// The class remains the fallback for windows we cannot resolve a pid for.
///
/// A profile that is off still matches: its game is then shown with normal
/// colors, not the general saturation. A profile that is on always wins over
/// one that is off, whichever of the two ways found it.
fn match_focused<'a>(
    profiles: &'a Profiles,
    window_class: &str,
    exe: Option<&str>,
) -> Option<&'a Profile> {
    let by_exe = exe.and_then(|exe| profiles.match_exe(exe));
    let by_class = profiles.match_window_class(window_class);
    match (by_exe, by_class) {
        (Some(profile), _) if profile.enabled => Some(profile),
        (_, Some(profile)) if profile.enabled => Some(profile),
        (by_exe, by_class) => by_exe.or(by_class),
    }
}

/// How a profile's value reads in the log: `1.60`, or that it is off.
fn shown_text(profile: &Profile) -> String {
    if profile.enabled {
        format!("{:.2}", profile.shown_saturation().get())
    } else {
        "off: normal colors".to_string()
    }
}

/// The output a profile's picture is limited to: the focused output, when the
/// profile follows focus (`profile_outputs = "focused"` and no outputs of its
/// own), the backend can target one output, and the focus source reported it.
fn limited_output(
    profiles: &Profiles,
    profile: &Profile,
    focused_output: Option<&str>,
    per_output: bool,
) -> Option<String> {
    let follows_focus =
        profiles.profile_outputs == ProfileOutputs::Focused && profile.outputs.is_empty();
    (per_output && follows_focus)
        .then_some(focused_output)
        .flatten()
        .map(str::to_string)
}

/// Whether a reload shows the desktop picture now: only with no profile
/// applied, and only when the new file resolves to a different desktop than the
/// old one. An unrelated edit then leaves the screen alone, so a manual
/// `satur8 set` or the app's live slider value is not wiped by it.
fn reload_shows_desktop(profile_active: bool, old: &Levels, new: &Levels) -> bool {
    !profile_active && old != new
}

/// What the profile already applied, so a repeated report is a no-op and a
/// move to another output is a change.
#[derive(Debug, Clone, Copy, Default)]
struct Current<'a> {
    profile: Option<&'a str>,
    output: Option<&'a str>,
}

/// The window class to match a focused window by. A Proton game's window
/// class is already `steam_app_<AppID>`, which a profile with that AppID
/// matches. A native Steam game's window class is its own (`cs2`), so a
/// profile that knows only the game's AppID, as one added from the app's list
/// of installed games does, would never match it. When neither the
/// executable nor the class finds a profile and the window's process carries
/// a Steam AppID that a profile has, this is `steam_app_<AppID>`. The same
/// when they find only a profile that is off and the AppID has one that is on.
fn class_to_match(
    profiles: &Profiles,
    window_class: &str,
    exe: Option<&str>,
    app_id: Option<u32>,
) -> Option<String> {
    let direct = match_focused(profiles, window_class, exe);
    if direct.is_some_and(|profile| profile.enabled) {
        return None;
    }
    let app_id = app_id?;
    let by_app_id = profiles.match_steam_app_id(app_id)?;
    (direct.is_none() || by_app_id.enabled).then(|| format!("steam_app_{app_id}"))
}

fn decide_focus_action(
    current: Current,
    window_class: &str,
    exe: Option<&str>,
    focused_output: Option<&str>,
    per_output: bool,
    profiles: &Profiles,
) -> FocusAction {
    match match_focused(profiles, window_class, exe) {
        Some(profile)
            if current.profile == Some(profile.name.as_str())
                && current.output
                    == limited_output(profiles, profile, focused_output, per_output).as_deref() =>
        {
            FocusAction::None
        }
        Some(profile) => FocusAction::Apply(profile.clone()),
        None if current.profile.is_some() => FocusAction::Restore,
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

/// The desktop picture `backend` restores to for `profiles`.
fn desktop_levels(profiles: &Profiles, backend: &dyn Backend) -> Levels {
    desktop_picture(profiles, backend.per_output()).levels
}

/// The notes for a picture, plus the backend's reason when the picture could
/// not be what the file asks for because the backend shows one value on every
/// output (an older KWin effect still loaded, for example).
fn with_backend_reason(mut notes: Vec<Note>, backend: &dyn Backend) -> Vec<Note> {
    if !notes.is_empty() && !backend.per_output() {
        if let Some(reason) = backend.per_output_reason() {
            notes.push(Note::OneValueBecause { reason });
        }
    }
    notes
}

/// Whether a backend that has just become reachable shows the desktop picture
/// now: only with no profile applied and a desktop that is not unchanged, so
/// an unchanged desktop loads nothing (no KWin effect for nothing). gamescope
/// is left to its own first sync, which already puts the desktop back.
fn shows_desktop_at_detection(backend: &str, profile_active: bool, desktop: &Levels) -> bool {
    backend != "gamescope" && !profile_active && !desktop.is_identity()
}

struct BackendState {
    guard: Option<RestoreGuard>,
    retry_delay: Duration,
    next_retry: Instant,
    /// A backend was found since the daemon last looked, so the desktop
    /// picture may need showing.
    just_detected: bool,
}

impl BackendState {
    fn detect(profiles: &Profiles) -> Self {
        let mut state = Self {
            guard: None,
            retry_delay: INITIAL_RETRY,
            next_retry: Instant::now(),
            just_detected: false,
        };
        state.try_detect(profiles);
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
            just_detected: false,
        }
    }

    #[cfg(test)]
    fn with_backend(backend: Box<dyn Backend>, profiles: &Profiles) -> Self {
        let restore_to = desktop_levels(profiles, backend.as_ref());
        Self {
            guard: Some(RestoreGuard::new(backend, restore_to)),
            retry_delay: INITIAL_RETRY,
            next_retry: Instant::now(),
            just_detected: false,
        }
    }

    fn try_detect(&mut self, profiles: &Profiles) {
        if self.guard.is_some() || Instant::now() < self.next_retry {
            return;
        }
        match select_backend() {
            Ok(backend) => {
                eprintln!("satur8-daemon: {} backend active", backend.name());
                let restore_to = desktop_levels(profiles, backend.as_ref());
                self.guard = Some(RestoreGuard::new(backend, restore_to));
                self.retry_delay = INITIAL_RETRY;
                self.just_detected = true;
            }
            Err(error) => {
                eprintln!("satur8-daemon: backend unavailable; will retry on focus: {error}");
                self.next_retry = Instant::now() + self.retry_delay;
                self.retry_delay = (self.retry_delay * 2).min(MAX_RETRY);
            }
        }
    }

    /// Whether a backend became reachable since the last call, and the desktop
    /// picture it should show then.
    fn take_detected(&mut self, profiles: &Profiles) -> Option<(&'static str, Levels)> {
        if !std::mem::take(&mut self.just_detected) {
            return None;
        }
        let backend = self.guard.as_ref()?.backend();
        Some((backend.name(), desktop_levels(profiles, backend)))
    }

    /// Whether the active backend can show a different saturation on each
    /// output; false while no backend is active.
    fn per_output(&self) -> bool {
        self.guard
            .as_ref()
            .is_some_and(|guard| guard.backend().per_output())
    }

    /// Show `profile` and remember the desktop to put back. Returns what the
    /// user should be told about the result, and the output the picture was
    /// limited to.
    fn apply_profile(
        &mut self,
        profiles: &Profiles,
        profile: &Profile,
        focused_output: Option<&str>,
    ) -> Result<(Vec<Note>, Option<String>), BackendError> {
        self.try_detect(profiles);
        let guard = self
            .guard
            .as_mut()
            .ok_or_else(|| BackendError::Unavailable("no backend is currently reachable".into()))?;
        let per_output = guard.backend().per_output();
        let desktop = desktop_picture(profiles, per_output);
        let picture = focus_picture(profiles, Some(profile), focused_output, per_output);
        let limited_to = limited_output(profiles, profile, focused_output, per_output);
        let not_connected = if per_output {
            profile_outputs_not_connected(profiles, profile, &guard.backend().outputs())
        } else {
            Vec::new()
        };
        guard.set_restore_to(desktop.levels);
        guard.backend_mut().apply_levels(&picture.levels)?;
        guard.arm();
        let notes = desktop
            .notes
            .into_iter()
            .chain(picture.notes)
            .chain(not_connected)
            .collect();
        Ok((with_backend_reason(notes, guard.backend()), limited_to))
    }

    /// Put the desktop back. Returns what the user should be told about it.
    fn restore(&mut self, profiles: &Profiles) -> Result<Vec<Note>, BackendError> {
        self.try_detect(profiles);
        let guard = self
            .guard
            .as_mut()
            .ok_or_else(|| BackendError::Unavailable("no backend is currently reachable".into()))?;
        let desktop = focus_picture(profiles, None, None, guard.backend().per_output());
        guard.set_restore_to(desktop.levels);
        guard.restore_now()?;
        Ok(with_backend_reason(desktop.notes, guard.backend()))
    }
}

/// The last focus a focus source reported, kept so a profile reload can match
/// it again without waiting for the next focus change.
struct Activation {
    class: String,
    /// The pid resolved to its program name when the focus was reported.
    exe: Option<String>,
    /// The output the focused window is on, when the focus source said.
    output: Option<String>,
}

struct Daemon {
    profiles: Profiles,
    backend: Arc<Mutex<BackendState>>,
    current: Option<String>,
    /// The output the applied profile was limited to, when it was.
    current_output: Option<String>,
    /// What the focus script last reported: the resolved executable when the
    /// pid could be resolved, otherwise the window class. Read by the GUI.
    last_focused: String,
    /// The last focus `react` saw, from the KWin script or the gamescope
    /// focus thread.
    last_activation: Option<Activation>,
    /// Notes already logged since the profiles were last loaded, so each is
    /// logged once per profile and reason.
    logged_notes: Vec<Note>,
    /// The desktop picture failed to apply, so the next focus report with no
    /// profile tries again.
    desktop_pending: bool,
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
            current_output: None,
            last_focused: String::new(),
            last_activation: None,
            logged_notes: Vec::new(),
            desktop_pending: false,
        }
    }

    #[cfg(test)]
    fn with_profiles(profiles: Profiles, backend: Box<dyn Backend>) -> Self {
        let state = BackendState::with_backend(backend, &profiles);
        Self {
            profiles,
            backend: Arc::new(Mutex::new(state)),
            current: None,
            current_output: None,
            last_focused: String::new(),
            last_activation: None,
            logged_notes: Vec::new(),
            desktop_pending: false,
        }
    }

    /// Log each note the first time it comes up.
    fn log_notes(&mut self, notes: Vec<Note>) {
        for note in notes {
            if !self.logged_notes.contains(&note) {
                eprintln!("satur8-daemon: {note}");
                self.logged_notes.push(note);
            }
        }
    }

    /// A focus report with no output: the KWin script before per-output
    /// support, and the gamescope focus thread.
    fn react(&mut self, class: &str, pid: u32) {
        self.react_on(class, pid, None);
    }

    /// A focus report, with the output the focused window is on when the
    /// focus source knows it. A report for the same window on another output
    /// re-resolves a profile that follows focus.
    fn react_on(&mut self, class: &str, pid: u32, output: Option<&str>) {
        let (per_output, detected) = {
            let mut backend = self.backend.lock().expect("backend state mutex poisoned");
            backend.try_detect(&self.profiles);
            (backend.per_output(), backend.take_detected(&self.profiles))
        };
        // A backend reachable for the first time shows the desktop picture,
        // unless this report applies a profile, which then wins. The picture is
        // shown below, through the same path as a retry.
        if let Some((name, desktop)) = detected {
            if shows_desktop_at_detection(name, self.current.is_some(), &desktop) {
                self.desktop_pending = true;
            }
        }
        let exe = (pid != 0).then(|| satur8_core::resolve_exe(pid)).flatten();
        self.last_focused = exe.clone().unwrap_or_else(|| class.to_string());
        let by_app_id = (pid != 0)
            .then(|| satur8_core::proc::steam_app_id_of(pid))
            .flatten()
            .and_then(|app_id| class_to_match(&self.profiles, class, exe.as_deref(), Some(app_id)));
        let class = by_app_id.as_deref().unwrap_or(class);
        self.last_activation = Some(Activation {
            class: class.to_string(),
            exe: exe.clone(),
            output: output.map(str::to_string),
        });
        let current = Current {
            profile: self.current.as_deref(),
            output: self.current_output.as_deref(),
        };
        match decide_focus_action(
            current,
            class,
            exe.as_deref(),
            output,
            per_output,
            &self.profiles,
        ) {
            FocusAction::Apply(profile) => {
                eprintln!(
                    "satur8-daemon: '{}' -> profile '{}' ({}){}",
                    exe.as_deref().unwrap_or(class),
                    profile.name,
                    shown_text(&profile),
                    output.map(|o| format!(" on {o}")).unwrap_or_default()
                );
                self.show_profile(&profile, output);
            }
            FocusAction::Restore => {
                eprintln!("satur8-daemon: '{class}' has no profile -> restoring default");
                self.invalidate_current();
                self.restore_default();
            }
            FocusAction::None if self.current.is_none() && self.desktop_pending => {
                eprintln!("satur8-daemon: showing the desktop picture");
                self.restore_default();
            }
            FocusAction::None => {}
        }
    }

    /// Apply `profile` for a window on `focused_output`, and remember it.
    fn show_profile(&mut self, profile: &Profile, focused_output: Option<&str>) {
        let result = self
            .backend
            .lock()
            .expect("backend state mutex poisoned")
            .apply_profile(&self.profiles, profile, focused_output);
        match result {
            Ok((notes, limited_to)) => {
                self.current = Some(profile.name.clone());
                self.current_output = limited_to;
                self.log_notes(notes);
            }
            Err(error) => {
                self.invalidate_current();
                eprintln!("satur8-daemon: apply failed: {error}");
            }
        }
    }

    /// Forget which profile is applied, so the next focus applies again.
    fn invalidate_current(&mut self) {
        self.current = None;
        self.current_output = None;
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
        self.restore_default();
    }

    /// At start: show the desktop picture once when a backend is reachable, no
    /// profile is applied, and the desktop is not unchanged. A failure is tried
    /// again at the next focus report with no profile.
    fn show_desktop_at_start(&mut self) {
        let detected = self
            .backend
            .lock()
            .expect("backend state mutex poisoned")
            .take_detected(&self.profiles);
        let Some((name, desktop)) = detected else {
            return;
        };
        if shows_desktop_at_detection(name, self.current.is_some(), &desktop) {
            eprintln!("satur8-daemon: showing the desktop picture");
            self.restore_default();
        }
    }

    /// Put the desktop back, logging a failure and any notes. A failure is
    /// tried again at the next focus report with no profile.
    fn restore_default(&mut self) {
        let result = self
            .backend
            .lock()
            .expect("backend state mutex poisoned")
            .restore(&self.profiles);
        match result {
            Ok(notes) => {
                self.desktop_pending = false;
                self.log_notes(notes);
            }
            Err(error) => {
                self.desktop_pending = true;
                eprintln!("satur8-daemon: restore failed: {error}");
            }
        }
    }

    /// The active backend's name, detecting one first if none is active yet.
    fn backend_name(&mut self) -> Option<&'static str> {
        let mut backend = self.backend.lock().expect("backend state mutex poisoned");
        backend.try_detect(&self.profiles);
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

    /// Take new profiles and match the last focus against them, so a profile
    /// added, changed, disabled or removed for the app on screen takes effect
    /// now. Nothing is applied before any focus was seen or while no backend is
    /// active.
    fn replace_profiles(&mut self, profiles: Profiles) {
        let old_profiles = std::mem::replace(&mut self.profiles, profiles);
        self.logged_notes.clear();
        // The desktop picture before and after, for the active backend.
        let desktops = match self
            .backend
            .lock()
            .expect("backend state mutex poisoned")
            .guard
            .as_mut()
        {
            Some(guard) => {
                let old = desktop_levels(&old_profiles, guard.backend());
                let new = desktop_levels(&self.profiles, guard.backend());
                guard.set_restore_to(new.clone());
                Some((old, new))
            }
            None => None,
        };
        eprintln!(
            "satur8-daemon: reloaded, {} profile(s)",
            self.profiles.profiles.len()
        );
        let Some((old_desktop, new_desktop)) = desktops else {
            return;
        };
        let activation = self.last_activation.as_ref().map(|activation| {
            (
                activation.class.clone(),
                activation.exe.clone(),
                activation.output.clone(),
            )
        });
        let (class, exe, output) = activation.clone().unwrap_or_default();
        let target = activation
            .as_ref()
            .and_then(|(class, exe, _)| match_focused(&self.profiles, class, exe.as_deref()))
            .cloned();
        match (target, self.current.clone()) {
            (Some(profile), current) => {
                let name = profile.name.clone();
                let shown = shown_text(&profile);
                if current.as_deref() == Some(name.as_str()) {
                    eprintln!("satur8-daemon: reapplied active profile '{name}' ({shown})");
                } else {
                    eprintln!(
                        "satur8-daemon: '{}' -> profile '{name}' ({shown}) after reload",
                        exe.as_deref().unwrap_or(&class),
                    );
                }
                self.show_profile(&profile, output.as_deref());
            }
            (None, Some(current)) => {
                // A profile that was turned off still matches, so this is a
                // profile that was removed or no longer names this window.
                if self.profiles.by_name(&current).is_some() {
                    eprintln!(
                        "satur8-daemon: active profile '{current}' no longer matches '{}' -> restoring default",
                        exe.as_deref().unwrap_or(&class)
                    );
                } else {
                    eprintln!(
                        "satur8-daemon: active profile '{current}' was removed -> restoring default"
                    );
                }
                self.invalidate_current();
                self.restore_default();
            }
            (None, None) => {
                if reload_shows_desktop(false, &old_desktop, &new_desktop) {
                    eprintln!("satur8-daemon: the desktop settings changed -> showing them");
                    self.restore_default();
                }
            }
        }
    }
}

// Focus reports are handled in the order they arrive (`spawn = false`): zbus
// otherwise gives each call its own task, and two quick reports could then be
// applied in the wrong order, leaving the profile of the window that lost focus.
#[interface(name = "org.satur8.Daemon", spawn = false)]
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

    /// Window activation with the output the window is on, also sent again
    /// when the focused window moves to another output. `output` is empty
    /// when the script knows none. Returns true, so a focus script can tell
    /// this daemon understands the call: an older daemon answers with an error.
    fn window_activated_on(
        &mut self,
        class: String,
        _caption: String,
        pid: String,
        output: String,
    ) -> bool {
        let output = output.trim();
        self.react_on(
            &class,
            pid.trim().parse::<u32>().unwrap_or(0),
            (!output.is_empty()).then_some(output),
        );
        true
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

    /// The output the applied profile is limited to (empty string when it
    /// changes every output, or none is applied).
    #[zbus(property)]
    fn active_output(&self) -> String {
        self.current_output.clone().unwrap_or_default()
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
    let backend = Arc::new(Mutex::new(BackendState::detect(&profiles)));
    let mut daemon = Daemon::new(profiles, backend.clone());
    daemon.show_desktop_at_start();
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
    use satur8_core::{CostNote, MatchRule, Output, OutputConfig, ProfileOutputs, Saturation};

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
            ..Profiles::default()
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
            ..Profiles::default()
        }
    }

    fn game_mode_focus(app_id: u32) -> (String, u32) {
        gamescope_focus::activation(satur8_gamescope::session::Focus {
            app_id: Some(app_id),
            pid: None,
        })
    }

    /// CS2 on a desktop: a native Steam game whose window class is `cs2`,
    /// with a profile added from the app's list, which knows only AppID 730.
    #[test]
    fn a_native_steam_game_matches_a_profile_that_knows_only_its_app_id() {
        let by_app_id = steam_profiles();
        assert_eq!(
            class_to_match(&by_app_id, "cs2", Some("cs2"), Some(730)).as_deref(),
            Some("steam_app_730")
        );
        assert!(match_focused(&by_app_id, "steam_app_730", Some("cs2")).is_some());
        // No AppID on the process, or one no profile has: nothing changes.
        assert_eq!(class_to_match(&by_app_id, "cs2", Some("cs2"), None), None);
        assert_eq!(
            class_to_match(&by_app_id, "cs2", Some("cs2"), Some(570)),
            None
        );
        // A profile that already matches by executable or class wins.
        let mut by_exe = steam_profiles();
        by_exe.profiles.insert(
            0,
            Profile {
                name: "by exe".into(),
                enabled: true,
                saturation: 1.2,
                match_rule: MatchRule {
                    exe: Some("cs2".into()),
                    window_class: None,
                    steam_app_id: None,
                },
                outputs: vec![],
            },
        );
        assert_eq!(class_to_match(&by_exe, "cs2", Some("cs2"), Some(730)), None);
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

    /// Rewritten for per-output support: a backend that shows one saturation
    /// on every output gets the profile once, on every output, instead of one
    /// call per named output it would ignore anyway.
    #[test]
    fn a_profile_naming_outputs_changes_every_output_on_a_uniform_backend() {
        let (mut daemon, calls, _) = setup();
        daemon.profiles.profiles[0].outputs = vec!["DP-1".into(), "HDMI-A-1".into()];

        daemon.react("game.class", 0);

        assert_eq!(*calls.lock().unwrap(), [Call::Apply("all", 1.5)]);
        assert_eq!(
            daemon.logged_notes,
            [Note::ProfileChangesEveryOutput {
                profile: "game".into()
            }]
        );
    }

    /// Rewritten for the off-means-normal rule: this used to pin that turning
    /// the active profile off put the desktop back.
    #[test]
    fn turning_the_active_profile_off_shows_normal_colors() {
        let (mut daemon, calls, _) = setup();
        daemon.profiles.default_saturation = 1.3;
        daemon.react("game.class", 0);

        let mut updated = daemon.profiles.clone();
        updated.profiles[0].enabled = false;
        daemon.replace_profiles(updated.clone());

        // Still the game's profile, now showing 1.0, not the general 1.3.
        assert_eq!(daemon.current.as_deref(), Some("game"));
        assert_eq!(
            *calls.lock().unwrap(),
            [Call::Apply("all", 1.5), Call::Apply("all", 1.0)]
        );

        // Turning it on again brings its value back, with the game in front
        // the whole time.
        updated.profiles[0].enabled = true;
        daemon.replace_profiles(updated);
        assert_eq!(calls.lock().unwrap().last(), Some(&Call::Apply("all", 1.5)));
    }

    #[test]
    fn removing_the_active_profile_restores_the_general_value() {
        let (mut daemon, calls, _) = setup();
        daemon.react("game.class", 0);

        let mut updated = daemon.profiles.clone();
        updated.profiles.clear();
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

    /// General value 1.3; CS2 (730) on at 2.0; ELDEN RING (1245620) off at
    /// 1.8; nothing for Dota 2 (570).
    fn four_case_profiles() -> Profiles {
        let mut profiles = steam_profiles();
        profiles.default_saturation = 1.3;
        profiles.profiles.push(Profile {
            name: "ELDEN RING".into(),
            enabled: false,
            saturation: 1.8,
            match_rule: MatchRule {
                exe: None,
                window_class: None,
                steam_app_id: Some(1245620),
            },
            outputs: vec![],
        });
        profiles
    }

    /// The four cases in Steam Game Mode. Each starts from another app in
    /// front, so the value asserted is the one this focus led to.
    #[test]
    fn game_mode_shows_the_right_value_for_each_of_the_four_cases() {
        // The app in front before, the focused AppID, the profile the daemon
        // holds afterwards, the value shown.
        let cases: [(&str, u32, u32, Option<&str>, f32); 4] = [
            ("a game whose row is on", 769, 730, Some("cs2"), 2.0),
            (
                "a game whose row is off",
                730,
                1245620,
                Some("ELDEN RING"),
                1.0,
            ),
            ("a game with no row", 730, 570, None, 1.3),
            ("Steam's own screens", 730, 769, None, 1.3),
        ];
        for (what, before, app_id, profile, value) in cases {
            let (mut daemon, calls, _) = setup_with(four_case_profiles());
            let (class, pid) = game_mode_focus(before);
            daemon.react(&class, pid);
            let (class, pid) = game_mode_focus(app_id);
            daemon.react(&class, pid);
            assert_eq!(daemon.current.as_deref(), profile, "{what}");
            assert_eq!(
                calls.lock().unwrap().last(),
                Some(&Call::Apply("all", value)),
                "{what}"
            );
        }
    }

    #[test]
    fn game_mode_moves_between_the_four_cases() {
        let (mut daemon, calls, _) = setup_with(four_case_profiles());
        for app_id in [730, 1245620, 570, 1245620, 769] {
            let (class, pid) = game_mode_focus(app_id);
            daemon.react(&class, pid);
        }
        assert_eq!(
            *calls.lock().unwrap(),
            [
                Call::Apply("all", 2.0), // row on
                Call::Apply("all", 1.0), // row off: normal, not 1.3
                Call::Apply("all", 1.3), // no row: the general value
                Call::Apply("all", 1.0), // row off again
                Call::Apply("all", 1.3), // back at Steam's screens
            ]
        );
    }

    /// The same four cases from a desktop focus source, which reports a
    /// window class and a program name.
    #[test]
    fn a_desktop_shows_the_right_value_for_each_of_the_four_cases() {
        let mut profiles = profiles();
        profiles.default_saturation = 1.3;
        profiles.profiles.push(Profile {
            name: "off game".into(),
            enabled: false,
            saturation: 1.8,
            match_rule: MatchRule {
                exe: Some("offgame".into()),
                window_class: None,
                steam_app_id: None,
            },
            outputs: vec![],
        });
        // The window in front before, the focused window, the profile the
        // daemon holds afterwards, the value shown.
        let cases: [(&str, &str, &str, Option<&str>, f32); 4] = [
            (
                "a game whose profile is on",
                "plasmashell",
                "game.class",
                Some("game"),
                1.5,
            ),
            (
                "a game whose profile is off",
                "game.class",
                "offgame",
                Some("off game"),
                1.0,
            ),
            (
                "a game with no profile",
                "game.class",
                "other.game",
                None,
                1.3,
            ),
            ("the desktop itself", "game.class", "plasmashell", None, 1.3),
        ];
        for (what, before, class, profile, value) in cases {
            let (mut daemon, calls, _) = setup_with(profiles.clone());
            daemon.react(before, 0);
            daemon.react(class, 0);
            assert_eq!(daemon.current.as_deref(), profile, "{what}");
            assert_eq!(
                calls.lock().unwrap().last(),
                Some(&Call::Apply("all", value)),
                "{what}"
            );
        }
    }

    #[test]
    fn a_profile_that_is_on_wins_over_one_that_is_off_for_the_same_window() {
        // The off one is found by program name, the on one by window class.
        let mut profiles = profiles();
        profiles.profiles.insert(
            0,
            Profile {
                name: "old".into(),
                enabled: false,
                saturation: 1.8,
                match_rule: MatchRule {
                    exe: Some("game".into()),
                    window_class: None,
                    steam_app_id: None,
                },
                outputs: vec![],
            },
        );
        assert_eq!(
            match_focused(&profiles, "game.class", Some("game")).map(|p| p.name.as_str()),
            Some("game")
        );
        // With only the off one matching, it is the match.
        assert_eq!(
            match_focused(&profiles, "something.else", Some("game")).map(|p| p.name.as_str()),
            Some("old")
        );
    }

    #[test]
    fn a_native_steam_game_whose_only_profile_is_off_is_still_matched_by_app_id() {
        let mut by_app_id = steam_profiles();
        by_app_id.profiles[0].enabled = false;
        assert_eq!(
            class_to_match(&by_app_id, "cs2", Some("cs2"), Some(730)).as_deref(),
            Some("steam_app_730")
        );
        // An off profile found by program name gives way to an on one by AppID.
        by_app_id.profiles[0].enabled = true;
        by_app_id.profiles.push(Profile {
            name: "old".into(),
            enabled: false,
            saturation: 1.2,
            match_rule: MatchRule {
                exe: Some("cs2".into()),
                window_class: None,
                steam_app_id: None,
            },
            outputs: vec![],
        });
        assert_eq!(
            class_to_match(&by_app_id, "cs2", Some("cs2"), Some(730)).as_deref(),
            Some("steam_app_730")
        );
        // Both off: the one found directly stays.
        by_app_id.profiles[0].enabled = false;
        assert_eq!(
            class_to_match(&by_app_id, "cs2", Some("cs2"), Some(730)),
            None
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
        state.lock().unwrap().guard = Some(RestoreGuard::new(
            Box::new(backend),
            Levels::uniform(Saturation::IDENTITY),
        ));
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

    #[test]
    fn a_profile_added_for_the_focused_app_applies_on_reload() {
        let mut empty = profiles();
        empty.profiles.clear();
        let (mut daemon, calls, _) = setup_with(empty);
        daemon.react("game.class", 0);
        assert!(calls.lock().unwrap().is_empty());

        daemon.replace_profiles(profiles());

        assert_eq!(daemon.current.as_deref(), Some("game"));
        assert_eq!(*calls.lock().unwrap(), [Call::Apply("all", 1.5)]);
    }

    #[test]
    fn removing_the_focused_apps_profile_restores_the_default_on_reload() {
        let (mut daemon, calls, _) = setup();
        daemon.react("game.class", 0);

        let mut updated = daemon.profiles.clone();
        updated.profiles.clear();
        daemon.replace_profiles(updated);

        assert_eq!(daemon.current, None);
        assert_eq!(
            *calls.lock().unwrap(),
            [Call::Apply("all", 1.5), Call::Reset]
        );
    }

    #[test]
    fn reload_before_any_focus_applies_nothing() {
        let (mut daemon, calls, _) = setup();
        daemon.replace_profiles(profiles());
        assert_eq!(daemon.current, None);
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn reload_reapplies_the_active_profile_with_its_new_saturation() {
        let (mut daemon, calls, _) = setup();
        daemon.react("game.class", 0);

        let mut updated = daemon.profiles.clone();
        updated.profiles[0].saturation = 2.25;
        daemon.replace_profiles(updated);

        assert_eq!(daemon.current.as_deref(), Some("game"));
        assert_eq!(
            *calls.lock().unwrap(),
            [Call::Apply("all", 1.5), Call::Apply("all", 2.25)]
        );
    }

    #[test]
    fn a_profile_added_for_the_game_on_screen_in_game_mode_applies_on_reload() {
        let mut empty = steam_profiles();
        empty.profiles.clear();
        let (mut daemon, calls, _) = setup_with(empty);
        let (class, pid) = game_mode_focus(730);
        daemon.react(&class, pid);
        assert!(calls.lock().unwrap().is_empty());

        daemon.replace_profiles(steam_profiles());

        assert_eq!(daemon.current.as_deref(), Some("cs2"));
        assert_eq!(*calls.lock().unwrap(), [Call::Apply("all", 2.0)]);
    }

    #[test]
    fn reload_without_an_active_backend_applies_nothing() {
        let state = Arc::new(Mutex::new(BackendState::pending()));
        let mut daemon = Daemon::new(steam_profiles(), state.clone());
        let (class, pid) = game_mode_focus(730);
        daemon.react(&class, pid);

        daemon.replace_profiles(steam_profiles());

        assert_eq!(daemon.current, None);
        assert!(state.lock().unwrap().guard.is_none());
    }

    /// A backend that can show different outputs at once, recording every
    /// picture it is given and every reset.
    struct PerOutputFake {
        pictures: Arc<Mutex<Vec<Shown>>>,
    }

    #[derive(Debug, Clone, PartialEq)]
    enum Shown {
        Levels(Levels),
        Reset,
    }

    impl Backend for PerOutputFake {
        fn name(&self) -> &'static str {
            "per-output fake"
        }

        fn cost(&self) -> CostNote {
            CostNote::ZeroCost
        }

        fn outputs(&self) -> Vec<Output> {
            ["DP-1", "HDMI-A-1"]
                .map(|id| Output {
                    id: id.into(),
                    human_name: id.into(),
                })
                .to_vec()
        }

        fn apply(&mut self, _output: &Output, _saturation: Saturation) -> Result<(), BackendError> {
            panic!("a per-output backend is driven through apply_levels");
        }

        fn reset(&mut self, _output: &Output) -> Result<(), BackendError> {
            self.pictures.lock().unwrap().push(Shown::Reset);
            Ok(())
        }

        fn per_output(&self) -> bool {
            true
        }

        fn apply_levels(&mut self, levels: &Levels) -> Result<(), BackendError> {
            self.pictures
                .lock()
                .unwrap()
                .push(Shown::Levels(levels.clone()));
            Ok(())
        }
    }

    fn per_output_setup(profiles: Profiles) -> (Daemon, Arc<Mutex<Vec<Shown>>>) {
        let pictures = Arc::new(Mutex::new(Vec::new()));
        let backend = PerOutputFake {
            pictures: pictures.clone(),
        };
        (Daemon::with_profiles(profiles, Box::new(backend)), pictures)
    }

    fn shown(base: f32, outputs: &[(&str, f32)]) -> Shown {
        Shown::Levels(Levels::new(
            Saturation::new(base),
            outputs
                .iter()
                .map(|(name, value)| (name.to_string(), Saturation::new(*value))),
        ))
    }

    #[test]
    fn a_profile_with_fixed_outputs_changes_only_those() {
        let mut profiles = profiles();
        profiles.profiles[0].outputs = vec!["HDMI-A-1".into()];
        let (mut daemon, pictures) = per_output_setup(profiles);

        daemon.react("game.class", 0);
        daemon.react("desktop", 0);

        assert_eq!(
            *pictures.lock().unwrap(),
            [shown(1.0, &[("HDMI-A-1", 1.5)]), Shown::Reset]
        );
    }

    /// The off-means-normal rule on a backend with a value per output: the
    /// desktop is 1.3 with HDMI-A-1 at 0.85, and the off profile is normal on
    /// the outputs it would change when on.
    #[test]
    fn a_profile_that_is_off_is_normal_on_its_own_outputs_only() {
        // `profile_outputs`, the profile's `outputs`, focused output, shown.
        let rows: [(ProfileOutputs, &[&str], Option<&str>, Shown); 4] = [
            (ProfileOutputs::All, &[], Some("DP-1"), shown(1.0, &[])),
            (
                ProfileOutputs::All,
                &["HDMI-A-1"],
                Some("DP-1"),
                shown(1.3, &[("HDMI-A-1", 1.0)]),
            ),
            (
                ProfileOutputs::Focused,
                &[],
                Some("DP-1"),
                shown(1.3, &[("DP-1", 1.0), ("HDMI-A-1", 0.85)]),
            ),
            (
                ProfileOutputs::Focused,
                &[],
                Some("HDMI-A-1"),
                shown(1.3, &[("HDMI-A-1", 1.0)]),
            ),
        ];
        for (follow, outputs, focused, expected) in rows {
            let mut profiles = profiles();
            profiles.default_saturation = 1.3;
            profiles.profile_outputs = follow;
            profiles.outputs.insert(
                "HDMI-A-1".into(),
                OutputConfig {
                    default_saturation: Some(0.85),
                },
            );
            profiles.profiles[0].enabled = false;
            profiles.profiles[0].outputs = outputs.iter().map(|name| name.to_string()).collect();
            let (mut daemon, pictures) = per_output_setup(profiles);

            daemon.react_on("game.class", 0, focused);

            assert_eq!(daemon.current.as_deref(), Some("game"));
            assert_eq!(
                pictures.lock().unwrap().last(),
                Some(&expected),
                "{follow:?}, outputs {outputs:?}, focused {focused:?}"
            );
        }
    }

    #[test]
    fn the_all_keyword_changes_every_output() {
        let mut profiles = profiles();
        profiles.profiles[0].outputs = vec!["all".into(), "HDMI-A-1".into()];
        let (mut daemon, pictures) = per_output_setup(profiles);

        daemon.react("game.class", 0);

        assert_eq!(*pictures.lock().unwrap(), [shown(1.5, &[])]);
    }

    #[test]
    fn focused_with_no_output_reported_changes_every_output_and_says_so() {
        let mut profiles = profiles();
        profiles.profile_outputs = ProfileOutputs::Focused;
        let (mut daemon, pictures) = per_output_setup(profiles);

        daemon.react("game.class", 0);
        daemon.react("game.class", 0);

        assert_eq!(*pictures.lock().unwrap(), [shown(1.5, &[])]);
        assert_eq!(
            daemon.logged_notes,
            [Note::FocusedOutputUnknown {
                profile: "game".into()
            }]
        );
    }

    #[test]
    fn leaving_the_game_restores_each_outputs_own_default() {
        let mut profiles = profiles();
        profiles.outputs.insert(
            "HDMI-A-1".into(),
            OutputConfig {
                default_saturation: Some(0.85),
            },
        );
        let (mut daemon, pictures) = per_output_setup(profiles);

        daemon.react("game.class", 0);
        daemon.react("desktop", 0);

        assert_eq!(
            *pictures.lock().unwrap(),
            [shown(1.5, &[]), shown(1.0, &[("HDMI-A-1", 0.85)])]
        );
    }

    #[test]
    fn a_reload_while_the_game_is_focused_applies_its_new_outputs() {
        let (mut daemon, pictures) = per_output_setup(profiles());
        daemon.react("game.class", 0);

        let mut updated = daemon.profiles.clone();
        updated.profiles[0].outputs = vec!["DP-1".into()];
        daemon.replace_profiles(updated);

        assert_eq!(
            *pictures.lock().unwrap(),
            [shown(1.5, &[]), shown(1.0, &[("DP-1", 1.5)])]
        );
        assert_eq!(daemon.current.as_deref(), Some("game"));
    }

    #[test]
    fn per_output_defaults_on_a_uniform_backend_are_reported_once() {
        let mut profiles = profiles();
        profiles.outputs.insert(
            "HDMI-A-1".into(),
            OutputConfig {
                default_saturation: Some(0.85),
            },
        );
        let (mut daemon, calls, _) = setup_with(profiles);

        daemon.react("game.class", 0);
        daemon.react("desktop", 0);
        daemon.react("game.class", 0);

        assert_eq!(
            *calls.lock().unwrap(),
            [
                Call::Apply("all", 1.5),
                Call::Reset,
                Call::Apply("all", 1.5)
            ]
        );
        assert_eq!(daemon.logged_notes, [Note::PerOutputDefaultsNotUsed]);
    }

    #[test]
    fn a_named_output_that_is_not_connected_is_skipped_and_reported() {
        let mut profiles = profiles();
        profiles.profiles[0].outputs = vec!["DP-9".into(), "HDMI-A-1".into()];
        let (mut daemon, pictures) = per_output_setup(profiles);

        daemon.react("game.class", 0);

        assert_eq!(
            *pictures.lock().unwrap(),
            [shown(1.0, &[("DP-9", 1.5), ("HDMI-A-1", 1.5)])]
        );
        assert_eq!(
            daemon.logged_notes,
            [Note::OutputNotConnected {
                profile: "game".into(),
                output: "DP-9".into()
            }]
        );
    }

    fn following_focus() -> Profiles {
        Profiles {
            profile_outputs: ProfileOutputs::Focused,
            ..profiles()
        }
    }

    #[test]
    fn a_profile_that_follows_focus_changes_only_the_games_output() {
        let (mut daemon, pictures) = per_output_setup(following_focus());

        assert!(daemon.window_activated_on(
            "game.class".into(),
            "Game".into(),
            "0".into(),
            "HDMI-A-1".into()
        ));

        assert_eq!(
            *pictures.lock().unwrap(),
            [shown(1.0, &[("HDMI-A-1", 1.5)])]
        );
        assert_eq!(daemon.active_output(), "HDMI-A-1");
        assert!(daemon.logged_notes.is_empty());
    }

    #[test]
    fn moving_the_game_to_another_output_moves_the_change() {
        let (mut daemon, pictures) = per_output_setup(following_focus());

        daemon.react_on("game.class", 0, Some("HDMI-A-1"));
        daemon.react_on("game.class", 0, Some("DP-1"));
        // The same report again changes nothing.
        daemon.react_on("game.class", 0, Some("DP-1"));

        assert_eq!(
            *pictures.lock().unwrap(),
            [
                shown(1.0, &[("HDMI-A-1", 1.5)]),
                shown(1.0, &[("DP-1", 1.5)])
            ]
        );
        assert_eq!(daemon.active_output(), "DP-1");
    }

    #[test]
    fn a_profile_with_outputs_of_its_own_ignores_moves() {
        let mut profiles = following_focus();
        profiles.profiles[0].outputs = vec!["DP-1".into()];
        let (mut daemon, pictures) = per_output_setup(profiles);

        daemon.react_on("game.class", 0, Some("HDMI-A-1"));
        daemon.react_on("game.class", 0, Some("DP-1"));

        assert_eq!(*pictures.lock().unwrap(), [shown(1.0, &[("DP-1", 1.5)])]);
        assert_eq!(daemon.active_output(), "");
    }

    /// An older focus script still calls WindowActivatedPid: no output is
    /// known, so a profile that follows focus changes every output and the
    /// user is told why.
    #[test]
    fn the_old_method_still_works_without_an_output() {
        let (mut daemon, pictures) = per_output_setup(following_focus());

        daemon.window_activated_pid("game.class".into(), "Game".into(), "0".into());

        assert_eq!(*pictures.lock().unwrap(), [shown(1.5, &[])]);
        assert_eq!(
            daemon.logged_notes,
            [Note::FocusedOutputUnknown {
                profile: "game".into()
            }]
        );
        assert_eq!(daemon.current.as_deref(), Some("game"));
    }

    #[test]
    fn an_output_reported_on_a_uniform_backend_changes_every_output_once() {
        let (mut daemon, calls, _) = setup_with(following_focus());

        daemon.window_activated_on(
            "game.class".into(),
            "".into(),
            "0".into(),
            "HDMI-A-1".into(),
        );
        daemon.window_activated_on("game.class".into(), "".into(), "0".into(), "DP-1".into());

        assert_eq!(*calls.lock().unwrap(), [Call::Apply("all", 1.5)]);
        assert_eq!(
            daemon.logged_notes,
            [Note::ProfileChangesEveryOutput {
                profile: "game".into()
            }]
        );
        assert_eq!(daemon.active_output(), "");
    }

    #[test]
    fn a_reload_keeps_the_games_output() {
        let (mut daemon, pictures) = per_output_setup(following_focus());
        daemon.react_on("game.class", 0, Some("HDMI-A-1"));

        let mut updated = daemon.profiles.clone();
        updated.profiles[0].saturation = 2.0;
        daemon.replace_profiles(updated);

        assert_eq!(
            *pictures.lock().unwrap(),
            [
                shown(1.0, &[("HDMI-A-1", 1.5)]),
                shown(1.0, &[("HDMI-A-1", 2.0)])
            ]
        );
    }

    #[test]
    fn a_reload_shows_the_desktop_only_when_it_changed_and_no_profile_is_on() {
        let one = Levels::uniform(Saturation::IDENTITY);
        let other = Levels::new(
            Saturation::IDENTITY,
            [("HDMI-A-1".to_string(), Saturation::new(0.85))],
        );
        for (profile_active, old, new, expected) in [
            (false, &one, &other, true),
            (false, &other, &one, true),
            (false, &one, &one, false),
            (true, &one, &other, false),
            (true, &one, &one, false),
        ] {
            assert_eq!(
                reload_shows_desktop(profile_active, old, new),
                expected,
                "profile active {profile_active}, {old:?} -> {new:?}"
            );
        }
    }

    fn with_output_default(mut profiles: Profiles, name: &str, value: f32) -> Profiles {
        profiles.outputs.insert(
            name.into(),
            OutputConfig {
                default_saturation: Some(value),
            },
        );
        profiles
    }

    #[test]
    fn a_new_output_default_is_shown_when_no_profile_is_on() {
        let (mut daemon, pictures) = per_output_setup(profiles());
        daemon.react("desktop", 0);

        daemon.replace_profiles(with_output_default(profiles(), "HDMI-A-1", 0.85));
        // The same file again shows nothing new.
        daemon.replace_profiles(with_output_default(profiles(), "HDMI-A-1", 0.85));

        assert_eq!(
            *pictures.lock().unwrap(),
            [shown(1.0, &[("HDMI-A-1", 0.85)])]
        );
    }

    #[test]
    fn unsetting_an_output_default_puts_it_back_to_the_base() {
        let (mut daemon, pictures) =
            per_output_setup(with_output_default(profiles(), "HDMI-A-1", 0.85));
        daemon.react("desktop", 0);

        daemon.replace_profiles(profiles());

        assert_eq!(*pictures.lock().unwrap(), [Shown::Reset]);
    }

    /// v0.4.1 applied nothing here until a profile was applied and restored;
    /// only the desktop app's slider changed the screen, by itself.
    #[test]
    fn a_new_top_level_default_is_shown_when_no_profile_is_on() {
        let (mut daemon, calls, _) = setup();
        daemon.react("desktop", 0);

        let mut updated = profiles();
        updated.default_saturation = 1.25;
        daemon.replace_profiles(updated);

        assert_eq!(*calls.lock().unwrap(), [Call::Apply("all", 1.25)]);
    }

    #[test]
    fn an_unrelated_edit_with_no_profile_on_changes_nothing() {
        let (mut daemon, calls, _) = setup();
        daemon.react("desktop", 0);

        let mut updated = profiles();
        updated.profiles[0].saturation = 2.0;
        updated.profiles.push(Profile {
            name: "other".into(),
            enabled: true,
            saturation: 1.8,
            match_rule: MatchRule {
                exe: Some("other-game".into()),
                ..MatchRule::default()
            },
            outputs: vec![],
        });
        daemon.replace_profiles(updated);

        assert!(calls.lock().unwrap().is_empty());
    }

    /// With a profile on, the new desktop is the restore target; the profile's
    /// own picture is applied again on reload, as always, and its untouched
    /// outputs show the new desktop value at once, because the resolver builds
    /// a limited profile picture on top of the desktop picture.
    #[test]
    fn a_desktop_change_with_a_profile_on_shows_on_its_untouched_outputs() {
        let base = Profiles {
            profile_outputs: ProfileOutputs::Focused,
            ..profiles()
        };
        let (mut daemon, pictures) = per_output_setup(base.clone());
        daemon.react_on("game.class", 0, Some("HDMI-A-1"));

        daemon.replace_profiles(with_output_default(base, "DP-1", 0.9));
        daemon.react_on("desktop", 0, Some("DP-1"));

        assert_eq!(
            *pictures.lock().unwrap(),
            [
                shown(1.0, &[("HDMI-A-1", 1.5)]),
                shown(1.0, &[("DP-1", 0.9), ("HDMI-A-1", 1.5)]),
                shown(1.0, &[("DP-1", 0.9)]),
            ]
        );
    }

    #[test]
    fn a_failed_desktop_change_is_tried_again_at_the_next_focus() {
        let (mut daemon, calls, fail_apply) = setup();
        daemon.react("desktop", 0);

        fail_apply.store(true, Ordering::Release);
        let mut updated = profiles();
        updated.default_saturation = 1.25;
        daemon.replace_profiles(updated);
        fail_apply.store(false, Ordering::Release);
        daemon.react("desktop", 0);
        // Applied once it worked; nothing more after that.
        daemon.react("desktop", 0);

        assert_eq!(
            *calls.lock().unwrap(),
            [Call::Apply("all", 1.25), Call::Apply("all", 1.25)]
        );
    }

    #[test]
    fn a_new_backend_shows_the_desktop_only_when_it_is_changed_and_no_profile_is_on() {
        let unchanged = Levels::uniform(Saturation::IDENTITY);
        let boosted = Levels::uniform(Saturation::new(1.5));
        let one_output = Levels::new(
            Saturation::IDENTITY,
            [("HDMI-A-1".to_string(), Saturation::new(0.85))],
        );
        for (backend, profile_active, desktop, expected) in [
            ("kwin", false, &boosted, true),
            ("kwin", false, &one_output, true),
            ("kwin", false, &unchanged, false),
            ("kwin", true, &boosted, false),
            ("gamescope", false, &boosted, false),
        ] {
            assert_eq!(
                shows_desktop_at_detection(backend, profile_active, desktop),
                expected,
                "{backend}, profile active {profile_active}, {desktop:?}"
            );
        }
    }

    /// A backend that has just been found, as at daemon start.
    fn fresh(daemon: &Daemon) {
        daemon.backend.lock().unwrap().just_detected = true;
    }

    #[test]
    fn a_changed_default_is_shown_when_the_daemon_starts() {
        let mut profiles = profiles();
        profiles.default_saturation = 1.5;
        let (mut daemon, calls, _) = setup_with(profiles);
        fresh(&daemon);

        daemon.show_desktop_at_start();
        // Once only.
        daemon.show_desktop_at_start();

        assert_eq!(*calls.lock().unwrap(), [Call::Apply("all", 1.5)]);
    }

    #[test]
    fn the_default_file_applies_nothing_when_the_daemon_starts() {
        let (mut daemon, calls, _) = setup();
        fresh(&daemon);

        daemon.show_desktop_at_start();
        daemon.react("desktop", 0);

        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn an_output_default_is_shown_when_the_daemon_starts() {
        let (mut daemon, pictures) =
            per_output_setup(with_output_default(profiles(), "HDMI-A-1", 0.85));
        fresh(&daemon);

        daemon.show_desktop_at_start();

        assert_eq!(
            *pictures.lock().unwrap(),
            [shown(1.0, &[("HDMI-A-1", 0.85)])]
        );
    }

    #[test]
    fn a_backend_found_at_a_focus_report_with_no_profile_shows_the_desktop() {
        let mut profiles = profiles();
        profiles.default_saturation = 1.5;
        let (mut daemon, calls, _) = setup_with(profiles);
        fresh(&daemon);

        daemon.react("desktop", 0);
        daemon.react("desktop", 0);

        assert_eq!(*calls.lock().unwrap(), [Call::Apply("all", 1.5)]);
    }

    #[test]
    fn a_profile_at_the_first_focus_report_wins_over_the_desktop() {
        let mut profiles = profiles();
        profiles.default_saturation = 1.25;
        let (mut daemon, calls, _) = setup_with(profiles);
        fresh(&daemon);

        daemon.react("game.class", 0);
        daemon.react("desktop", 0);

        assert_eq!(
            *calls.lock().unwrap(),
            [Call::Apply("all", 1.5), Call::Apply("all", 1.25)]
        );
    }

    #[test]
    fn a_failed_desktop_at_start_is_tried_again_at_the_next_focus() {
        let mut profiles = profiles();
        profiles.default_saturation = 1.5;
        let (mut daemon, calls, fail_apply) = setup_with(profiles);
        fresh(&daemon);

        fail_apply.store(true, Ordering::Release);
        daemon.show_desktop_at_start();
        fail_apply.store(false, Ordering::Release);
        daemon.react("desktop", 0);
        daemon.react("desktop", 0);

        assert_eq!(
            *calls.lock().unwrap(),
            [Call::Apply("all", 1.5), Call::Apply("all", 1.5)]
        );
    }

    #[test]
    fn gamescope_keeps_its_own_first_sync() {
        let mut profiles = steam_profiles();
        profiles.default_saturation = 1.25;
        let calls = Arc::new(Mutex::new(Vec::new()));
        let backend = FakeBackend {
            name: "gamescope",
            calls: calls.clone(),
            fail_apply: Arc::new(AtomicBool::new(false)),
        };
        let mut daemon = Daemon::with_profiles(profiles, Box::new(backend));
        fresh(&daemon);

        daemon.show_desktop_at_start();
        assert!(calls.lock().unwrap().is_empty());

        let (class, pid) = game_mode_focus(769);
        assert_eq!(daemon.sync_gamescope(&class, pid), GamescopeSync::Done);
        assert_eq!(*calls.lock().unwrap(), [Call::Apply("all", 1.25)]);
    }

    /// A uniform backend that, like KWin with an older effect still loaded,
    /// says why it shows one value on every output.
    struct WithReason(FakeBackend);

    impl Backend for WithReason {
        fn name(&self) -> &'static str {
            "kwin"
        }
        fn cost(&self) -> CostNote {
            CostNote::CompositorShaderPass
        }
        fn outputs(&self) -> Vec<Output> {
            self.0.outputs()
        }
        fn apply(&mut self, output: &Output, saturation: Saturation) -> Result<(), BackendError> {
            self.0.apply(output, saturation)
        }
        fn reset(&mut self, output: &Output) -> Result<(), BackendError> {
            self.0.reset(output)
        }
        fn per_output_reason(&self) -> Option<String> {
            Some("KDE is still running the Satur8 effect from before the upgrade.".into())
        }
    }

    #[test]
    fn the_fallback_note_carries_the_backends_reason_once() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let backend = WithReason(FakeBackend {
            name: "kwin",
            calls: calls.clone(),
            fail_apply: Arc::new(AtomicBool::new(false)),
        });
        let mut daemon = Daemon::with_profiles(following_focus(), Box::new(backend));

        daemon.react_on("game.class", 0, Some("HDMI-A-1"));
        daemon.react_on("desktop", 0, None);
        daemon.react_on("game.class", 0, Some("HDMI-A-1"));

        assert_eq!(
            daemon.logged_notes,
            [
                Note::ProfileChangesEveryOutput {
                    profile: "game".into()
                },
                Note::OneValueBecause {
                    reason: "KDE is still running the Satur8 effect from before the upgrade."
                        .into()
                }
            ]
        );
    }

    #[test]
    fn no_reason_is_logged_when_nothing_asks_for_per_output() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let backend = WithReason(FakeBackend {
            name: "kwin",
            calls: calls.clone(),
            fail_apply: Arc::new(AtomicBool::new(false)),
        });
        let mut daemon = Daemon::with_profiles(profiles(), Box::new(backend));

        daemon.react("game.class", 0);
        daemon.react("desktop", 0);

        assert!(daemon.logged_notes.is_empty());
        assert_eq!(
            *calls.lock().unwrap(),
            [Call::Apply("all", 1.5), Call::Reset]
        );
    }
}
