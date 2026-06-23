//! `vibrance-tray` - a StatusNotifierItem tray for quick saturation control.
//!
//! A lightweight system-tray control (the GUI half of M6): toggle vibrance and
//! pick a saturation preset without touching the terminal. It drives the same
//! backends as the CLI.
//!
//! Note: a tray needs a StatusNotifierItem host (the desktop panel) to display,
//! so it only does something useful inside a graphical session with a tray.

use std::sync::mpsc::{self, Sender};
use std::thread;

use ksni::menu::{MenuItem, StandardItem};
use ksni::Tray;

use vibrance_core::{Backend, Output, Saturation};

/// Work items handed from menu callbacks to the apply thread, so the menu never
/// blocks on D-Bus.
enum Action {
    Set(f32),
    Off,
}

fn all_outputs() -> Output {
    Output {
        id: "all".into(),
        human_name: "All outputs".into(),
    }
}

/// Minimal backend selector (mirrors the CLI's order) so the tray is standalone.
fn select_backend() -> Option<Box<dyn Backend>> {
    use vibrance_drm_ctm::DrmCtmBackend;
    use vibrance_gnome::GnomeBackend;
    use vibrance_hyprland::HyprlandBackend;
    use vibrance_kwin::KwinBackend;
    use vibrance_nv_control::NvControlBackend;

    if let Some(b) = KwinBackend::detect() {
        return Some(Box::new(b));
    }
    if let Some(b) = GnomeBackend::detect() {
        return Some(Box::new(b));
    }
    if let Some(b) = HyprlandBackend::detect() {
        return Some(Box::new(b));
    }
    if let Some(b) = NvControlBackend::detect() {
        return Some(Box::new(b));
    }
    if let Some(b) = DrmCtmBackend::detect() {
        return Some(Box::new(b));
    }
    None
}

/// Run the apply loop on its own thread, holding the backend.
fn spawn_apply_thread() -> Sender<Action> {
    let (tx, rx) = mpsc::channel::<Action>();
    thread::spawn(move || {
        let mut backend = select_backend();
        if backend.is_none() {
            eprintln!("vibrance-tray: no usable backend in this session");
        }
        for action in rx {
            let Some(b) = backend.as_mut() else { continue };
            let result = match action {
                Action::Set(s) => b.apply(&all_outputs(), Saturation::new(s)),
                Action::Off => b.reset(&all_outputs()),
            };
            if let Err(e) = result {
                eprintln!("vibrance-tray: {e}");
            }
        }
    });
    tx
}

struct VibranceTray {
    on: bool,
    saturation: f32,
    tx: Sender<Action>,
}

impl VibranceTray {
    fn preset(&self, label: &str, value: f32) -> MenuItem<Self> {
        StandardItem {
            label: label.into(),
            activate: Box::new(move |t: &mut VibranceTray| {
                t.on = true;
                t.saturation = value;
                let _ = t.tx.send(Action::Set(value));
            }),
            ..Default::default()
        }
        .into()
    }
}

impl Tray for VibranceTray {
    fn id(&self) -> String {
        "vibrance".into()
    }

    fn title(&self) -> String {
        "Vibrance".into()
    }

    fn icon_name(&self) -> String {
        "preferences-desktop-color".into()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Vibrance".into(),
            description: if self.on {
                format!("On - saturation {:.2}", self.saturation)
            } else {
                "Off".into()
            },
            icon_name: "preferences-desktop-color".into(),
            icon_pixmap: Vec::new(),
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: if self.on {
                    format!("Vibrance: on ({:.2})", self.saturation)
                } else {
                    "Vibrance: off".into()
                },
                enabled: false,
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            self.preset("Subtle  (1.2)", 1.2),
            self.preset("Vivid   (1.5)", 1.5),
            self.preset("Intense (2.0)", 2.0),
            MenuItem::Separator,
            StandardItem {
                label: "Off".into(),
                activate: Box::new(|t: &mut VibranceTray| {
                    t.on = false;
                    let _ = t.tx.send(Action::Off);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|_| std::process::exit(0)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

fn main() -> anyhow::Result<()> {
    use ksni::blocking::TrayMethods;

    let tx = spawn_apply_thread();
    let tray = VibranceTray {
        on: false,
        saturation: 1.5,
        tx,
    };
    let _handle = tray
        .spawn()
        .map_err(|e| anyhow::anyhow!("couldn't register tray (no StatusNotifier host?): {e}"))?;

    eprintln!("vibrance-tray: running. Use the tray icon to toggle vibrance.");
    // Keep the process alive; the tray service runs in the background.
    loop {
        thread::park();
    }
}
