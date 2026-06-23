//! Backend selection shared across subcommands.

use anyhow::{bail, Result};
use vibrance_core::{Backend, Environment, Output};
use vibrance_drm_ctm::DrmCtmBackend;
use vibrance_kwin::KwinBackend;

/// The single output we act on for now (per-output targeting is M7).
pub fn all_outputs() -> Output {
    Output {
        id: "all".into(),
        human_name: "All outputs".into(),
    }
}

/// Resolve the backend for this environment. Only KWin exists today; other
/// environments get a clear, honest error pointing at the roadmap.
pub fn select_backend() -> Result<Box<dyn Backend>> {
    // Lowest-cost reachable backend first. KWin (compositor pass) on KDE
    // Wayland; the zero-cost DRM CTM on bare KMS/TTY.
    if let Some(kwin) = KwinBackend::detect() {
        return Ok(Box::new(kwin));
    }
    if let Some(drm) = DrmCtmBackend::detect() {
        return Ok(Box::new(drm));
    }
    let envr = Environment::detect();
    bail!(
        "no usable backend for this session ({}, {}, {}).\n\
         The preferred backend here is '{}', which isn't implemented yet \
         (KWin/KDE Wayland is the current target). See PLAN.md for the roadmap.",
        envr.session,
        envr.desktop,
        envr.gpu,
        envr.preferred_backend()
    )
}
