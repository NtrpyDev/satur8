//! Backend selection shared across subcommands.

use anyhow::{bail, Result};
use satur8_core::{Backend, Environment, Output};
use satur8_drm_ctm::DrmCtmBackend;
use satur8_gnome::GnomeBackend;
use satur8_hyprland::HyprlandBackend;
use satur8_kwin::KwinBackend;
use satur8_nv_control::NvControlBackend;

/// The single output we act on for now (per-output targeting is M7).
pub fn all_outputs() -> Output {
    Output {
        id: "all".into(),
        human_name: "All outputs".into(),
    }
}

/// Resolve the best reachable backend for this environment, lowest cost and
/// most native first, matching PLAN.md's selection order. gamescope is *not*
/// here - it's a launch-only fallback used by `satur8 run`, not an apply/reset
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
         hook, use the gamescope fallback: `satur8 run --via gamescope -- <game>`.",
        envr.session,
        envr.desktop,
        envr.gpu,
        envr.preferred_backend()
    )
}
