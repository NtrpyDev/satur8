//! KWin backend (B1) - the MVP path for KDE Plasma Wayland.
//!
//! KWin owns DRM master and exposes no client CTM API, so we ship a tiny
//! single-purpose saturation effect (see `assets/kwin-effect/`) and drive it
//! over D-Bus from here. Loading/unloading the effect is how we turn the GPU
//! pass on and off, so when vibrance is "off" there is genuinely zero cost.
//!
//! Nothing here touches the game process - we only talk to the compositor.

use vibrance_core::{
    Backend, BackendError, CostNote, Desktop, Environment, Output, Saturation, SessionType,
};
use zbus::blocking::Connection;

/// KWin derives an effect's id from the installed plugin's filename. We install
/// the effect as `vibrance.so`, so the id KWin uses is `vibrance`.
const EFFECT_ID: &str = "vibrance";

const KWIN_SERVICE: &str = "org.kde.KWin";
const EFFECTS_PATH: &str = "/Effects";
const EFFECTS_IFACE: &str = "org.kde.kwin.Effects";
const EFFECT_PATH: &str = "/org/kde/KWin/Effect/Vibrance1";
const EFFECT_IFACE: &str = "org.kde.kwin.Effect.Vibrance";

pub struct KwinBackend {
    conn: Connection,
}

impl KwinBackend {
    /// Connect to the session bus and confirm KWin is actually reachable.
    /// Returns `None` when this isn't a KDE Wayland session with KWin live, so
    /// the selector can fall through to another backend.
    pub fn detect() -> Option<KwinBackend> {
        let envr = Environment::detect();
        if envr.session != SessionType::Wayland || envr.desktop != Desktop::Kde {
            return None;
        }
        let conn = Connection::session().ok()?;
        let backend = KwinBackend { conn };
        // Liveness probe: only succeeds if org.kde.KWin/Effects answers.
        backend.is_loaded().ok()?;
        Some(backend)
    }

    /// Whether our effect is currently loaded in KWin.
    pub fn is_loaded(&self) -> Result<bool, BackendError> {
        let reply = self
            .conn
            .call_method(
                Some(KWIN_SERVICE),
                EFFECTS_PATH,
                Some(EFFECTS_IFACE),
                "isEffectLoaded",
                &EFFECT_ID,
            )
            .map_err(apply_err)?;
        reply.body().deserialize::<bool>().map_err(apply_err)
    }

    /// Load the effect (turns the GPU pass on). Idempotent.
    pub fn load(&self) -> Result<(), BackendError> {
        if self.is_loaded()? {
            return Ok(());
        }
        let reply = self
            .conn
            .call_method(
                Some(KWIN_SERVICE),
                EFFECTS_PATH,
                Some(EFFECTS_IFACE),
                "loadEffect",
                &EFFECT_ID,
            )
            .map_err(apply_err)?;
        let ok = reply.body().deserialize::<bool>().map_err(apply_err)?;
        if !ok {
            return Err(BackendError::Apply(format!(
                "KWin refused to load effect '{EFFECT_ID}'. Is vibrance.so installed \
                 in a kwin/effects/plugins dir on KWin's plugin path?"
            )));
        }
        Ok(())
    }

    /// Unload the effect (turns the GPU pass off - zero cost afterwards).
    pub fn unload(&self) -> Result<(), BackendError> {
        self.conn
            .call_method(
                Some(KWIN_SERVICE),
                EFFECTS_PATH,
                Some(EFFECTS_IFACE),
                "unloadEffect",
                &EFFECT_ID,
            )
            .map(|_| ())
            .map_err(apply_err)
    }

    /// Current saturation reported by the loaded effect.
    pub fn current_saturation(&self) -> Result<Saturation, BackendError> {
        let reply = self
            .conn
            .call_method(
                Some(KWIN_SERVICE),
                EFFECT_PATH,
                Some(EFFECT_IFACE),
                "saturation",
                &(),
            )
            .map_err(|e| {
                BackendError::Apply(format!("couldn't read saturation (effect loaded?): {e}"))
            })?;
        let v = reply.body().deserialize::<f64>().map_err(apply_err)?;
        Ok(Saturation::new(v as f32))
    }

    fn set_saturation(&self, sat: Saturation) -> Result<(), BackendError> {
        self.conn
            .call_method(
                Some(KWIN_SERVICE),
                EFFECT_PATH,
                Some(EFFECT_IFACE),
                "setSaturation",
                &(sat.get() as f64),
            )
            .map(|_| ())
            .map_err(|e| {
                BackendError::Apply(format!("couldn't set saturation (effect loaded?): {e}"))
            })
    }
}

impl Backend for KwinBackend {
    fn name(&self) -> &'static str {
        "kwin"
    }

    fn cost(&self) -> CostNote {
        CostNote::CompositorShaderPass
    }

    fn outputs(&self) -> Vec<Output> {
        // Per-output targeting is M7; for now the effect applies session-wide.
        vec![Output {
            id: "all".into(),
            human_name: "All outputs".into(),
        }]
    }

    fn apply(&mut self, _output: &Output, saturation: Saturation) -> Result<(), BackendError> {
        self.load()?;
        self.set_saturation(saturation)
    }

    fn reset(&mut self, _output: &Output) -> Result<(), BackendError> {
        // Identity then unload, so we leave no GPU pass running.
        let _ = self.set_saturation(Saturation::IDENTITY);
        self.unload()
    }
}

fn apply_err(e: zbus::Error) -> BackendError {
    BackendError::Apply(e.to_string())
}
