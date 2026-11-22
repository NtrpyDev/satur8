//! Shared backend selection and temporary-application lifecycle.

use std::io;
use std::thread::{self, JoinHandle};

use anyhow::{anyhow, bail, Result};
use satur8_core::{Backend, BackendError, Environment, Output, Saturation};
use satur8_drm_ctm::DrmCtmBackend;
use satur8_gamescope::GamescopeBackend;
use satur8_gnome::GnomeBackend;
use satur8_hyprland::HyprlandBackend;
use satur8_kwin::KwinBackend;
use satur8_nv_control::NvControlBackend;
use signal_hook::iterator::{Handle, Signals};

/// Sentinel used when an operation targets every output.
pub fn all_outputs() -> Output {
    Output {
        id: "all".into(),
        human_name: "All outputs".into(),
    }
}

/// Apply saturation to configured output ids, or every output when none are set.
pub fn apply_to_outputs(
    backend: &mut dyn Backend,
    outputs: &[String],
    saturation: Saturation,
) -> Result<(), BackendError> {
    if outputs.is_empty() {
        return backend.apply(&all_outputs(), saturation);
    }
    for id in outputs {
        let output = Output {
            id: id.clone(),
            human_name: id.clone(),
        };
        backend.apply(&output, saturation).map_err(|error| {
            BackendError::Apply(format!("backend rejected profile output '{id}': {error}"))
        })?;
    }
    Ok(())
}

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
                     version than the one running. Run `satur8 repair` to rebuild it."
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
    restore_to: Saturation,
    armed: bool,
}

impl RestoreGuard {
    pub fn new(backend: Box<dyn Backend>, restore_to: Saturation) -> Self {
        Self {
            backend,
            restore_to,
            armed: true,
        }
    }

    pub fn inactive(backend: Box<dyn Backend>, restore_to: Saturation) -> Self {
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

    pub fn set_restore_to(&mut self, saturation: Saturation) {
        self.restore_to = saturation;
    }

    pub fn arm(&mut self) {
        self.armed = true;
    }

    pub fn disarm(&mut self) {
        self.armed = false;
    }

    pub fn restore_now(&mut self) -> Result<(), BackendError> {
        let result = restore_backend(self.backend.as_mut(), self.restore_to);
        if result.is_ok() {
            self.armed = false;
        }
        result
    }
}

impl Drop for RestoreGuard {
    fn drop(&mut self) {
        if self.armed {
            if let Err(error) = restore_backend(self.backend.as_mut(), self.restore_to) {
                eprintln!("satur8: warning, failed to restore saturation: {error}");
            }
        }
    }
}

fn restore_backend(backend: &mut dyn Backend, restore_to: Saturation) -> Result<(), BackendError> {
    if restore_to.is_identity() {
        backend.reset(&all_outputs())
    } else {
        backend.apply(&all_outputs(), restore_to)
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
    use satur8_core::CostNote;

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
    fn nothing_found_is_none() {
        assert!(select_in_order(&SELECTION_ORDER, |_| ProbeResult::Absent)
            .unwrap()
            .is_none());
    }
}
