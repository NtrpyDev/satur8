//! GNOME Shell backend (B4).
//!
//! Mutter exposes no client CTM API, but our GNOME Shell extension
//! (`assets/gnome-extension/`) applies a saturation shader at the shell level
//! and publishes `org.satur8.GnomeShell`. This backend just drives that over
//! D-Bus - the same shape as the KWin backend. Because it's a compositor shader,
//! it is GPU-agnostic and works on NVIDIA Wayland too.
//!
//! `detect()` requires the extension's service to be on the bus, so if the user
//! hasn't enabled it we fall through cleanly. Untested on the dev box (no GNOME).

use satur8_core::{
    Backend, BackendError, CostNote, Desktop, Environment, Output, Saturation, SessionType,
};
use zbus::blocking::Connection;

const SERVICE: &str = "org.satur8.GnomeShell";
const PATH: &str = "/org/satur8/GnomeShell";
const IFACE: &str = "org.satur8.GnomeShell";

pub struct GnomeBackend {
    conn: Connection,
}

impl GnomeBackend {
    pub fn detect() -> Option<GnomeBackend> {
        let env = Environment::detect();
        if env.session != SessionType::Wayland || env.desktop != Desktop::Gnome {
            return None;
        }
        let conn = Connection::session().ok()?;
        let backend = GnomeBackend { conn };
        // Only usable if the extension is enabled and serving its interface.
        backend.current_saturation().ok()?;
        Some(backend)
    }

    pub fn current_saturation(&self) -> Result<Saturation, BackendError> {
        let reply = self
            .conn
            .call_method(
                Some(SERVICE),
                PATH,
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &(IFACE, "Saturation"),
            )
            .map_err(err)?;
        let body = reply.body();
        let v: zbus::zvariant::Value = body.deserialize().map_err(err)?;
        let s = f64::try_from(v).map_err(|_| BackendError::Apply("bad Saturation value".into()))?;
        Saturation::try_new(s as f32)
            .map_err(|error| BackendError::Apply(format!("bad Saturation value: {error}")))
    }

    fn call(&self, method: &str, sat: Option<Saturation>) -> Result<(), BackendError> {
        let res = match sat {
            Some(s) => {
                self.conn
                    .call_method(Some(SERVICE), PATH, Some(IFACE), method, &(s.get() as f64))
            }
            None => self
                .conn
                .call_method(Some(SERVICE), PATH, Some(IFACE), method, &()),
        };
        res.map(|_| ()).map_err(err)
    }
}

impl Backend for GnomeBackend {
    fn name(&self) -> &'static str {
        "gnome-shell"
    }

    fn cost(&self) -> CostNote {
        CostNote::CompositorShaderPass
    }

    fn outputs(&self) -> Vec<Output> {
        vec![Output {
            id: "all".into(),
            human_name: "All outputs".into(),
        }]
    }

    fn apply(&mut self, _output: &Output, saturation: Saturation) -> Result<(), BackendError> {
        self.call("SetSaturation", Some(saturation))
    }

    fn reset(&mut self, _output: &Output) -> Result<(), BackendError> {
        self.call("Reset", None)
    }
}

fn err(e: zbus::Error) -> BackendError {
    BackendError::Apply(e.to_string())
}
