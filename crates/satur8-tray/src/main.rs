//! `satur8-tray` - a StatusNotifierItem tray for quick saturation control.
//!
//! A lightweight system-tray control (the GUI half of M6): toggle satur8 and
//! pick a saturation preset without touching the terminal. It drives the same
//! backends as the CLI.
//!
//! Note: a tray needs a StatusNotifierItem host (the desktop panel) to display,
//! so it only does something useful inside a graphical session with a tray.

use std::sync::mpsc::{self, Sender};
use std::thread;

use ksni::menu::{MenuItem, StandardItem};
use ksni::Tray;

use satur8_backend::{all_outputs, install_signal_handler, select_backend, RestoreGuard};
use satur8_core::{BackendError, Levels, Saturation};

/// Work items handed from menu callbacks to the apply thread, so the menu never
/// blocks on D-Bus.
enum Action {
    Set(f32),
    Off,
    Quit,
}

fn handle_action(guard: &mut RestoreGuard, action: Action) -> Result<bool, BackendError> {
    match action {
        Action::Set(saturation) => {
            guard
                .backend_mut()
                .apply(&all_outputs(), Saturation::new(saturation))?;
            guard.arm();
            Ok(false)
        }
        Action::Off => {
            guard.restore_now()?;
            Ok(false)
        }
        Action::Quit => {
            guard.restore_now()?;
            Ok(true)
        }
    }
}

/// Run the apply loop on its own thread, holding the backend.
fn spawn_apply_thread() -> Sender<Action> {
    let (tx, rx) = mpsc::channel::<Action>();
    thread::spawn(move || {
        let mut guard = select_backend()
            .map(|backend| RestoreGuard::inactive(backend, Levels::uniform(Saturation::IDENTITY)))
            .map_err(|error| {
                eprintln!("satur8-tray: {error}");
                error
            });
        for action in rx {
            if matches!(action, Action::Quit) && guard.is_err() {
                std::process::exit(0);
            }
            let Ok(guard) = guard.as_mut() else { continue };
            match handle_action(guard, action) {
                Ok(true) => std::process::exit(0),
                Ok(false) => {}
                Err(error) => eprintln!("satur8-tray: {error}"),
            }
        }
    });
    tx
}

struct Satur8Tray {
    on: bool,
    saturation: f32,
    tx: Sender<Action>,
}

impl Satur8Tray {
    fn preset(&self, label: &str, value: f32) -> MenuItem<Self> {
        StandardItem {
            label: label.into(),
            activate: Box::new(move |t: &mut Satur8Tray| {
                t.on = true;
                t.saturation = value;
                let _ = t.tx.send(Action::Set(value));
            }),
            ..Default::default()
        }
        .into()
    }
}

impl Tray for Satur8Tray {
    fn id(&self) -> String {
        "satur8".into()
    }

    fn title(&self) -> String {
        "Satur8".into()
    }

    fn icon_name(&self) -> String {
        "preferences-desktop-color".into()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Satur8".into(),
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
                    format!("Satur8: on ({:.2})", self.saturation)
                } else {
                    "Satur8: off".into()
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
                activate: Box::new(|t: &mut Satur8Tray| {
                    t.on = false;
                    let _ = t.tx.send(Action::Off);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|t: &mut Satur8Tray| {
                    let _ = t.tx.send(Action::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

fn main() -> anyhow::Result<()> {
    use ksni::blocking::TrayMethods;

    let tx = spawn_apply_thread();
    let signal_tx = tx.clone();
    let _signal_handler = install_signal_handler(move |_| {
        let _ = signal_tx.send(Action::Quit);
    })?;
    let tray = Satur8Tray {
        on: false,
        saturation: 1.5,
        tx,
    };
    let _handle = tray
        .spawn()
        .map_err(|e| anyhow::anyhow!("couldn't register tray (no StatusNotifier host?): {e}"))?;

    eprintln!("satur8-tray: running. Use the tray icon to toggle satur8.");
    // Keep the process alive; the tray service runs in the background.
    loop {
        thread::park();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use satur8_core::{Backend, BackendError, CostNote, Output};

    struct FakeBackend(Arc<Mutex<Vec<&'static str>>>);

    impl Backend for FakeBackend {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn cost(&self) -> CostNote {
            CostNote::ZeroCost
        }
        fn outputs(&self) -> Vec<Output> {
            vec![all_outputs()]
        }
        fn apply(&mut self, _: &Output, _: Saturation) -> Result<(), BackendError> {
            self.0.lock().unwrap().push("apply");
            Ok(())
        }
        fn reset(&mut self, _: &Output) -> Result<(), BackendError> {
            self.0.lock().unwrap().push("reset");
            Ok(())
        }
    }

    #[test]
    fn quit_restores_saturation_before_exiting() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let backend = Box::new(FakeBackend(calls.clone()));
        let mut guard =
            satur8_backend::RestoreGuard::inactive(backend, Levels::uniform(Saturation::IDENTITY));

        assert!(!handle_action(&mut guard, Action::Set(1.5)).unwrap());
        assert!(handle_action(&mut guard, Action::Quit).unwrap());
        assert_eq!(*calls.lock().unwrap(), ["apply", "reset"]);
    }
}
