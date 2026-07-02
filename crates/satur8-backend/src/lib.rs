//! Shared backend selection and temporary-application lifecycle.

use std::io;
use std::thread::{self, JoinHandle};

use anyhow::{bail, Result};
use satur8_core::{Backend, BackendError, Environment, Output, Saturation};
use satur8_drm_ctm::DrmCtmBackend;
use satur8_gnome::GnomeBackend;
use satur8_hyprland::HyprlandBackend;
use satur8_kwin::KwinBackend;
use satur8_nv_control::NvControlBackend;
use signal_hook::iterator::{Handle, Signals};

/// The single output Satur8 acts on until per-output targeting is implemented.
pub fn all_outputs() -> Output {
    Output {
        id: "all".into(),
        human_name: "All outputs".into(),
    }
}

/// Resolve the best reachable apply/reset backend in canonical cost order.
///
/// Gamescope is launch-only and is intentionally not part of this selector.
pub fn select_backend() -> Result<Box<dyn Backend>> {
    if let Some(backend) = KwinBackend::detect() {
        return Ok(Box::new(backend));
    }
    if let Some(backend) = GnomeBackend::detect() {
        return Ok(Box::new(backend));
    }
    if let Some(backend) = HyprlandBackend::detect() {
        return Ok(Box::new(backend));
    }
    if let Some(backend) = NvControlBackend::detect() {
        return Ok(Box::new(backend));
    }
    if let Some(backend) = DrmCtmBackend::detect() {
        return Ok(Box::new(backend));
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
