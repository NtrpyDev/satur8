//! Satur8 core: the parts every backend shares.
//!
//! The only real logic here is the saturation matrix. Every backend (KWin
//! shader, DRM CTM, Hyprland CTM, gamescope reshade) applies the *same* 3x3
//! matrix; they differ only in how they hand it to the GPU. Keeping the math in
//! one place means the look is identical everywhere and is unit-tested once.

pub mod ctm;
pub mod env;
pub mod profile;

pub use env::{Desktop, Environment, Gpu, SessionType};
pub use profile::{MatchRule, Profile, Profiles};

/// Rec.709 luma weights. Saturation is defined as a blend toward this luma.
pub const LUMA_R: f32 = 0.2126;
pub const LUMA_G: f32 = 0.7152;
pub const LUMA_B: f32 = 0.0722;

/// A saturation factor. `1.0` is unchanged, `>1.0` more vivid, `0.0` greyscale.
/// Range mirrors libvibrant (0.0..=4.0) so values feel familiar to users coming
/// from `vibrant-cli` / vibrantLinux.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Saturation(f32);

impl Saturation {
    pub const IDENTITY: Saturation = Saturation(1.0);
    pub const MIN: f32 = 0.0;
    pub const MAX: f32 = 4.0;

    /// Clamps into the supported range.
    pub fn new(value: f32) -> Saturation {
        Saturation(value.clamp(Self::MIN, Self::MAX))
    }

    pub fn get(self) -> f32 {
        self.0
    }

    pub fn is_identity(self) -> bool {
        (self.0 - 1.0).abs() < f32::EPSILON
    }

    /// The 3x3 saturation matrix, row-major. Identity when saturation is 1.0.
    ///
    /// Derived from `out = luma + s * (in - luma)`:
    /// `out_r = R*(w*Lr + s) + G*(w*Lg) + B*(w*Lb)`, where `w = 1 - s`.
    /// Backends convert this to their target format (kernel S31.32 fixed point
    /// for DRM CTM, a `mat3` uniform for the shader paths).
    pub fn matrix(self) -> [[f32; 3]; 3] {
        let s = self.0;
        let w = 1.0 - s;
        let (lr, lg, lb) = (LUMA_R, LUMA_G, LUMA_B);
        [
            [w * lr + s, w * lg, w * lb],
            [w * lr, w * lg + s, w * lb],
            [w * lr, w * lg, w * lb + s],
        ]
    }
}

/// A display output a backend can act on (CRTC / monitor / KWin screen).
#[derive(Debug, Clone)]
pub struct Output {
    /// Stable identifier in the backend's namespace (connector name, KWin id...).
    pub id: String,
    pub human_name: String,
}

/// Honest per-frame cost of a backend, surfaced to the user instead of hidden.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CostNote {
    /// Hardware scanout matrix - zero per-frame CPU and GPU. The ideal.
    ZeroCost,
    /// One extra fullscreen GPU pass in the compositor; negligible CPU.
    CompositorShaderPass,
    /// Nested compositor: extra composite pass plus a little added latency.
    NestedCompositor,
}

/// What every backend implements. `detect` answers "is this usable in the
/// current session?"; the daemon picks the lowest-cost available backend.
pub trait Backend {
    fn name(&self) -> &'static str;
    fn cost(&self) -> CostNote;
    fn outputs(&self) -> Vec<Output>;
    fn apply(&mut self, output: &Output, saturation: Saturation) -> Result<(), BackendError>;
    fn reset(&mut self, output: &Output) -> Result<(), BackendError>;

    /// Blend in linear light instead of gamma-encoded sRGB (more physically
    /// correct, subtly different look). Only the per-pixel shader backends can
    /// honor this; matrix/hardware backends (CTM, NV-CONTROL) work in their
    /// native space and ignore it. Default: no-op so every backend accepts it.
    fn set_linear_light(&mut self, _enabled: bool) -> Result<(), BackendError> {
        Ok(())
    }

    /// Whether this backend can honor [`set_linear_light`](Backend::set_linear_light).
    fn supports_linear_light(&self) -> bool {
        false
    }
}

#[derive(Debug)]
pub enum BackendError {
    Unavailable(String),
    Apply(String),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendError::Unavailable(m) => write!(f, "backend unavailable: {m}"),
            BackendError::Apply(m) => write!(f, "failed to apply: {m}"),
        }
    }
}

impl std::error::Error for BackendError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn identity_matrix_is_actually_identity() {
        let m = Saturation::IDENTITY.matrix();
        let id = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        for r in 0..3 {
            for c in 0..3 {
                assert!(approx(m[r][c], id[r][c]), "m[{r}][{c}] = {}", m[r][c]);
            }
        }
    }

    #[test]
    fn rows_sum_to_one_so_white_stays_white() {
        // Any saturation must preserve neutral grey/white: row sums == 1.
        for &s in &[0.0f32, 0.5, 1.0, 2.0, 4.0] {
            let m = Saturation::new(s).matrix();
            for row in m {
                assert!(approx(row[0] + row[1] + row[2], 1.0));
            }
        }
    }

    #[test]
    fn zero_saturation_is_pure_luma() {
        // At s=0 every output channel is the luma of the input.
        let m = Saturation::new(0.0).matrix();
        for row in m {
            assert!(approx(row[0], LUMA_R));
            assert!(approx(row[1], LUMA_G));
            assert!(approx(row[2], LUMA_B));
        }
    }

    #[test]
    fn saturation_clamps_to_range() {
        assert_eq!(Saturation::new(-1.0).get(), Saturation::MIN);
        assert_eq!(Saturation::new(99.0).get(), Saturation::MAX);
    }
}
