//! DRM/KMS CTM backend (B2) - the zero-cost ideal.
//!
//! The saturation matrix lives in exactly one place in hardware: the GPU
//! scanout block's Color Transformation Matrix (CTM). Setting it is a one-time
//! atomic commit; after that the GPU multiplies every output pixel by it during
//! scanout, for free. This is the gold standard.
//!
//! The catch is *who owns DRM master*: only on bare KMS / a TTY (no display
//! server) can a client own master and set the CTM directly, which is what this
//! backend does. On X11 the X server owns it (use the RandR `CTM` property
//! instead - a separate sub-backend), and on Wayland the compositor owns it
//! (use the KWin/GNOME/Hyprland backends). So `detect()` only claims this path
//! where we can actually drive it.
//!
//! Even where we cannot *set* the CTM (e.g. a Wayland box), [`probe_ctm`] can
//! still enumerate which CRTCs expose a CTM property - useful for diagnostics
//! and for answering "does this GPU honor DRM CTM?".

use std::collections::{BTreeSet, HashMap};
use std::fs::{File, OpenOptions};
use std::os::fd::{AsFd, BorrowedFd};

use anyhow::{anyhow, bail, Context, Result};
use drm::control::{
    atomic::AtomicModeReq, crtc, property, AtomicCommitFlags, Device as ControlDevice,
};
use drm::{ClientCapability, Device as BasicDevice};

use satur8_core::ctm::drm_ctm_blob_bytes;
use satur8_core::{
    Backend, BackendError, CostNote, Environment, Levels, Output, Saturation, SessionType,
};

/// A DRM device node we can talk modesetting to.
struct Card(File);

impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}
impl BasicDevice for Card {}
impl ControlDevice for Card {}

impl Card {
    fn open(path: &str) -> Result<Card> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .with_context(|| format!("opening {path}"))?;
        Ok(Card(file))
    }
}

/// One CRTC that supports a CTM, plus the property handle to set it.
struct CtmCrtc {
    crtc: crtc::Handle,
    ctm_prop: property::Handle,
}

pub struct DrmCtmBackend {
    card: Card,
    /// crtc id (u32) -> how to set its CTM.
    crtcs: HashMap<u32, CtmCrtc>,
    /// CRTCs this backend has left at a level other than 1.0. An inactive one
    /// is not reached by "all", so it is put back explicitly when a picture
    /// stops naming it.
    managed: BTreeSet<u32>,
}

impl DrmCtmBackend {
    /// Claim this backend only where we can own DRM master: bare KMS / TTY.
    /// (X11 -> RandR CTM, Wayland -> compositor backends.)
    pub fn detect() -> Option<DrmCtmBackend> {
        if Environment::detect().session != SessionType::Tty {
            return None;
        }
        DrmCtmBackend::open_first_capable().ok()
    }

    /// Open the card we should drive and enable the client capabilities atomic
    /// modesetting needs. On a multi-GPU box (e.g. an iGPU plus a discrete card)
    /// several nodes expose CTM-capable CRTCs but only one is actually driving
    /// the monitor, so prefer the first card with an *active* CTM CRTC and fall
    /// back to any CTM-capable card if none reports an active display.
    pub fn open_first_capable() -> Result<DrmCtmBackend> {
        let mut last_err = None;
        let mut fallback: Option<DrmCtmBackend> = None;
        for n in 0..16 {
            let path = format!("/dev/dri/card{n}");
            if !std::path::Path::new(&path).exists() {
                continue;
            }
            match DrmCtmBackend::open_path(&path) {
                Ok(backend) if !backend.crtcs.is_empty() => {
                    if backend.has_active_ctm_crtc() {
                        return Ok(backend);
                    }
                    fallback.get_or_insert(backend);
                }
                Ok(_) => {}
                Err(e) => last_err = Some(e),
            }
        }
        fallback
            .ok_or_else(|| last_err.unwrap_or_else(|| anyhow!("no CTM-capable DRM device found")))
    }

    /// Does any CTM-capable CRTC on this card currently drive a display?
    fn has_active_ctm_crtc(&self) -> bool {
        has_active_ctm_crtc_id(self.crtcs.keys().copied(), |id| {
            self.crtcs
                .get(&id)
                .map(|c| self.crtc_is_active(c.crtc))
                .unwrap_or(false)
        })
    }

    fn open_path(path: &str) -> Result<DrmCtmBackend> {
        let card = Card::open(path)?;
        // Atomic modesetting (and universal planes, its prerequisite) must be
        // requested per-fd before we can build atomic commits.
        card.set_client_capability(ClientCapability::UniversalPlanes, true)
            .with_context(|| format!("enabling universal planes on {path}"))?;
        card.set_client_capability(ClientCapability::Atomic, true)
            .with_context(|| format!("enabling atomic modesetting on {path}"))?;
        let crtcs = discover_ctm_crtcs(&card)?;
        Ok(DrmCtmBackend {
            card,
            crtcs,
            managed: BTreeSet::new(),
        })
    }

    fn set_crtc_ctm(&self, target: &CtmCrtc, saturation: Saturation) -> Result<()> {
        let bytes = drm_ctm_blob_bytes(saturation);
        let blob = self
            .card
            .create_property_blob(&bytes[..])
            .context("creating CTM property blob")?;
        let blob_id = u64::from(blob);
        let mut req = AtomicModeReq::new();
        req.add_property(target.crtc, target.ctm_prop, blob);
        // A CTM change does not require a modeset.
        let commit = self.card.atomic_commit(AtomicCommitFlags::empty(), req);
        let destroy = self.card.destroy_property_blob(blob_id);
        commit.context("atomic commit of CTM (needs DRM master - run from a TTY)")?;
        destroy.context("destroying CTM property blob")?;
        Ok(())
    }

    /// Is this CRTC currently driving a display (has a mode set)? Setting a CTM
    /// on a disabled CRTC is pointless and the kernel may reject the commit, so
    /// the "all outputs" path only touches active ones.
    fn crtc_is_active(&self, handle: crtc::Handle) -> bool {
        self.card
            .get_crtc(handle)
            .map(|info| info.mode().is_some())
            .unwrap_or(false)
    }

    /// Resolve the CTM CRTCs an `Output` refers to. The shared `"all"` sentinel
    /// (what `satur8 set`/`off` use with no explicit `--output`) fans out across
    /// every active CTM-capable CRTC; a numeric id targets exactly that CRTC.
    fn resolve_targets(&self, output: &Output) -> Result<Vec<&CtmCrtc>, BackendError> {
        let ids = resolve_target_ids(output, self.crtcs.keys().copied(), |id| {
            self.crtcs
                .get(&id)
                .map(|c| self.crtc_is_active(c.crtc))
                .unwrap_or(false)
        })?;
        ids.into_iter()
            .map(|id| {
                self.crtcs
                    .get(&id)
                    .ok_or_else(|| BackendError::Apply(format!("CRTC {id} has no CTM property")))
            })
            .collect()
    }
}

fn has_active_ctm_crtc_id<I, F>(ids: I, is_active: F) -> bool
where
    I: IntoIterator<Item = u32>,
    F: FnMut(u32) -> bool,
{
    ids.into_iter().any(is_active)
}

fn resolve_target_ids<I, F>(
    output: &Output,
    ids: I,
    mut is_active: F,
) -> Result<Vec<u32>, BackendError>
where
    I: IntoIterator<Item = u32>,
    F: FnMut(u32) -> bool,
{
    let mut ids: Vec<u32> = ids.into_iter().collect();
    ids.sort_unstable();
    ids.dedup();

    if output.id == "all" {
        let active: Vec<u32> = ids.iter().copied().filter(|id| is_active(*id)).collect();
        let targets = if active.is_empty() { ids } else { active };
        if targets.is_empty() {
            return Err(BackendError::Apply(
                "no CTM-capable CRTC available on this device".into(),
            ));
        }
        return Ok(targets);
    }

    let id: u32 = output
        .id
        .parse()
        .map_err(|_| BackendError::Apply(format!("bad CRTC id '{}'", output.id)))?;
    if ids.binary_search(&id).is_err() {
        return Err(BackendError::Apply(format!(
            "CRTC {id} has no CTM property"
        )));
    }
    Ok(vec![id])
}

fn apply_to_targets<T, I, G, F>(
    targets: I,
    saturation: Saturation,
    mut target_id: G,
    mut set_target_ctm: F,
) -> Result<(), BackendError>
where
    I: IntoIterator<Item = T>,
    G: FnMut(&T) -> u32,
    F: FnMut(T, Saturation) -> Result<()>,
{
    let mut failures = Vec::new();
    for target in targets {
        let id = target_id(&target);
        if let Err(error) = set_target_ctm(target, saturation) {
            failures.push(format!("CRTC {id}: {error:#}"));
        }
    }
    if !failures.is_empty() {
        return Err(BackendError::Apply(format!(
            "CTM commit failed for {}",
            failures.join("; ")
        )));
    }
    Ok(())
}

/// The CRTC and saturation each commit of `levels` sets, sorted by CRTC id.
///
/// Each named output that is a CTM-capable CRTC id gets its own level, active or
/// not, as a numeric `--output` always has. Every other CRTC that the `"all"`
/// sentinel reaches (the active ones, or all of them when none is active) gets
/// `base`. A name that is not a CTM-capable CRTC here, such as an unplugged
/// screen or a connector name, is skipped. Each `managed` CRTC (one this
/// backend left at another level) that the picture no longer names gets
/// `base` too, even when it is not active, so no stale level stays behind for
/// when it turns on; one that no longer exists on the device is skipped.
fn plan_levels<I, F>(
    levels: &Levels,
    ids: I,
    is_active: F,
    managed: &BTreeSet<u32>,
) -> Result<Vec<(u32, Saturation)>, BackendError>
where
    I: IntoIterator<Item = u32>,
    F: FnMut(u32) -> bool,
{
    let mut ids: Vec<u32> = ids.into_iter().collect();
    ids.sort_unstable();
    ids.dedup();
    let known = ids.clone();
    let named: Vec<(u32, Saturation)> = levels
        .outputs
        .iter()
        .filter_map(|(name, saturation)| {
            let id: u32 = name.parse().ok()?;
            ids.binary_search(&id).ok().map(|_| (id, *saturation))
        })
        .collect();
    let rest = resolve_target_ids(&satur8_core::all_outputs(), ids, is_active)?;
    let mut plan: Vec<(u32, Saturation)> = rest
        .into_iter()
        .filter(|id| !named.iter().any(|(named_id, _)| named_id == id))
        .map(|id| (id, levels.base))
        .collect();
    plan.extend(named);
    for &id in managed {
        if known.binary_search(&id).is_ok() && !plan.iter().any(|(planned, _)| *planned == id) {
            plan.push((id, levels.base));
        }
    }
    plan.sort_by_key(|(id, _)| *id);
    Ok(plan)
}

/// Record what was committed: a CRTC at 1.0 needs no more care, any other
/// level is remembered so a later picture can put it back.
fn remember(managed: &mut BTreeSet<u32>, committed: &[(u32, Saturation)]) {
    for &(id, saturation) in committed {
        if saturation.is_identity() {
            managed.remove(&id);
        } else {
            managed.insert(id);
        }
    }
}

/// Commit each planned level, trying every CRTC even when one fails.
fn apply_plan<F>(plan: Vec<(u32, Saturation)>, mut set_ctm: F) -> Result<(), BackendError>
where
    F: FnMut(u32, Saturation) -> Result<()>,
{
    let failures: Vec<String> = plan
        .into_iter()
        .filter_map(|(id, saturation)| {
            set_ctm(id, saturation)
                .err()
                .map(|error| format!("CRTC {id}: {error:#}"))
        })
        .collect();
    if !failures.is_empty() {
        return Err(BackendError::Apply(format!(
            "CTM commit failed for {}",
            failures.join("; ")
        )));
    }
    Ok(())
}

/// Walk every CRTC and keep the ones exposing a "CTM" property.
fn discover_ctm_crtcs(card: &Card) -> Result<HashMap<u32, CtmCrtc>> {
    let resources = card
        .resource_handles()
        .context("reading DRM resources (is this a modesetting node?)")?;
    let mut out = HashMap::new();
    for &crtc in resources.crtcs() {
        let props = match card.get_properties(crtc) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let (handles, _values) = props.as_props_and_values();
        for &ph in handles {
            if let Ok(info) = card.get_property(ph) {
                if info.name().to_str() == Ok("CTM") {
                    let id: u32 = crtc.into();
                    out.insert(id, CtmCrtc { crtc, ctm_prop: ph });
                    break;
                }
            }
        }
    }
    Ok(out)
}

impl Backend for DrmCtmBackend {
    fn name(&self) -> &'static str {
        "drm-ctm"
    }

    fn cost(&self) -> CostNote {
        CostNote::ZeroCost
    }

    fn outputs(&self) -> Vec<Output> {
        let mut outs: Vec<Output> = self
            .crtcs
            .keys()
            .map(|id| Output {
                id: id.to_string(),
                human_name: format!("CRTC {id}"),
            })
            .collect();
        outs.sort_by(|a, b| a.id.cmp(&b.id));
        outs
    }

    fn apply(&mut self, output: &Output, saturation: Saturation) -> Result<(), BackendError> {
        // Every output, `reset` included, goes through the picture so a CRTC
        // left at another level while inactive is put back too.
        if output.id == satur8_core::levels::ALL_OUTPUTS {
            return self.apply_levels(&Levels::uniform(saturation));
        }
        let targets = self.resolve_targets(output)?;
        let ids: Vec<u32> = targets.iter().map(|target| target.crtc.into()).collect();
        apply_to_targets(
            targets,
            saturation,
            |target| target.crtc.into(),
            |target, saturation| self.set_crtc_ctm(target, saturation),
        )?;
        let committed: Vec<(u32, Saturation)> =
            ids.into_iter().map(|id| (id, saturation)).collect();
        remember(&mut self.managed, &committed);
        Ok(())
    }

    fn reset(&mut self, output: &Output) -> Result<(), BackendError> {
        self.apply(output, Saturation::IDENTITY)
    }

    /// Each CRTC has its own CTM, so outputs can differ.
    fn per_output(&self) -> bool {
        true
    }

    fn apply_levels(&mut self, levels: &Levels) -> Result<(), BackendError> {
        let crtcs = &self.crtcs;
        self.managed.retain(|id| crtcs.contains_key(id));
        let plan = plan_levels(
            levels,
            self.crtcs.keys().copied(),
            |id| {
                self.crtcs
                    .get(&id)
                    .map(|c| self.crtc_is_active(c.crtc))
                    .unwrap_or(false)
            },
            &self.managed,
        )?;
        let mut committed = Vec::new();
        let result = apply_plan(plan, |id, saturation| {
            let result = match self.crtcs.get(&id) {
                Some(target) => self.set_crtc_ctm(target, saturation),
                None => bail!("CRTC {id} has no CTM property"),
            };
            if result.is_ok() {
                committed.push((id, saturation));
            }
            result
        });
        remember(&mut self.managed, &committed);
        result
    }
}

/// Read-only diagnostic: list CRTCs that expose a CTM property. This only opens
/// the node and queries properties - it never tries to *take* DRM master, so it
/// is safe to call under a running compositor (it will not disturb the display).
/// Whether we could actually set the CTM is a function of the session (TTY yes,
/// X11/Wayland no - the server owns master), which the caller already knows.
pub fn probe_ctm() -> Result<Vec<String>> {
    let mut report = Vec::new();
    let mut found_any = false;
    for n in 0..16 {
        let path = format!("/dev/dri/card{n}");
        if !std::path::Path::new(&path).exists() {
            continue;
        }
        let card = match Card::open(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let crtcs = match discover_ctm_crtcs(&card) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if crtcs.is_empty() {
            continue;
        }
        found_any = true;
        report.push(format!("{path}: {} CTM-capable CRTC(s)", crtcs.len()));
    }
    if !found_any {
        bail!("no CTM-capable DRM CRTCs found on this system");
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(id: &str) -> Output {
        Output {
            id: id.into(),
            human_name: id.into(),
        }
    }

    #[test]
    fn has_active_ctm_crtc_id_reports_true_when_any_crtc_is_active() {
        let ids = [31, 42, 73];

        assert!(has_active_ctm_crtc_id(ids, |id| id == 42));
        assert!(!has_active_ctm_crtc_id(ids, |_| false));
    }

    #[test]
    fn resolve_targets_all_fans_out_to_active_crtcs_only() {
        let targets = resolve_target_ids(&output("all"), [42, 31, 73], |id| id != 42).unwrap();

        assert_eq!(targets, vec![31, 73]);
    }

    #[test]
    fn resolve_targets_all_falls_back_to_every_ctm_crtc_when_none_report_active() {
        let targets = resolve_target_ids(&output("all"), [42, 31, 73], |_| false).unwrap();

        assert_eq!(targets, vec![31, 42, 73]);
    }

    #[test]
    fn resolve_targets_all_errors_when_no_ctm_crtcs_exist() {
        let err = resolve_target_ids(&output("all"), [], |_| false).unwrap_err();

        assert!(err.to_string().contains("no CTM-capable CRTC available"));
    }

    #[test]
    fn resolve_targets_numeric_output_selects_that_crtc() {
        let targets = resolve_target_ids(&output("42"), [31, 42, 73], |_| false).unwrap();

        assert_eq!(targets, vec![42]);
    }

    #[test]
    fn resolve_targets_rejects_non_numeric_output_id() {
        let err = resolve_target_ids(&output("DP-1"), [31, 42, 73], |_| false).unwrap_err();

        assert!(err.to_string().contains("bad CRTC id 'DP-1'"));
    }

    #[test]
    fn resolve_targets_rejects_crtc_without_ctm_property() {
        let err = resolve_target_ids(&output("99"), [31, 42, 73], |_| false).unwrap_err();

        assert!(err.to_string().contains("CRTC 99 has no CTM property"));
    }

    #[test]
    fn apply_to_targets_errors_when_any_target_rejects_the_ctm() {
        let mut attempted = Vec::new();

        let err = apply_to_targets(
            [31, 42, 73],
            Saturation::new(1.5),
            |id| *id,
            |id, saturation| {
                attempted.push((id, saturation.get()));
                if id == 42 {
                    anyhow::bail!("simulated commit failure");
                }
                Ok(())
            },
        )
        .unwrap_err();

        assert_eq!(attempted, vec![(31, 1.5), (42, 1.5), (73, 1.5)]);
        assert!(err.to_string().contains("CRTC 42"));
        assert!(err.to_string().contains("simulated commit failure"));
    }

    #[test]
    fn apply_to_targets_errors_when_every_target_rejects_the_ctm() {
        let err = apply_to_targets(
            [31, 42],
            Saturation::new(1.5),
            |id| *id,
            |id, _| anyhow::bail!("simulated commit failure on {id}"),
        )
        .unwrap_err();

        assert!(err.to_string().contains("CRTC 31"));
        assert!(err.to_string().contains("CRTC 42"));
        assert!(err.to_string().contains("simulated commit failure on 31"));
        assert!(err.to_string().contains("simulated commit failure on 42"));
    }

    fn levels(base: f32, outputs: &[(&str, f32)]) -> Levels {
        Levels::new(
            Saturation::new(base),
            outputs
                .iter()
                .map(|(name, value)| (name.to_string(), Saturation::new(*value))),
        )
    }

    fn plan(levels: &Levels, active: &[u32]) -> Vec<(u32, f32)> {
        plan_with(levels, active, &BTreeSet::new())
    }

    fn plan_with(levels: &Levels, active: &[u32], managed: &BTreeSet<u32>) -> Vec<(u32, f32)> {
        plan_levels(levels, [42, 31, 73], |id| active.contains(&id), managed)
            .unwrap()
            .into_iter()
            .map(|(id, saturation)| (id, saturation.get()))
            .collect()
    }

    /// The same commits as `apply` on `"all"`: every active CRTC, or every
    /// CTM CRTC when none is active.
    #[test]
    fn a_uniform_picture_sets_the_crtcs_that_all_reaches() {
        assert_eq!(plan(&levels(1.5, &[]), &[31, 73]), [(31, 1.5), (73, 1.5)]);
        assert_eq!(
            plan(&levels(1.5, &[]), &[]),
            [(31, 1.5), (42, 1.5), (73, 1.5)]
        );
    }

    /// Pins the v0.4.1 behavior for a profile that names CRTCs: the profile's
    /// value reaches only the named ones. The others show the desktop level.
    #[test]
    fn a_profile_naming_crtcs_changes_only_those_crtcs() {
        assert_eq!(
            plan(&levels(1.0, &[("42", 1.6)]), &[31, 42, 73]),
            [(31, 1.0), (42, 1.6), (73, 1.0)]
        );
        assert_eq!(
            plan(&levels(1.0, &[("31", 1.6), ("73", 1.6)]), &[31, 42, 73]),
            [(31, 1.6), (42, 1.0), (73, 1.6)]
        );
    }

    #[test]
    fn a_named_crtc_is_set_even_when_it_is_not_active() {
        assert_eq!(
            plan(&levels(1.0, &[("42", 1.6)]), &[31]),
            [(31, 1.0), (42, 1.6)]
        );
    }

    #[test]
    fn names_that_are_not_ctm_crtcs_here_are_skipped() {
        assert_eq!(
            plan(&levels(1.0, &[("99", 1.6), ("DP-1", 1.6)]), &[31, 42, 73]),
            [(31, 1.0), (42, 1.0), (73, 1.0)]
        );
    }

    #[test]
    fn each_crtc_gets_its_own_desktop_level_back() {
        assert_eq!(
            plan(&levels(1.25, &[("73", 0.85)]), &[31, 42, 73]),
            [(31, 1.25), (42, 1.25), (73, 0.85)]
        );
    }

    #[test]
    fn a_picture_with_no_ctm_crtcs_is_an_error() {
        let err = plan_levels(&levels(1.5, &[]), [], |_| false, &BTreeSet::new()).unwrap_err();
        assert!(err.to_string().contains("no CTM-capable CRTC available"));
    }

    #[test]
    fn apply_plan_tries_every_crtc_and_names_the_failures() {
        let mut attempted = Vec::new();
        let err = apply_plan(
            vec![(31, Saturation::new(1.0)), (42, Saturation::new(1.6))],
            |id, saturation| {
                attempted.push((id, saturation.get()));
                if id == 31 {
                    anyhow::bail!("simulated commit failure");
                }
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(attempted, vec![(31, 1.0), (42, 1.6)]);
        assert!(err
            .to_string()
            .contains("CRTC 31: simulated commit failure"));
    }

    /// The review finding's sequence: a profile names an inactive CRTC, then
    /// the next picture no longer names it. It must be put back to the base,
    /// although "all" does not reach it.
    #[test]
    fn a_crtc_left_at_another_level_while_inactive_is_put_back() {
        let mut managed = BTreeSet::new();
        let first = plan_levels(
            &levels(1.0, &[("42", 1.6)]),
            [31, 42, 73],
            |id| id == 31,
            &managed,
        )
        .unwrap();
        remember(&mut managed, &first);
        assert!(managed.contains(&42));

        assert_eq!(
            plan_with(&levels(1.25, &[]), &[31], &managed),
            [(31, 1.25), (42, 1.25)]
        );
    }

    #[test]
    fn a_remembered_crtc_that_is_gone_from_the_device_is_skipped() {
        let managed = BTreeSet::from([99]);
        assert_eq!(plan_with(&levels(1.0, &[]), &[31], &managed), [(31, 1.0)]);
    }

    #[test]
    fn an_unchanged_picture_clears_what_is_remembered() {
        let mut managed = BTreeSet::from([42, 73]);
        let plan = plan_levels(&levels(1.0, &[]), [31, 42, 73], |id| id == 31, &managed).unwrap();
        assert_eq!(
            plan.iter()
                .map(|(id, s)| (*id, s.get()))
                .collect::<Vec<_>>(),
            [(31, 1.0), (42, 1.0), (73, 1.0)]
        );
        remember(&mut managed, &plan);
        assert!(managed.is_empty());
    }

    #[test]
    fn remember_keeps_only_crtcs_at_another_level() {
        let mut managed = BTreeSet::new();
        remember(
            &mut managed,
            &[(31, Saturation::new(1.5)), (42, Saturation::IDENTITY)],
        );
        assert_eq!(managed, BTreeSet::from([31]));
    }
}
