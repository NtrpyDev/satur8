//! Satur8 core: the parts every backend shares.
//!
//! The only real logic here is the saturation matrix. Every backend (KWin
//! shader, DRM CTM, Hyprland CTM, gamescope reshade) applies the *same* 3x3
//! matrix; they differ only in how they hand it to the GPU. Keeping the math in
//! one place means the look is identical everywhere and is unit-tested once.

pub mod ctm;
pub mod env;
pub mod levels;
pub mod proc;
pub mod profile;
pub mod steam;
pub mod steam_shortcut;

pub use env::{Desktop, Environment, Gpu, SessionType};
pub use levels::{Levels, Note, Picture};
pub use proc::{resolve_exe, running_apps, RunningApp};
pub use profile::{MatchRule, OutputConfig, Profile, ProfileOutputs, Profiles};

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

    /// Clamps into the supported range. Non-finite values fall back to identity.
    pub fn new(value: f32) -> Saturation {
        Self::try_new(value).unwrap_or(Self::IDENTITY)
    }

    /// Rejects non-finite values and clamps finite values into the supported range.
    pub fn try_new(value: f32) -> Result<Saturation, SaturationError> {
        if !value.is_finite() {
            return Err(SaturationError(value));
        }
        Ok(Saturation(value.clamp(Self::MIN, Self::MAX)))
    }

    pub fn get(self) -> f32 {
        self.0
    }

    pub fn from_percent(percent: f32) -> Saturation {
        Saturation::new(1.0 + percent / 100.0)
    }

    pub fn to_percent(self) -> f32 {
        (self.0 - 1.0) * 100.0
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SaturationError(f32);

impl std::fmt::Display for SaturationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "saturation must be finite, got {}", self.0)
    }
}

impl std::error::Error for SaturationError {}

/// A display output a backend can act on (CRTC / monitor / KWin screen).
#[derive(Debug, Clone)]
pub struct Output {
    /// Stable identifier in the backend's namespace (connector name, KWin id...).
    pub id: String,
    pub human_name: String,
}

/// Sentinel used when an operation targets every output.
pub fn all_outputs() -> Output {
    Output {
        id: levels::ALL_OUTPUTS.into(),
        human_name: "All outputs".into(),
    }
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
pub trait Backend: Send {
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

    /// Whether different outputs can show different saturation at once.
    /// Default: no, one saturation on every output.
    fn per_output(&self) -> bool {
        false
    }

    /// What every output shows right now, when the backend can tell.
    /// Default: unknown.
    fn current_levels(&self) -> Option<Levels> {
        None
    }

    /// Why this backend shows one saturation on every output right now and
    /// what to do about it, when that is something the user can change.
    /// Default: none, it simply works that way.
    fn per_output_reason(&self) -> Option<String> {
        None
    }

    /// Show `levels`. The default handles a picture that is the same on every
    /// output through [`apply`](Backend::apply) on [`all_outputs`], and refuses
    /// any other.
    fn apply_levels(&mut self, levels: &Levels) -> Result<(), BackendError> {
        match levels.uniform_value() {
            Some(saturation) => self.apply(&all_outputs(), saturation),
            None => Err(BackendError::Unsupported(format!(
                "the {} backend shows one saturation on every output",
                self.name()
            ))),
        }
    }
}

#[derive(Debug)]
pub enum BackendError {
    Unavailable(String),
    Apply(String),
    /// The backend cannot do what was asked, such as different outputs at once.
    Unsupported(String),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendError::Unavailable(m) => write!(f, "backend unavailable: {m}"),
            BackendError::Apply(m) => write!(f, "failed to apply: {m}"),
            BackendError::Unsupported(m) => write!(f, "not supported: {m}"),
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

    /// A backend that only implements the required methods, recording `apply`.
    struct Uniform {
        applied: Vec<(String, f32)>,
    }

    impl Backend for Uniform {
        fn name(&self) -> &'static str {
            "uniform"
        }
        fn cost(&self) -> CostNote {
            CostNote::ZeroCost
        }
        fn outputs(&self) -> Vec<Output> {
            vec![all_outputs()]
        }
        fn apply(&mut self, output: &Output, saturation: Saturation) -> Result<(), BackendError> {
            self.applied.push((output.id.clone(), saturation.get()));
            Ok(())
        }
        fn reset(&mut self, _output: &Output) -> Result<(), BackendError> {
            Ok(())
        }
    }

    #[test]
    fn default_apply_levels_sends_a_uniform_picture_to_every_output() {
        let mut backend = Uniform { applied: vec![] };
        assert!(!backend.per_output());
        backend
            .apply_levels(&Levels::uniform(Saturation::new(1.5)))
            .unwrap();
        assert_eq!(backend.applied, [("all".to_string(), 1.5)]);
    }

    #[test]
    fn default_apply_levels_refuses_different_outputs() {
        let mut backend = Uniform { applied: vec![] };
        let levels = Levels::new(
            Saturation::IDENTITY,
            [("DP-1".to_string(), Saturation::new(1.5))],
        );
        assert!(matches!(
            backend.apply_levels(&levels),
            Err(BackendError::Unsupported(_))
        ));
        assert!(backend.applied.is_empty());
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

    #[test]
    fn try_new_rejects_non_finite_values() {
        assert!(Saturation::try_new(f32::NAN).is_err());
        assert!(Saturation::try_new(f32::INFINITY).is_err());
        assert!(Saturation::try_new(f32::NEG_INFINITY).is_err());
    }

    #[test]
    fn new_sanitizes_non_finite_values_to_identity() {
        assert_eq!(Saturation::new(f32::NAN), Saturation::IDENTITY);
        assert_eq!(Saturation::new(f32::INFINITY), Saturation::IDENTITY);
        assert_eq!(Saturation::new(f32::NEG_INFINITY), Saturation::IDENTITY);
    }

    #[test]
    fn identity_reports_identity_only_at_one() {
        assert!(Saturation::IDENTITY.is_identity());
        assert!(Saturation::new(1.0).is_identity());
        assert!(!Saturation::new(0.99).is_identity());
        assert!(!Saturation::new(1.01).is_identity());
    }

    #[test]
    fn percent_conversion_maps_gui_range_to_saturation() {
        for (percent, saturation) in [(-100.0, 0.0), (0.0, 1.0), (75.0, 1.75), (100.0, 2.0)] {
            let sat = Saturation::from_percent(percent);
            assert!(approx(sat.get(), saturation), "{percent}% -> {}", sat.get());
            assert!(
                approx(sat.to_percent(), percent),
                "{saturation} -> {}%",
                sat.to_percent()
            );
        }
    }

    #[test]
    fn percent_conversion_clamps_to_supported_saturation_range() {
        assert_eq!(Saturation::from_percent(-200.0).get(), Saturation::MIN);
        assert_eq!(Saturation::from_percent(400.0).get(), Saturation::MAX);
        assert!(approx(Saturation::new(Saturation::MAX).to_percent(), 300.0));
    }
}
