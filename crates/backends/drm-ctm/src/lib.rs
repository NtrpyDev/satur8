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

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::os::fd::{AsFd, BorrowedFd};

use anyhow::{anyhow, bail, Context, Result};
use drm::control::{
    atomic::AtomicModeReq, crtc, property, AtomicCommitFlags, Device as ControlDevice,
};
use drm::{ClientCapability, Device as BasicDevice};

use satur8_core::ctm::drm_ctm_blob_bytes;
use satur8_core::{
    Backend, BackendError, CostNote, Environment, Output, Saturation, SessionType,
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

    /// Open the first card that exposes a CTM-capable CRTC and enable the client
    /// capabilities atomic modesetting needs.
    pub fn open_first_capable() -> Result<DrmCtmBackend> {
        let mut last_err = None;
        for n in 0..16 {
            let path = format!("/dev/dri/card{n}");
            if !std::path::Path::new(&path).exists() {
                continue;
            }
            match DrmCtmBackend::open_path(&path) {
                Ok(backend) if !backend.crtcs.is_empty() => return Ok(backend),
                Ok(_) => {}
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow!("no CTM-capable DRM device found")))
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
        Ok(DrmCtmBackend { card, crtcs })
    }

    fn set_crtc_ctm(&self, target: &CtmCrtc, saturation: Saturation) -> Result<()> {
        let bytes = drm_ctm_blob_bytes(saturation);
        let blob = self
            .card
            .create_property_blob(&bytes[..])
            .context("creating CTM property blob")?;
        let mut req = AtomicModeReq::new();
        req.add_property(target.crtc, target.ctm_prop, blob);
        // A CTM change does not require a modeset.
        self.card
            .atomic_commit(AtomicCommitFlags::empty(), req)
            .context("atomic commit of CTM (needs DRM master - run from a TTY)")?;
        Ok(())
    }
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
        let id: u32 = output
            .id
            .parse()
            .map_err(|_| BackendError::Apply(format!("bad CRTC id '{}'", output.id)))?;
        let target = self
            .crtcs
            .get(&id)
            .ok_or_else(|| BackendError::Apply(format!("CRTC {id} has no CTM property")))?;
        self.set_crtc_ctm(target, saturation)
            .map_err(|e| BackendError::Apply(format!("{e:#}")))
    }

    fn reset(&mut self, output: &Output) -> Result<(), BackendError> {
        self.apply(output, Saturation::IDENTITY)
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
