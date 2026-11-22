//! Shared backend selection and temporary-application lifecycle.

use std::io;
use std::thread::{self, JoinHandle};

use anyhow::{anyhow, bail, Result};
use satur8_core::{Backend, BackendError, Environment, Levels};
use satur8_drm_ctm::DrmCtmBackend;
use satur8_gamescope::GamescopeBackend;
use satur8_gnome::GnomeBackend;
use satur8_hyprland::HyprlandBackend;
use satur8_kwin::KwinBackend;
use satur8_nv_control::NvControlBackend;
use signal_hook::iterator::{Handle, Signals};

/// The GNOME extension's names, for reports that check it without the backend.
pub use satur8_gnome::{
    EXTENSION_UUID as GNOME_EXTENSION_UUID, IFACE as GNOME_IFACE, PATH as GNOME_PATH,
    SERVICE as GNOME_SERVICE,
};

/// Sentinel used when an operation targets every output.
pub use satur8_core::all_outputs;

/// One step of backend selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Probe {
    Kwin,
    Gnome,
    Hyprland,
    NvControl,
    /// KWin is running but rejects our effect: that is the exact-version plugin
    /// mismatch after a KWin update, not a missing backend. Say so, instead of
    /// pointing a KDE user at another backend.
    KwinRejectsEffect,
    /// Steam Game Mode. After every desktop compositor, so desktops behave as
    /// before; before DRM CTM, which cannot work while gamescope owns the display.
    Gamescope,
    DrmCtm,
}

/// The order `select_backend` tries backends in.
const SELECTION_ORDER: [Probe; 7] = [
    Probe::Kwin,
    Probe::Gnome,
    Probe::Hyprland,
    Probe::NvControl,
    Probe::KwinRejectsEffect,
    Probe::Gamescope,
    Probe::DrmCtm,
];

enum ProbeResult {
    Found(Box<dyn Backend>),
    /// Stop selecting and report this instead.
    Refused(anyhow::Error),
    Absent,
}

fn found<B: Backend + 'static>(backend: Option<B>) -> ProbeResult {
    match backend {
        Some(backend) => ProbeResult::Found(Box::new(backend)),
        None => ProbeResult::Absent,
    }
}

fn run_probe(probe: Probe) -> ProbeResult {
    match probe {
        Probe::Kwin => found(KwinBackend::detect()),
        Probe::Gnome => found(GnomeBackend::detect()),
        Probe::Hyprland => found(HyprlandBackend::detect()),
        Probe::NvControl => found(NvControlBackend::detect()),
        Probe::KwinRejectsEffect => {
            if KwinBackend::connect().is_some() {
                ProbeResult::Refused(anyhow!(
                    "the Satur8 KWin effect is missing or was built for a different KWin \
                     version than the one running; {}.",
                    satur8_kwin::repair::advice_here()
                ))
            } else {
                ProbeResult::Absent
            }
        }
        Probe::Gamescope => found(GamescopeBackend::detect()),
        Probe::DrmCtm => found(DrmCtmBackend::detect()),
    }
}

/// Run `probes` in order and stop at the first backend found or refusal.
fn select_in_order(
    probes: &[Probe],
    mut run: impl FnMut(Probe) -> ProbeResult,
) -> Result<Option<Box<dyn Backend>>> {
    for &probe in probes {
        match run(probe) {
            ProbeResult::Found(backend) => return Ok(Some(backend)),
            ProbeResult::Refused(error) => return Err(error),
            ProbeResult::Absent => {}
        }
    }
    Ok(None)
}

/// Whether any probe ahead of gamescope in `probes` finds or refuses.
fn desktop_present_in_order(probes: &[Probe], mut run: impl FnMut(Probe) -> ProbeResult) -> bool {
    probes
        .iter()
        .take_while(|&&probe| probe != Probe::Gamescope)
        .any(|&probe| !matches!(run(probe), ProbeResult::Absent))
}

/// Whether selection would stop at a desktop backend before reaching
/// gamescope: KWin (also when it rejects the effect), GNOME, Hyprland or
/// NV-CONTROL. Reports use this to pick the desktop or the Game Mode view.
pub fn desktop_backend_present() -> bool {
    desktop_present_in_order(&SELECTION_ORDER, run_probe)
}

/// Resolve the best reachable apply/reset backend in canonical cost order.
///
/// The nested gamescope launch and the `gamescope-native` override are
/// launch-only and are not part of this selector; Steam Game Mode is.
pub fn select_backend() -> Result<Box<dyn Backend>> {
    if let Some(backend) = select_in_order(&SELECTION_ORDER, run_probe)? {
        return Ok(backend);
    }

    let environment = Environment::detect();
    bail!(
        "no usable apply/reset backend for this session ({}, {}, {}).\n\
         Preferred here is '{}'. On a niche wlroots compositor with no native \
         hook, use the gamescope fallback: `satur8 run --via gamescope -- <game>`.",
        environment.session,
        environment.desktop,
        environment.gpu,
        environment.preferred_backend()
    )
}

/// Owns a backend and restores its configured desktop state when dropped.
pub struct RestoreGuard {
    backend: Box<dyn Backend>,
    /// The desktop picture to put back.
    restore_to: Levels,
    armed: bool,
}

impl RestoreGuard {
    pub fn new(backend: Box<dyn Backend>, restore_to: Levels) -> Self {
        Self {
            backend,
            restore_to,
            armed: true,
        }
    }

    pub fn inactive(backend: Box<dyn Backend>, restore_to: Levels) -> Self {
        Self {
            backend,
            restore_to,
            armed: false,
        }
    }

    pub fn backend(&self) -> &dyn Backend {
        self.backend.as_ref()
    }

    pub fn backend_mut(&mut self) -> &mut dyn Backend {
        self.backend.as_mut()
    }

    pub fn set_restore_to(&mut self, levels: Levels) {
        self.restore_to = levels;
    }

    pub fn arm(&mut self) {
        self.armed = true;
    }

    pub fn disarm(&mut self) {
        self.armed = false;
    }

    pub fn restore_now(&mut self) -> Result<(), BackendError> {
        let result = restore_backend(self.backend.as_mut(), &self.restore_to);
        if result.is_ok() {
            self.armed = false;
        }
        result
    }
}

impl Drop for RestoreGuard {
    fn drop(&mut self) {
        if self.armed {
            if let Err(error) = restore_backend(self.backend.as_mut(), &self.restore_to) {
                eprintln!("satur8: warning, failed to restore saturation: {error}");
            }
        }
    }
}

/// An unchanged desktop is restored with `reset`, which lets backends release
/// their per-frame cost; any other desktop is shown with `apply_levels`.
fn restore_backend(backend: &mut dyn Backend, restore_to: &Levels) -> Result<(), BackendError> {
    if restore_to.is_identity() {
        backend.reset(&all_outputs())
    } else {
        backend.apply_levels(restore_to)
    }
}

/// A signal-listener thread that closes and joins on drop.
pub struct SignalHandler {
    handle: Handle,
    thread: Option<JoinHandle<()>>,
}

impl Drop for SignalHandler {
    fn drop(&mut self) {
        self.handle.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Install one callback for SIGINT, SIGTERM, and SIGHUP.
pub fn install_signal_handler<F>(callback: F) -> io::Result<SignalHandler>
where
    F: Fn(i32) + Send + 'static,
{
    let mut signals = Signals::new([
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
    ])?;
    let handle = signals.handle();
    let thread = thread::spawn(move || {
        for signal in signals.forever() {
            callback(signal);
        }
    });
    Ok(SignalHandler {
        handle,
        thread: Some(thread),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use satur8_core::{CostNote, Output, Saturation};

    /// Records every `apply` and `reset` as "apply all 1.25" or "reset all".
    struct Recording(Arc<Mutex<Vec<String>>>);

    impl Backend for Recording {
        fn name(&self) -> &'static str {
            "recording"
        }

        fn cost(&self) -> CostNote {
            CostNote::ZeroCost
        }

        fn outputs(&self) -> Vec<Output> {
            vec![all_outputs()]
        }

        fn apply(&mut self, output: &Output, saturation: Saturation) -> Result<(), BackendError> {
            self.0
                .lock()
                .unwrap()
                .push(format!("apply {} {}", output.id, saturation.get()));
            Ok(())
        }

        fn reset(&mut self, output: &Output) -> Result<(), BackendError> {
            self.0.lock().unwrap().push(format!("reset {}", output.id));
            Ok(())
        }
    }

    fn restored_calls(desktop: Levels) -> Vec<String> {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut guard = RestoreGuard::new(Box::new(Recording(calls.clone())), desktop);
        guard.restore_now().unwrap();
        drop(guard);
        let calls = calls.lock().unwrap().clone();
        calls
    }

    /// The same calls as v0.4.1: an unchanged desktop is a reset, any other
    /// desktop one apply on every output, and nothing more when dropped.
    #[test]
    fn restore_guard_resets_an_unchanged_desktop_and_applies_any_other() {
        assert_eq!(
            restored_calls(Levels::uniform(Saturation::IDENTITY)),
            ["reset all"]
        );
        assert_eq!(
            restored_calls(Levels::uniform(Saturation::new(1.25))),
            ["apply all 1.25"]
        );
    }

    struct Named(&'static str);

    impl Backend for Named {
        fn name(&self) -> &'static str {
            self.0
        }

        fn cost(&self) -> CostNote {
            CostNote::ZeroCost
        }

        fn outputs(&self) -> Vec<Output> {
            vec![all_outputs()]
        }

        fn apply(&mut self, _output: &Output, _saturation: Saturation) -> Result<(), BackendError> {
            Ok(())
        }

        fn reset(&mut self, _output: &Output) -> Result<(), BackendError> {
            Ok(())
        }
    }

    fn name_of(probe: Probe) -> &'static str {
        match probe {
            Probe::Kwin => "kwin",
            Probe::Gnome => "gnome-shell",
            Probe::Hyprland => "hyprland",
            Probe::NvControl => "nv-control",
            Probe::KwinRejectsEffect => "kwin-rejects-effect",
            Probe::Gamescope => "gamescope",
            Probe::DrmCtm => "drm-ctm",
        }
    }

    #[test]
    fn gamescope_comes_after_the_desktops_and_the_kwin_bail_and_before_drm_ctm() {
        assert_eq!(
            SELECTION_ORDER,
            [
                Probe::Kwin,
                Probe::Gnome,
                Probe::Hyprland,
                Probe::NvControl,
                Probe::KwinRejectsEffect,
                Probe::Gamescope,
                Probe::DrmCtm,
            ]
        );
    }

    #[test]
    fn the_first_backend_found_wins_and_later_probes_never_run() {
        let mut ran = Vec::new();
        let picked = select_in_order(&SELECTION_ORDER, |probe| {
            ran.push(probe);
            match probe {
                Probe::Gamescope | Probe::DrmCtm => {
                    ProbeResult::Found(Box::new(Named(name_of(probe))))
                }
                _ => ProbeResult::Absent,
            }
        })
        .unwrap()
        .unwrap();
        assert_eq!(picked.name(), "gamescope");
        assert_eq!(ran.last(), Some(&Probe::Gamescope));
        assert!(!ran.contains(&Probe::DrmCtm));
    }

    #[test]
    fn a_desktop_backend_is_picked_before_gamescope() {
        let picked = select_in_order(&SELECTION_ORDER, |probe| match probe {
            Probe::Kwin | Probe::Gamescope => ProbeResult::Found(Box::new(Named(name_of(probe)))),
            _ => ProbeResult::Absent,
        })
        .unwrap()
        .unwrap();
        assert_eq!(picked.name(), "kwin");
    }

    #[test]
    fn kwin_rejecting_the_effect_stops_selection_before_gamescope() {
        let mut ran = Vec::new();
        let result = select_in_order(&SELECTION_ORDER, |probe| {
            ran.push(probe);
            match probe {
                Probe::KwinRejectsEffect => ProbeResult::Refused(anyhow!("rejected")),
                Probe::Gamescope => ProbeResult::Found(Box::new(Named(name_of(probe)))),
                _ => ProbeResult::Absent,
            }
        });
        assert!(result.is_err());
        assert!(!ran.contains(&Probe::Gamescope));
    }

    #[test]
    fn a_desktop_backend_ahead_of_gamescope_is_present() {
        for desktop in [Probe::Kwin, Probe::Gnome, Probe::Hyprland, Probe::NvControl] {
            assert!(desktop_present_in_order(&SELECTION_ORDER, |probe| {
                if probe == desktop {
                    ProbeResult::Found(Box::new(Named(name_of(probe))))
                } else {
                    ProbeResult::Absent
                }
            }));
        }
    }

    #[test]
    fn kwin_rejecting_the_effect_counts_as_a_desktop() {
        assert!(desktop_present_in_order(&SELECTION_ORDER, |probe| {
            match probe {
                Probe::KwinRejectsEffect => ProbeResult::Refused(anyhow!("rejected")),
                _ => ProbeResult::Absent,
            }
        }));
    }

    #[test]
    fn gamescope_and_drm_ctm_are_not_desktops_and_are_never_probed() {
        let mut ran = Vec::new();
        let present = desktop_present_in_order(&SELECTION_ORDER, |probe| {
            ran.push(probe);
            match probe {
                Probe::Gamescope | Probe::DrmCtm => {
                    ProbeResult::Found(Box::new(Named(name_of(probe))))
                }
                _ => ProbeResult::Absent,
            }
        });
        assert!(!present);
        assert!(!ran.contains(&Probe::Gamescope));
        assert!(!ran.contains(&Probe::DrmCtm));
    }

    #[test]
    fn nothing_found_is_none() {
        assert!(select_in_order(&SELECTION_ORDER, |_| ProbeResult::Absent)
            .unwrap()
            .is_none());
    }
}
