//! KWin backend (B1) - the MVP path for KDE Plasma Wayland.
//!
//! KWin owns DRM master and exposes no client CTM API, so we ship a tiny
//! single-purpose saturation effect (see `assets/kwin-effect/`) and drive it
//! over D-Bus from here. Loading/unloading the effect is how we turn the GPU
//! pass on and off, so when satur8 is "off" there is genuinely zero cost.
//!
//! Nothing here touches the game process - we only talk to the compositor.

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};

use satur8_core::{
    Backend, BackendError, CostNote, Desktop, Environment, Levels, Output, Saturation, SessionType,
};
use zbus::blocking::Connection;
use zbus::zvariant::{OwnedValue, Value};

pub mod repair;

/// KWin derives an effect's id from the installed plugin's filename. We install
/// the effect as `satur8.so`, so the id KWin uses is `satur8`.
const EFFECT_ID: &str = "satur8";

const KWIN_SERVICE: &str = "org.kde.KWin";
const EFFECTS_PATH: &str = "/Effects";
const EFFECTS_IFACE: &str = "org.kde.kwin.Effects";
const EFFECT_PATH: &str = "/org/kde/KWin/Effect/Satur81";
const EFFECT_IFACE: &str = "org.kde.kwin.Effect.Satur8";

pub struct KwinBackend {
    conn: Connection,
    /// What the effect KWin runs can do with outputs, once asked. KWin keeps
    /// the copy it loaded for the whole session, so the answer holds.
    per_output: Cell<Option<EffectOutputs>>,
}

/// What the effect KWin runs can do with outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectOutputs {
    /// It answers the per-output calls.
    PerOutput,
    /// It is loaded but has no per-output calls: an effect from before
    /// per-output support. `installed_is_newer` is whether the effect file on
    /// disk has them, which means KWin still runs the copy it loaded before
    /// an upgrade.
    OlderLoaded { installed_is_newer: bool },
    /// It could not be loaded or asked.
    Unavailable,
}

impl EffectOutputs {
    /// Why one value applies to every output, and the one next step; `None`
    /// when the effect can do per-output.
    pub fn explanation(self) -> Option<&'static str> {
        match self {
            EffectOutputs::PerOutput => None,
            EffectOutputs::OlderLoaded {
                installed_is_newer: true,
            } => Some(
                "KDE is still running the Satur8 effect from before the upgrade, so one \
                 value applies to every output. Log out and back in once to use a \
                 different value per output.",
            ),
            EffectOutputs::OlderLoaded {
                installed_is_newer: false,
            } => Some(
                "The installed Satur8 KWin effect is from before per-output support, so \
                 one value applies to every output. Reinstall Satur8 or run \
                 `satur8 repair`, then log out and back in once.",
            ),
            EffectOutputs::Unavailable => Some(
                "Satur8 could not ask its KWin effect what it supports, so one value \
                 applies to every output. `satur8 doctor` shows the effect's state.",
            ),
        }
    }
}

/// The state from a probe of the running effect. The effect file on disk is
/// only read for an effect without the per-output calls.
fn effect_outputs_state(
    probe: &Result<Option<BTreeMap<String, String>>, BackendError>,
    installed_has_per_output: impl FnOnce() -> Option<bool>,
) -> EffectOutputs {
    match probe {
        Ok(Some(_)) => EffectOutputs::PerOutput,
        Ok(None) => EffectOutputs::OlderLoaded {
            installed_is_newer: installed_has_per_output() == Some(true),
        },
        Err(_) => EffectOutputs::Unavailable,
    }
}

impl KwinBackend {
    /// Connect to the session bus and confirm KWin is actually reachable.
    /// Returns `None` when this isn't a KDE Wayland session with KWin live, so
    /// the selector can fall through to another backend.
    pub fn connect() -> Option<KwinBackend> {
        let envr = Environment::detect();
        if envr.session != SessionType::Wayland || envr.desktop != Desktop::Kde {
            return None;
        }
        let conn = Connection::session().ok()?;
        let backend = KwinBackend {
            conn,
            per_output: Cell::new(None),
        };
        // Liveness probe: only succeeds if org.kde.KWin/Effects answers.
        backend.is_loaded().ok()?;
        Some(backend)
    }

    /// Detect a KWin session only when the installed effect is loadable by the
    /// current compositor ABI.
    pub fn detect() -> Option<KwinBackend> {
        let backend = Self::connect()?;
        backend.is_supported().ok().filter(|supported| *supported)?;
        Some(backend)
    }

    /// The running KWin's version, from its support information; `None` when
    /// KWin does not answer or the text has no version line.
    pub fn running_version(&self) -> Option<String> {
        let reply = self
            .conn
            .call_method(
                Some(KWIN_SERVICE),
                "/KWin",
                Some("org.kde.KWin"),
                "supportInformation",
                &(),
            )
            .ok()?;
        let text = reply.body().deserialize::<String>().ok()?;
        repair::abi_in_support_information(&text)
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

    /// Whether KWin can load the installed effect. False covers a missing
    /// plugin as well as the common exact-ABI mismatch after a KWin upgrade.
    pub fn is_supported(&self) -> Result<bool, BackendError> {
        let reply = self
            .conn
            .call_method(
                Some(KWIN_SERVICE),
                EFFECTS_PATH,
                Some(EFFECTS_IFACE),
                "isEffectSupported",
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
        if !self.is_supported()? {
            return Err(BackendError::Unavailable(format!(
                "the Satur8 KWin effect is missing or was built for a different KWin version; {}",
                repair::advice_here()
            )));
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
                "KWin refused to load effect '{EFFECT_ID}'. Is satur8.so installed \
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
        Saturation::try_new(v as f32)
            .map_err(|error| BackendError::Apply(format!("bad saturation value: {error}")))
    }

    /// Toggle linear-light blending in the effect (a no-op until the effect is
    /// loaded; the value is re-sent on the next apply path anyway).
    pub fn set_linear(&self, enabled: bool) -> Result<(), BackendError> {
        self.conn
            .call_method(
                Some(KWIN_SERVICE),
                EFFECT_PATH,
                Some(EFFECT_IFACE),
                "setLinearLight",
                &enabled,
            )
            .map(|_| ())
            .map_err(|e| {
                BackendError::Apply(format!("couldn't set linear light (effect loaded?): {e}"))
            })
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

/// The D-Bus error name for a method the object does not have.
const UNKNOWN_METHOD: &str = "org.freedesktop.DBus.Error.UnknownMethod";

/// Whether a failed call means the effect has no such method: an effect built
/// before per-output support.
fn is_unknown_method(error: &zbus::Error) -> bool {
    matches!(error, zbus::Error::MethodError(name, _, _) if name.as_str() == UNKNOWN_METHOD)
}

impl KwinBackend {
    /// What the effect KWin runs can do with outputs. A real answer is kept for
    /// the session; an effect that could not be asked is asked again next time.
    pub fn effect_outputs(&self) -> EffectOutputs {
        if let Some(known) = self.per_output.get() {
            return known;
        }
        let probe = probe_outputs(self);
        self.remember_state(&probe)
    }

    fn remember_state(
        &self,
        probe: &Result<Option<BTreeMap<String, String>>, BackendError>,
    ) -> EffectOutputs {
        let state = effect_outputs_state(probe, repair::installed_effect_has_per_output);
        if state != EffectOutputs::Unavailable {
            self.per_output.set(Some(state));
        }
        state
    }

    fn call_effect<B>(&self, method: &str, body: &B) -> Result<zbus::Message, zbus::Error>
    where
        B: zbus::export::serde::Serialize + zbus::zvariant::DynamicType,
    {
        self.conn.call_method(
            Some(KWIN_SERVICE),
            EFFECT_PATH,
            Some(EFFECT_IFACE),
            method,
            body,
        )
    }
}

/// The effect calls the backend makes, so the per-output logic is tested with
/// a fake instead of a running KWin.
trait EffectApi {
    fn effect_loaded(&self) -> Result<bool, BackendError>;
    fn load_effect(&self) -> Result<(), BackendError>;
    fn unload_effect(&self) -> Result<(), BackendError>;
    /// `setSaturation`: every output shows `saturation`.
    fn set_one_level(&self, saturation: Saturation) -> Result<(), BackendError>;
    /// `saturation`: the level outputs without their own show.
    fn base_level(&self) -> Result<Saturation, BackendError>;
    /// `outputs`: connected output name to description, or `None` when the
    /// loaded effect has no such method.
    fn effect_outputs(&self) -> Result<Option<BTreeMap<String, String>>, BackendError>;
    /// `setOutputSaturation`: one output's level; the others keep theirs.
    fn set_output_level(&self, output: &str, saturation: Saturation) -> Result<(), BackendError>;
    /// `setSaturations`: the whole picture in one call.
    fn set_levels(
        &self,
        base: Saturation,
        outputs: &BTreeMap<String, Saturation>,
    ) -> Result<(), BackendError>;
    /// `outputSaturations`: outputs that differ from the base, or `None` when
    /// the loaded effect has no such method.
    fn output_levels(&self) -> Result<Option<BTreeMap<String, Saturation>>, BackendError>;
}

impl EffectApi for KwinBackend {
    fn effect_loaded(&self) -> Result<bool, BackendError> {
        self.is_loaded()
    }

    fn load_effect(&self) -> Result<(), BackendError> {
        self.load()
    }

    fn unload_effect(&self) -> Result<(), BackendError> {
        self.unload()
    }

    fn set_one_level(&self, saturation: Saturation) -> Result<(), BackendError> {
        self.set_saturation(saturation)
    }

    fn base_level(&self) -> Result<Saturation, BackendError> {
        self.current_saturation()
    }

    fn effect_outputs(&self) -> Result<Option<BTreeMap<String, String>>, BackendError> {
        match self.call_effect("outputs", &()) {
            Ok(reply) => {
                let map: HashMap<String, OwnedValue> =
                    reply.body().deserialize().map_err(apply_err)?;
                Ok(Some(
                    map.into_iter()
                        .map(|(name, value)| {
                            let description = String::try_from(value).unwrap_or_default();
                            (name, description)
                        })
                        .collect(),
                ))
            }
            Err(error) if is_unknown_method(&error) => Ok(None),
            Err(error) => Err(apply_err(error)),
        }
    }

    fn set_output_level(&self, output: &str, saturation: Saturation) -> Result<(), BackendError> {
        self.call_effect(
            "setOutputSaturation",
            &(output, f64::from(saturation.get())),
        )
        .map(|_| ())
        .map_err(|e| BackendError::Apply(format!("couldn't set saturation on {output}: {e}")))
    }

    fn set_levels(
        &self,
        base: Saturation,
        outputs: &BTreeMap<String, Saturation>,
    ) -> Result<(), BackendError> {
        let table: HashMap<&str, Value<'_>> = outputs
            .iter()
            .map(|(name, level)| (name.as_str(), Value::from(f64::from(level.get()))))
            .collect();
        self.call_effect("setSaturations", &(f64::from(base.get()), table))
            .map(|_| ())
            .map_err(|e| BackendError::Apply(format!("couldn't set per-output saturation: {e}")))
    }

    fn output_levels(&self) -> Result<Option<BTreeMap<String, Saturation>>, BackendError> {
        match self.call_effect("outputSaturations", &()) {
            Ok(reply) => {
                let map: HashMap<String, OwnedValue> =
                    reply.body().deserialize().map_err(apply_err)?;
                Ok(Some(
                    map.into_iter()
                        .filter_map(|(name, value)| {
                            let level = f64::try_from(value).ok()?;
                            Saturation::try_new(level as f32).ok().map(|s| (name, s))
                        })
                        .collect(),
                ))
            }
            Err(error) if is_unknown_method(&error) => Ok(None),
            Err(error) => Err(apply_err(error)),
        }
    }
}

/// The outputs the running effect lists, `Ok(None)` for an effect without
/// per-output support, or the error when the effect could not be asked. The
/// effect is only on the bus while loaded, so it is loaded for the question
/// when needed and unloaded again afterwards.
fn probe_outputs(api: &dyn EffectApi) -> Result<Option<BTreeMap<String, String>>, BackendError> {
    let was_loaded = api.effect_loaded()?;
    if !was_loaded {
        api.load_effect()?;
    }
    let outputs = api.effect_outputs();
    if !was_loaded {
        let _ = api.unload_effect();
    }
    outputs
}

/// `outputs()` for a per-output effect: one row per output it lists.
fn outputs_from(listed: BTreeMap<String, String>) -> Vec<Output> {
    listed
        .into_iter()
        .map(|(id, description)| {
            let human_name = if description.trim().is_empty() {
                id.clone()
            } else {
                description
            };
            Output { id, human_name }
        })
        .collect()
}

/// Show `levels`. An unchanged picture unloads the effect, as `apply` at 1.0
/// does. A uniform picture goes through `setSaturation`, which every effect
/// version has. Any other picture needs the per-output calls: one
/// `setSaturations`, so no frame shows half of it. Entries for outputs that are
/// not connected are sent too: the effect matches names when it draws, so
/// such an output gets its level as soon as it is plugged in.
fn apply_levels_via(api: &dyn EffectApi, levels: &Levels) -> Result<(), BackendError> {
    if levels.is_identity() {
        let _ = api.unload_effect();
        return Ok(());
    }
    api.load_effect()?;
    if let Some(saturation) = levels.uniform_value() {
        return api.set_one_level(saturation);
    }
    if api.effect_outputs()?.is_none() {
        return Err(BackendError::Unsupported(format!(
            "the loaded KWin effect shows one saturation on every output; {}",
            "log out and back in to load the newer effect"
        )));
    }
    api.set_levels(levels.base, &levels.outputs)
}

/// Why a single output cannot be set: the loaded effect predates per-output
/// support.
fn one_level_only() -> BackendError {
    BackendError::Unsupported(
        "the loaded KWin effect shows one saturation on every output; \
         log out and back in to load the newer effect"
            .to_string(),
    )
}

/// Unload the effect once every output is back to 1.0, so no GPU pass stays.
fn unload_if_unchanged(api: &dyn EffectApi) {
    if current_levels_via(api).is_some_and(|levels| levels.is_identity()) {
        let _ = api.unload_effect();
    }
}

/// A manual change to one output (`satur8 set --output NAME`): only that
/// output changes, every other output keeps its level.
fn apply_output_via(
    api: &dyn EffectApi,
    output: &str,
    saturation: Saturation,
) -> Result<(), BackendError> {
    api.load_effect()?;
    if api.effect_outputs()?.is_none() {
        return Err(one_level_only());
    }
    api.set_output_level(output, saturation)?;
    unload_if_unchanged(api);
    Ok(())
}

/// Put one output back to the base level; the others keep theirs.
fn reset_output_via(api: &dyn EffectApi, output: &str) -> Result<(), BackendError> {
    if !api.effect_loaded()? {
        return Ok(());
    }
    if api.effect_outputs()?.is_none() {
        return Err(one_level_only());
    }
    api.set_output_level(output, api.base_level()?)?;
    unload_if_unchanged(api);
    Ok(())
}

/// What every output shows now: unchanged when the effect is not loaded, else
/// the effect's base and, for a per-output effect, its table.
fn current_levels_via(api: &dyn EffectApi) -> Option<Levels> {
    if !api.effect_loaded().ok()? {
        return Some(Levels::uniform(Saturation::IDENTITY));
    }
    let base = api.base_level().ok()?;
    let table = api.output_levels().ok()?.unwrap_or_default();
    Some(Levels::new(base, table))
}

impl Backend for KwinBackend {
    fn name(&self) -> &'static str {
        "kwin"
    }

    fn cost(&self) -> CostNote {
        CostNote::CompositorShaderPass
    }

    /// The outputs a per-output effect lists; the single "all" row for an
    /// older effect, which applies one saturation session-wide.
    fn outputs(&self) -> Vec<Output> {
        let probe = probe_outputs(self);
        self.remember_state(&probe);
        match probe {
            Ok(Some(listed)) => outputs_from(listed),
            Ok(None) | Err(_) => vec![satur8_core::all_outputs()],
        }
    }

    /// A named output changes only that output, through the per-output calls.
    /// Every output (`"all"`) works as it always has.
    fn apply(&mut self, output: &Output, saturation: Saturation) -> Result<(), BackendError> {
        if output.id != satur8_core::levels::ALL_OUTPUTS {
            return apply_output_via(self, &output.id, saturation);
        }
        // Identity means "no change". Loading the effect at identity would
        // leave an unnecessary compositor pass running, so unload instead.
        if saturation.is_identity() {
            let _ = self.unload();
            return Ok(());
        }
        self.load()?;
        self.set_saturation(saturation)
    }

    fn reset(&mut self, output: &Output) -> Result<(), BackendError> {
        if output.id != satur8_core::levels::ALL_OUTPUTS {
            return reset_output_via(self, &output.id);
        }
        // Identity then unload, so we leave no GPU pass running.
        let _ = self.set_saturation(Saturation::IDENTITY);
        self.unload()
    }

    fn set_linear_light(&mut self, enabled: bool) -> Result<(), BackendError> {
        self.set_linear(enabled)
    }

    fn supports_linear_light(&self) -> bool {
        true
    }

    /// True only when the effect KWin runs answers the per-output calls. An
    /// effect KWin loaded before an upgrade keeps one level on every output
    /// until the next login.
    fn per_output(&self) -> bool {
        self.effect_outputs() == EffectOutputs::PerOutput
    }

    fn per_output_reason(&self) -> Option<String> {
        self.effect_outputs().explanation().map(str::to_string)
    }

    fn apply_levels(&mut self, levels: &Levels) -> Result<(), BackendError> {
        apply_levels_via(self, levels)
    }

    fn current_levels(&self) -> Option<Levels> {
        current_levels_via(self)
    }
}

fn apply_err(e: zbus::Error) -> BackendError {
    BackendError::Apply(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    use satur8_core::levels::{desktop_picture, profile_outputs_not_connected, profile_picture};
    use satur8_core::{MatchRule, Note, OutputConfig, Profile, Profiles};

    /// One call the backend made on the effect.
    #[derive(Debug, Clone, PartialEq)]
    enum Call {
        Load,
        Unload,
        SetOne(f32),
        SetOutput(String, f32),
        SetLevels(f32, Vec<(String, f32)>),
    }

    /// A KWin with the effect, which is either new (answers the per-output
    /// calls) or old (has only `setSaturation` and `saturation`).
    struct FakeEffect {
        new_effect: bool,
        listed: BTreeMap<String, String>,
        loaded: RefCell<bool>,
        base: RefCell<f32>,
        table: RefCell<BTreeMap<String, Saturation>>,
        calls: RefCell<Vec<Call>>,
    }

    impl FakeEffect {
        fn new(new_effect: bool) -> FakeEffect {
            FakeEffect {
                new_effect,
                listed: [("DP-1", "Dell U2720Q"), ("HDMI-A-1", "LG TV")]
                    .map(|(id, name)| (id.to_string(), name.to_string()))
                    .into(),
                loaded: RefCell::new(false),
                base: RefCell::new(1.0),
                table: RefCell::new(BTreeMap::new()),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.borrow().clone()
        }
    }

    impl EffectApi for FakeEffect {
        fn effect_loaded(&self) -> Result<bool, BackendError> {
            Ok(*self.loaded.borrow())
        }

        fn load_effect(&self) -> Result<(), BackendError> {
            if !*self.loaded.borrow() {
                *self.loaded.borrow_mut() = true;
                self.calls.borrow_mut().push(Call::Load);
            }
            Ok(())
        }

        fn unload_effect(&self) -> Result<(), BackendError> {
            *self.loaded.borrow_mut() = false;
            self.calls.borrow_mut().push(Call::Unload);
            Ok(())
        }

        fn set_one_level(&self, saturation: Saturation) -> Result<(), BackendError> {
            *self.base.borrow_mut() = saturation.get();
            self.table.borrow_mut().clear();
            self.calls.borrow_mut().push(Call::SetOne(saturation.get()));
            Ok(())
        }

        fn base_level(&self) -> Result<Saturation, BackendError> {
            Ok(Saturation::new(*self.base.borrow()))
        }

        fn effect_outputs(&self) -> Result<Option<BTreeMap<String, String>>, BackendError> {
            Ok(self.new_effect.then(|| self.listed.clone()))
        }

        fn set_output_level(
            &self,
            output: &str,
            saturation: Saturation,
        ) -> Result<(), BackendError> {
            assert!(self.new_effect, "an old effect has no setOutputSaturation");
            // As the effect does: a value equal to the base removes the entry.
            if saturation.get() == *self.base.borrow() {
                self.table.borrow_mut().remove(output);
            } else {
                self.table
                    .borrow_mut()
                    .insert(output.to_string(), saturation);
            }
            self.calls
                .borrow_mut()
                .push(Call::SetOutput(output.to_string(), saturation.get()));
            Ok(())
        }

        fn set_levels(
            &self,
            base: Saturation,
            outputs: &BTreeMap<String, Saturation>,
        ) -> Result<(), BackendError> {
            assert!(self.new_effect, "an old effect has no setSaturations");
            *self.base.borrow_mut() = base.get();
            *self.table.borrow_mut() = outputs.clone();
            self.calls.borrow_mut().push(Call::SetLevels(
                base.get(),
                outputs
                    .iter()
                    .map(|(name, level)| (name.clone(), level.get()))
                    .collect(),
            ));
            Ok(())
        }

        fn output_levels(&self) -> Result<Option<BTreeMap<String, Saturation>>, BackendError> {
            Ok(self.new_effect.then(|| self.table.borrow().clone()))
        }
    }

    fn levels(base: f32, outputs: &[(&str, f32)]) -> Levels {
        Levels::new(
            Saturation::new(base),
            outputs
                .iter()
                .map(|(name, value)| (name.to_string(), Saturation::new(*value))),
        )
    }

    fn couch_profile(outputs: &[&str]) -> Profile {
        Profile {
            name: "couch".into(),
            enabled: true,
            saturation: 1.3,
            match_rule: MatchRule {
                exe: Some("hades".into()),
                ..MatchRule::default()
            },
            outputs: outputs.iter().map(|o| o.to_string()).collect(),
        }
    }

    #[test]
    fn a_new_effect_is_per_output_and_lists_its_outputs() {
        let effect = FakeEffect::new(true);
        let listed = probe_outputs(&effect).unwrap().unwrap();
        assert_eq!(
            outputs_from(listed)
                .into_iter()
                .map(|o| (o.id, o.human_name))
                .collect::<Vec<_>>(),
            [
                ("DP-1".to_string(), "Dell U2720Q".to_string()),
                ("HDMI-A-1".to_string(), "LG TV".to_string()),
            ]
        );
        // The effect was not loaded before the question, so it is not after.
        assert_eq!(effect.calls(), [Call::Load, Call::Unload]);
    }

    #[test]
    fn a_new_effect_gets_the_whole_picture_in_one_call() {
        let effect = FakeEffect::new(true);
        apply_levels_via(&effect, &levels(1.0, &[("HDMI-A-1", 1.3)])).unwrap();
        assert_eq!(
            effect.calls(),
            [
                Call::Load,
                Call::SetLevels(1.0, vec![("HDMI-A-1".to_string(), 1.3)])
            ]
        );
        assert_eq!(
            current_levels_via(&effect),
            Some(levels(1.0, &[("HDMI-A-1", 1.3)]))
        );
    }

    /// The same calls as before per-output support: a uniform picture is one
    /// `setSaturation`, an unchanged one unloads the effect.
    #[test]
    fn uniform_pictures_use_the_calls_every_effect_has() {
        for new_effect in [true, false] {
            let effect = FakeEffect::new(new_effect);
            apply_levels_via(&effect, &levels(1.5, &[])).unwrap();
            apply_levels_via(&effect, &levels(1.0, &[])).unwrap();
            assert_eq!(
                effect.calls(),
                [Call::Load, Call::SetOne(1.5), Call::Unload],
                "new effect: {new_effect}"
            );
        }
    }

    #[test]
    fn an_old_effect_falls_back_to_one_level_with_the_note() {
        let effect = FakeEffect::new(false);
        let per_output = probe_outputs(&effect).unwrap().is_some();
        assert!(!per_output);

        let profiles = Profiles {
            profiles: vec![couch_profile(&["HDMI-A-1"])],
            ..Profiles::default()
        };
        let picture = profile_picture(&profiles, &profiles.profiles[0], None, per_output);
        assert_eq!(
            picture.notes,
            [Note::ProfileChangesEveryOutput {
                profile: "couch".into()
            }]
        );
        apply_levels_via(&effect, &picture.levels).unwrap();
        assert_eq!(effect.calls().last(), Some(&Call::SetOne(1.3)));

        // A mixed picture cannot reach an old effect.
        let err = apply_levels_via(&effect, &levels(1.0, &[("HDMI-A-1", 1.3)])).unwrap_err();
        assert!(matches!(err, BackendError::Unsupported(_)), "{err}");
        assert_eq!(current_levels_via(&effect), Some(levels(1.3, &[])));
    }

    /// The entry is kept, so the output gets its level when it is plugged in;
    /// until then it matches no window, and the user is told it is not
    /// connected.
    #[test]
    fn an_output_kwin_does_not_list_is_kept_with_the_note() {
        let effect = FakeEffect::new(true);
        let profiles = Profiles {
            profiles: vec![couch_profile(&["DP-9", "HDMI-A-1"])],
            ..Profiles::default()
        };
        let profile = &profiles.profiles[0];
        let picture = profile_picture(&profiles, profile, None, true);
        apply_levels_via(&effect, &picture.levels).unwrap();
        assert_eq!(
            effect.calls().last(),
            Some(&Call::SetLevels(
                1.0,
                vec![("DP-9".to_string(), 1.3), ("HDMI-A-1".to_string(), 1.3)]
            ))
        );

        let connected = outputs_from(effect.listed.clone());
        assert_eq!(
            profile_outputs_not_connected(&profiles, profile, &connected),
            [Note::OutputNotConnected {
                profile: "couch".into(),
                output: "DP-9".into()
            }]
        );
    }

    #[test]
    fn restoring_gives_each_output_its_own_desktop_level() {
        let effect = FakeEffect::new(true);
        let mut profiles = Profiles {
            default_saturation: 1.1,
            ..Profiles::default()
        };
        profiles.outputs.insert(
            "HDMI-A-1".into(),
            OutputConfig {
                default_saturation: Some(0.85),
            },
        );
        apply_levels_via(&effect, &levels(1.6, &[])).unwrap();
        apply_levels_via(&effect, &desktop_picture(&profiles, true).levels).unwrap();
        assert_eq!(
            effect.calls(),
            [
                Call::Load,
                Call::SetOne(1.6),
                Call::SetLevels(1.1, vec![("HDMI-A-1".to_string(), 0.85)])
            ]
        );
        assert_eq!(
            current_levels_via(&effect),
            Some(levels(1.1, &[("HDMI-A-1", 0.85)]))
        );
    }

    #[test]
    fn an_unloaded_effect_shows_every_output_unchanged() {
        let effect = FakeEffect::new(true);
        assert_eq!(
            current_levels_via(&effect),
            Some(Levels::uniform(Saturation::IDENTITY))
        );
        assert!(effect.calls().is_empty());
    }

    #[test]
    fn a_manual_change_to_one_output_keeps_the_others() {
        let effect = FakeEffect::new(true);
        apply_levels_via(&effect, &levels(1.0, &[("HDMI-A-1", 0.85)])).unwrap();

        apply_output_via(&effect, "DP-1", Saturation::new(1.5)).unwrap();

        assert_eq!(
            effect.calls().last(),
            Some(&Call::SetOutput("DP-1".into(), 1.5))
        );
        assert_eq!(
            current_levels_via(&effect),
            Some(levels(1.0, &[("DP-1", 1.5), ("HDMI-A-1", 0.85)]))
        );
    }

    #[test]
    fn a_manual_change_to_one_output_loads_the_effect_first() {
        let effect = FakeEffect::new(true);
        apply_output_via(&effect, "DP-1", Saturation::new(1.5)).unwrap();
        assert_eq!(
            effect.calls(),
            [Call::Load, Call::SetOutput("DP-1".into(), 1.5)]
        );
        assert_eq!(
            current_levels_via(&effect),
            Some(levels(1.0, &[("DP-1", 1.5)]))
        );
    }

    #[test]
    fn resetting_one_output_puts_only_it_back_to_the_base() {
        let effect = FakeEffect::new(true);
        apply_levels_via(&effect, &levels(1.2, &[("HDMI-A-1", 0.85), ("DP-1", 1.5)])).unwrap();

        reset_output_via(&effect, "DP-1").unwrap();

        assert_eq!(
            current_levels_via(&effect),
            Some(levels(1.2, &[("HDMI-A-1", 0.85)]))
        );
        assert_eq!(
            effect.calls().last(),
            Some(&Call::SetOutput("DP-1".into(), 1.2))
        );
    }

    #[test]
    fn the_last_output_back_to_unchanged_unloads_the_effect() {
        let effect = FakeEffect::new(true);
        apply_output_via(&effect, "DP-1", Saturation::new(1.5)).unwrap();
        reset_output_via(&effect, "DP-1").unwrap();
        assert_eq!(effect.calls().last(), Some(&Call::Unload));
        // Nothing loaded, nothing to reset.
        reset_output_via(&effect, "DP-1").unwrap();
        assert_eq!(effect.calls().last(), Some(&Call::Unload));
    }

    #[test]
    fn an_old_effect_refuses_one_output_instead_of_changing_every_screen() {
        let effect = FakeEffect::new(false);
        let err = apply_output_via(&effect, "DP-1", Saturation::new(1.5)).unwrap_err();
        assert!(matches!(err, BackendError::Unsupported(_)), "{err}");
        assert!(!effect
            .calls()
            .iter()
            .any(|call| matches!(call, Call::SetOne(_))));
    }

    #[test]
    fn the_three_effect_states_from_a_probe() {
        let new_effect = FakeEffect::new(true);
        let old_effect = FakeEffect::new(false);
        assert_eq!(
            effect_outputs_state(&probe_outputs(&new_effect), || Some(true)),
            EffectOutputs::PerOutput
        );
        assert_eq!(
            effect_outputs_state(&probe_outputs(&old_effect), || Some(true)),
            EffectOutputs::OlderLoaded {
                installed_is_newer: true
            }
        );
        for on_disk in [Some(false), None] {
            assert_eq!(
                effect_outputs_state(&probe_outputs(&old_effect), || on_disk),
                EffectOutputs::OlderLoaded {
                    installed_is_newer: false
                }
            );
        }
        let failed: Result<Option<BTreeMap<String, String>>, BackendError> =
            Err(BackendError::Apply("KWin refused to load".into()));
        assert_eq!(
            effect_outputs_state(&failed, || panic!(
                "the file is only read for an older effect"
            )),
            EffectOutputs::Unavailable
        );
    }

    #[test]
    fn each_state_says_why_and_what_to_do() {
        assert_eq!(EffectOutputs::PerOutput.explanation(), None);
        assert_eq!(
            EffectOutputs::OlderLoaded {
                installed_is_newer: true
            }
            .explanation(),
            Some(
                "KDE is still running the Satur8 effect from before the upgrade, so one value \
                 applies to every output. Log out and back in once to use a different value \
                 per output."
            )
        );
        assert_eq!(
            EffectOutputs::OlderLoaded {
                installed_is_newer: false
            }
            .explanation(),
            Some(
                "The installed Satur8 KWin effect is from before per-output support, so one \
                 value applies to every output. Reinstall Satur8 or run `satur8 repair`, then \
                 log out and back in once."
            )
        );
        assert!(EffectOutputs::Unavailable
            .explanation()
            .unwrap()
            .contains("`satur8 doctor`"));
    }

    #[test]
    fn the_effect_file_is_checked_for_the_per_output_calls() {
        assert!(repair::has_per_output_calls(
            b"\0qt_meta\0setSaturation\0setSaturations\0outputs\0"
        ));
        assert!(!repair::has_per_output_calls(
            b"\0qt_meta\0setSaturation\0saturation\0setLinearLight\0"
        ));
    }
}
