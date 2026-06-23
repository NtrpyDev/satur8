//! Backend selection shared across subcommands.

use anyhow::{bail, Result};
use vibrance_core::{Backend, Environment, Output};
use vibrance_drm_ctm::DrmCtmBackend;
use vibrance_gnome::GnomeBackend;
use vibrance_hyprland::HyprlandBackend;
use vibrance_kwin::KwinBackend;
use vibrance_nv_control::NvControlBackend;

/// The single output we act on for now (per-output targeting is M7).
pub fn all_outputs() -> Output {
    Output {
        id: "all".into(),
        human_name: "All outputs".into(),
    }
}

/// Resolve the best reachable backend for this environment, lowest cost and
/// most native first, matching PLAN.md's selection order. gamescope is *not*
/// here - it's a launch-only fallback used by `vibrance run`, not an apply/reset
/// backend.
pub fn select_backend() -> Result<Box<dyn Backend>> {
    if let Some(b) = KwinBackend::detect() {
        return Ok(Box::new(b)); // KDE Wayland
    }
    if let Some(b) = GnomeBackend::detect() {
        return Ok(Box::new(b)); // GNOME Wayland (extension enabled)
    }
    if let Some(b) = HyprlandBackend::detect() {
        return Ok(Box::new(b)); // Hyprland
    }
    if let Some(b) = NvControlBackend::detect() {
        return Ok(Box::new(b)); // X11 + NVIDIA
    }
    if let Some(b) = DrmCtmBackend::detect() {
        return Ok(Box::new(b)); // bare KMS / TTY, zero-cost
    }

    let envr = Environment::detect();
    bail!(
        "no usable apply/reset backend for this session ({}, {}, {}).\n\
         Preferred here is '{}'. On a niche wlroots compositor with no native \
         hook, use the gamescope fallback: `vibrance run --via gamescope -- <game>`.",
        envr.session,
        envr.desktop,
        envr.gpu,
        envr.preferred_backend()
    )
}
