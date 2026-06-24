//! NVIDIA X11 backend (B5) - native Digital Vibrance.
//!
//! The proprietary NVIDIA driver exposes the *exact* feature VibranceGUI drives
//! on Windows: "Digital Vibrance", over the NV-CONTROL X extension. On Linux X11
//! the documented interface is `nvidia-settings -a "[gpu:0]/DigitalVibrance=N"`,
//! N in roughly -1024..1023 (0 = neutral). This is native and zero-cost - the
//! driver applies it in the display pipeline, not in the game.
//!
//! We shell out to `nvidia-settings` rather than reimplementing NV-CONTROL: it's
//! the supported surface and avoids an X dependency. NVIDIA-on-Wayland is covered
//! by the compositor shader backends instead (KWin/GNOME/Hyprland), not this.
//!
//! Verified on real NVIDIA hardware (RTX 3070 Ti, X11): `satur8 on` drives the
//! driver's Digital Vibrance via `nvidia-settings`. Gated to X11 + NVIDIA by
//! `detect()`. The saturation->Digital-Vibrance mapping is unit-tested.

use std::path::PathBuf;
use std::process::Command;

use satur8_core::{
    Backend, BackendError, CostNote, Environment, Gpu, Output, Saturation, SessionType,
};

const DV_MIN: i32 = -1024;
const DV_MAX: i32 = 1023;

/// Map satur8-core saturation (0.0..=4.0, 1.0 = neutral) onto the driver's
/// Digital Vibrance range (-1024..=1023, 0 = neutral). Above neutral we scale
/// up to the +max at s=4; below neutral down to -max (full desaturation) at s=0.
pub fn saturation_to_digital_vibrance(saturation: Saturation) -> i32 {
    let s = saturation.get();
    let dv = if s >= 1.0 {
        ((s - 1.0) / (Saturation::MAX - 1.0)) * DV_MAX as f32
    } else {
        (1.0 - s) * DV_MIN as f32 // (1-s) in [0,1] * -1024 -> down to -1024
    };
    (dv.round() as i32).clamp(DV_MIN, DV_MAX)
}

pub struct NvControlBackend;

impl NvControlBackend {
    pub fn detect() -> Option<NvControlBackend> {
        let env = Environment::detect();
        if env.session != SessionType::X11 || env.gpu != Gpu::Nvidia {
            return None;
        }
        which("nvidia-settings").map(|_| NvControlBackend)
    }

    fn set_vibrance(&self, dv: i32) -> Result<(), BackendError> {
        // [gpu:0] applies to the GPU's attached displays; per-display targeting
        // ([DPY:...]) is an M7 refinement.
        let attr = format!("[gpu:0]/DigitalVibrance={dv}");
        let out = Command::new("nvidia-settings")
            .args(["-a", &attr])
            .output()
            .map_err(|e| BackendError::Apply(format!("running nvidia-settings: {e}")))?;
        if !out.status.success() {
            return Err(BackendError::Apply(format!(
                "nvidia-settings failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(())
    }
}

impl Backend for NvControlBackend {
    fn name(&self) -> &'static str {
        "nv-control"
    }

    fn cost(&self) -> CostNote {
        CostNote::ZeroCost
    }

    fn outputs(&self) -> Vec<Output> {
        vec![Output {
            id: "gpu:0".into(),
            human_name: "NVIDIA GPU 0 displays".into(),
        }]
    }

    fn apply(&mut self, _output: &Output, saturation: Saturation) -> Result<(), BackendError> {
        self.set_vibrance(saturation_to_digital_vibrance(saturation))
    }

    fn reset(&mut self, _output: &Output) -> Result<(), BackendError> {
        self.set_vibrance(0)
    }
}

fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(bin))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_maps_to_zero() {
        assert_eq!(saturation_to_digital_vibrance(Saturation::IDENTITY), 0);
    }

    #[test]
    fn max_maps_to_max() {
        assert_eq!(saturation_to_digital_vibrance(Saturation::new(4.0)), DV_MAX);
    }

    #[test]
    fn zero_maps_to_min() {
        assert_eq!(saturation_to_digital_vibrance(Saturation::new(0.0)), DV_MIN);
    }

    #[test]
    fn stays_in_range() {
        for &s in &[0.0f32, 0.5, 1.0, 1.6, 2.5, 4.0] {
            let dv = saturation_to_digital_vibrance(Saturation::new(s));
            assert!((DV_MIN..=DV_MAX).contains(&dv));
        }
    }
}
