//! `vibrance-gui` - a simple, clean desktop app for per-game vibrance.
//!
//! Pick a running game, set how vivid it should be, done. It writes the same
//! `profiles.toml` the CLI/daemon read, so a profile added here is what
//! `vibrance run --profile <name>` (your Steam launch option) uses.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod proc;

use std::path::PathBuf;

use eframe::egui;
use vibrance_core::{Backend, MatchRule, Output, Profile, Profiles, Saturation};

/// Saturation a freshly-added game gets; tweak per game afterwards.
const DEFAULT_SAT: f32 = 1.4;
/// Game-useful range. 1.0 = normal, higher = more vivid.
const SAT_RANGE: std::ops::RangeInclusive<f32> = 1.0..=3.0;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([440.0, 560.0])
            .with_min_inner_size([360.0, 360.0])
            .with_title("Vibrance"),
        ..Default::default()
    };
    eframe::run_native(
        "Vibrance",
        options,
        Box::new(|cc| {
            setup_style(&cc.egui_ctx);
            Ok(Box::new(VibranceApp::new()))
        }),
    )
}

fn setup_style(ctx: &egui::Context) {
    let mut style = (*ctx.global_style()).clone();
    style.visuals = egui::Visuals::light();
    let accent = egui::Color32::from_rgb(0, 158, 138); // teal
    style.visuals.selection.bg_fill = accent;
    style.visuals.hyperlink_color = accent;
    style.visuals.widgets.hovered.bg_stroke.color = accent;
    style.visuals.panel_fill = egui::Color32::from_rgb(247, 248, 250);
    style.spacing.item_spacing = egui::vec2(8.0, 10.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.slider_width = 150.0;
    ctx.set_global_style(style);
}

struct VibranceApp {
    backend: Option<Box<dyn Backend>>,
    backend_name: String,
    profiles: Profiles,
    running: Vec<String>,
    picked: Option<String>,
    status: String,
    previewing: bool,
}

impl VibranceApp {
    fn new() -> VibranceApp {
        let backend = select_backend();
        let backend_name = backend
            .as_ref()
            .map(|b| b.name().to_string())
            .unwrap_or_else(|| "none".into());
        VibranceApp {
            backend,
            backend_name,
            profiles: load_profiles(),
            running: proc::running_executables(),
            picked: None,
            status: "Ready.".into(),
            previewing: false,
        }
    }

    fn outputs() -> Output {
        Output { id: "all".into(), human_name: "All outputs".into() }
    }

    fn preview(&mut self, sat: f32) {
        let Some(b) = self.backend.as_mut() else {
            self.status = "No supported backend in this session.".into();
            return;
        };
        match b.apply(&Self::outputs(), Saturation::new(sat)) {
            Ok(()) => {
                self.previewing = true;
                self.status = format!("Previewing {sat:.2}×. Click Stop to restore.");
            }
            Err(e) => self.status = format!("Couldn't apply: {e}"),
        }
    }

    fn stop_preview(&mut self) {
        if let Some(b) = self.backend.as_mut() {
            let _ = b.reset(&Self::outputs());
        }
        self.previewing = false;
        self.status = "Restored.".into();
    }

    fn save(&mut self) {
        if let Err(e) = save_profiles(&self.profiles) {
            self.status = format!("Save failed: {e}");
        }
    }
}

impl eframe::App for VibranceApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.heading("Vibrance");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let ok = self.backend.is_some();
                let (dot, color) = if ok {
                    ("●", egui::Color32::from_rgb(0, 158, 138))
                } else {
                    ("●", egui::Color32::from_rgb(200, 80, 80))
                };
                ui.label(egui::RichText::new(format!("{dot} {}", self.backend_name)).color(color));
            });
        });
        ui.label(
            egui::RichText::new("Per-game saturation - applied in the compositor, never in the game.")
                .weak()
                .small(),
        );
        ui.add_space(6.0);

        // ---- Games ----
        let mut to_delete: Option<usize> = None;
        let mut to_preview: Option<f32> = None;
        let mut dirty = false;

        egui::Frame::group(ui.style())
            .fill(egui::Color32::WHITE)
            .corner_radius(8.0)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(egui::RichText::new("GAMES").small().strong().weak());
                ui.add_space(2.0);

                if self.profiles.profiles.is_empty() {
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("No games yet. Add one below.").weak());
                    ui.add_space(6.0);
                }

                egui::ScrollArea::vertical()
                    .max_height(240.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (i, p) in self.profiles.profiles.iter_mut().enumerate() {
                            ui.horizontal(|ui| {
                                ui.add_sized(
                                    [110.0, 20.0],
                                    egui::Label::new(egui::RichText::new(&p.name).strong())
                                        .truncate(),
                                );
                                dirty |= ui
                                    .add(
                                        egui::Slider::new(&mut p.saturation, SAT_RANGE)
                                            .suffix("×")
                                            .fixed_decimals(2)
                                            .trailing_fill(true),
                                    )
                                    .changed();
                                if ui.button("Preview").clicked() {
                                    to_preview = Some(p.saturation);
                                }
                                if ui
                                    .button(egui::RichText::new("✕").color(egui::Color32::GRAY))
                                    .on_hover_text("Remove")
                                    .clicked()
                                {
                                    to_delete = Some(i);
                                }
                            });
                        }
                    });
            });

        if let Some(s) = to_preview {
            self.preview(s);
        }
        if let Some(i) = to_delete {
            self.profiles.profiles.remove(i);
            self.save();
            self.status = "Removed.".into();
        } else if dirty {
            self.save();
        }

        ui.add_space(8.0);

        // ---- Add a game ----
        egui::Frame::group(ui.style())
            .fill(egui::Color32::WHITE)
            .corner_radius(8.0)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(egui::RichText::new("ADD A GAME").small().strong().weak());
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("running")
                        .selected_text(
                            self.picked.clone().unwrap_or_else(|| "Select a running program".into()),
                        )
                        .width(230.0)
                        .show_ui(ui, |ui| {
                            for name in &self.running {
                                ui.selectable_value(&mut self.picked, Some(name.clone()), name);
                            }
                        });
                    if ui.button("⟳").on_hover_text("Refresh list").clicked() {
                        self.running = proc::running_executables();
                    }
                    let can_add = self.picked.is_some();
                    if ui.add_enabled(can_add, egui::Button::new("Add")).clicked() {
                        if let Some(exe) = self.picked.clone() {
                            add_or_update(&mut self.profiles, &exe, DEFAULT_SAT);
                            self.save();
                            self.status = format!("Added {exe}.");
                            self.picked = None;
                        }
                    }
                });
            });

        // ---- Footer ----
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if self.previewing {
                if ui.button("Stop preview").clicked() {
                    self.stop_preview();
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(&self.status).weak().small());
            });
        });
    }
}

/// Add a game profile named after the exe (or bump an existing one).
fn add_or_update(profiles: &mut Profiles, exe: &str, saturation: f32) {
    let name = exe
        .trim_end_matches(".exe")
        .trim_end_matches(".x86_64")
        .to_string();
    if let Some(p) = profiles
        .profiles
        .iter_mut()
        .find(|p| p.match_rule.exe.as_deref() == Some(exe) || p.name == name)
    {
        p.saturation = saturation;
        p.match_rule.exe = Some(exe.to_string());
        return;
    }
    profiles.profiles.push(Profile {
        name,
        saturation,
        match_rule: MatchRule {
            exe: Some(exe.to_string()),
            window_class: None,
            steam_app_id: None,
        },
        outputs: vec![],
    });
}

// ---- backend + config (small local copies so the GUI is standalone) ----

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

fn config_dir() -> PathBuf {
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x).join("vibrance");
        }
    }
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join(".config")
        .join("vibrance")
}

fn profiles_path() -> PathBuf {
    config_dir().join("profiles.toml")
}

fn load_profiles() -> Profiles {
    match std::fs::read_to_string(profiles_path()) {
        Ok(s) => Profiles::from_toml(&s).unwrap_or_default(),
        Err(_) => Profiles::default(),
    }
}

fn save_profiles(profiles: &Profiles) -> anyhow::Result<()> {
    std::fs::create_dir_all(config_dir())?;
    std::fs::write(profiles_path(), profiles.to_toml()?)?;
    Ok(())
}
