//! `satur8-daemon` - the always-on, event-driven trigger (PLAN.md M4/section 6).
//!
//! KWin doesn't broadcast the active window to third parties, so the companion
//! KWin script (`assets/kwin-script/`) forwards each activation to us by calling
//! our `WindowActivated` D-Bus method. We react *only* on those calls - there is
//! no polling and the daemon idles at effectively 0% CPU.
//!
//! On each activation we match the focused window's class against the profiles
//! file: a match applies that profile's saturation; focusing anything else
//! restores the desktop default. This complements the launch wrapper (M2) for
//! people who want satur8 to follow focus rather than wrap a launch command.

use std::time::Duration;

use anyhow::{Context, Result};
use satur8_core::{Backend, Output, Profiles, Saturation};
use satur8_kwin::KwinBackend;
use zbus::interface;

const SERVICE: &str = "org.satur8.Daemon";
const PATH: &str = "/org/satur8/Daemon";

fn all_outputs() -> Output {
    Output {
        id: "all".into(),
        human_name: "All outputs".into(),
    }
}

struct Daemon {
    profiles: Profiles,
    backend: Option<KwinBackend>,
    /// Name of the profile currently applied, if any.
    current: Option<String>,
}

impl Daemon {
    fn new() -> Daemon {
        let profiles = load_profiles();
        let backend = KwinBackend::detect();
        if backend.is_none() {
            eprintln!("satur8-daemon: warning - no KWin backend; will track focus but can't apply");
        }
        eprintln!(
            "satur8-daemon: ready, {} profile(s) loaded",
            profiles.profiles.len()
        );
        Daemon {
            profiles,
            backend,
            current: None,
        }
    }

    fn apply_saturation(&mut self, sat: Saturation) {
        if let Some(b) = self.backend.as_mut() {
            if let Err(e) = b.apply(&all_outputs(), sat) {
                eprintln!("satur8-daemon: apply failed: {e}");
            }
        }
    }

    fn restore_default(&mut self) {
        let def = self.profiles.default_saturation();
        if let Some(b) = self.backend.as_mut() {
            let result = if def.is_identity() {
                b.reset(&all_outputs())
            } else {
                b.apply(&all_outputs(), def)
            };
            if let Err(e) = result {
                eprintln!("satur8-daemon: restore failed: {e}");
            }
        }
    }

    fn react(&mut self, class: &str) {
        match self.profiles.match_window_class(class) {
            Some(profile) => {
                // Avoid redundant re-applies when focus stays on the same game.
                if self.current.as_deref() == Some(profile.name.as_str()) {
                    return;
                }
                let (name, sat) = (profile.name.clone(), profile.saturation());
                eprintln!("satur8-daemon: '{class}' -> profile '{name}' ({:.2})", sat.get());
                self.apply_saturation(sat);
                self.current = Some(name);
            }
            None => {
                if self.current.take().is_some() {
                    eprintln!("satur8-daemon: '{class}' has no profile -> restoring default");
                    self.restore_default();
                }
            }
        }
    }
}

#[interface(name = "org.satur8.Daemon")]
impl Daemon {
    /// Called by the KWin script on every window activation.
    fn window_activated(&mut self, class: String, _caption: String) {
        self.react(&class);
    }

    /// Re-read the profiles file from disk (after editing profiles).
    fn reload(&mut self) {
        self.profiles = load_profiles();
        eprintln!(
            "satur8-daemon: reloaded, {} profile(s)",
            self.profiles.profiles.len()
        );
    }

    /// The profile currently applied (empty string if none).
    #[zbus(property)]
    fn active_profile(&self) -> String {
        self.current.clone().unwrap_or_default()
    }
}

/// Load profiles from the same file the CLI uses.
fn load_profiles() -> Profiles {
    let path = profiles_path();
    match path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()) {
        Some(s) => Profiles::from_toml(&s).unwrap_or_default(),
        None => Profiles::default(),
    }
}

fn profiles_path() -> Option<std::path::PathBuf> {
    let dir = if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if xdg.is_empty() {
            std::path::PathBuf::from(std::env::var("HOME").ok()?).join(".config")
        } else {
            std::path::PathBuf::from(xdg)
        }
    } else {
        std::path::PathBuf::from(std::env::var("HOME").ok()?).join(".config")
    };
    Some(dir.join("satur8").join("profiles.toml"))
}

fn main() -> Result<()> {
    let daemon = Daemon::new();
    let _conn = zbus::blocking::connection::Builder::session()
        .context("connecting to the session bus")?
        .name(SERVICE)
        .context("claiming the org.satur8.Daemon bus name (already running?)")?
        .serve_at(PATH, daemon)
        .context("publishing the daemon interface")?
        .build()
        .context("starting the daemon service")?;

    eprintln!("satur8-daemon: listening on {SERVICE} {PATH}");
    // Event-driven: nothing to do but stay alive for incoming activations.
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}
