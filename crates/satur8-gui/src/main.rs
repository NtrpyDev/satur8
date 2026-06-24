//! `satur8-gui` - the Satur8 desktop app (Slint).
//!
//! Sidebar + per-game profile rows with the game's real Steam icon, a satur8
//! slider and an enable toggle; a before/after preview built from the game's own
//! Steam art with the saturation applied; desktop vibrance settings and an
//! activity log.
//! Reads/writes the same `profiles.toml` the CLI and daemon use.
//!
//! Game art comes from the user's *local* Steam cache - nothing is bundled.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod proc;
mod steam;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde::{Deserialize, Serialize};
use slint::{Color, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};
use satur8_core::{Backend, MatchRule, Output, Profile, Profiles, Saturation};

slint::include_modules!();

const SWATCHES: [(u8, u8, u8); 6] = [
    (128, 128, 128),
    (229, 57, 53),
    (253, 216, 53),
    (67, 160, 71),
    (38, 166, 154),
    (30, 136, 229),
];

const TILE_COLORS: [(u8, u8, u8); 8] = [
    (13, 148, 136),
    (37, 99, 235),
    (217, 70, 39),
    (147, 51, 234),
    (202, 138, 4),
    (5, 150, 105),
    (219, 39, 119),
    (71, 85, 105),
];

struct State {
    backend: Option<Box<dyn Backend>>,
    profiles: Profiles,
    current_sat: f32,
    /// True while a live desktop preview is applied, so we can restore it.
    previewing: bool,
}

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
struct GuiConfig {
    #[serde(default)]
    dark_mode: bool,
}

fn main() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    ui.set_version_text(format!("v{}", env!("CARGO_PKG_VERSION")).into());
    ui.global::<Pal>().set_dark(load_gui_config().dark_mode);

    let backend = select_backend();
    ui.set_backend_ok(backend.is_some());
    ui.set_backend_name(
        backend
            .as_ref()
            .map(|b| b.name().to_string())
            .unwrap_or_else(|| "no".into())
            .into(),
    );

    let profiles = load_profiles();
    let first = profiles.profiles.first().cloned();
    let first_sat = first.as_ref().map(|p| p.saturation).unwrap_or(1.0);
    let state = Rc::new(RefCell::new(State {
        backend,
        profiles,
        current_sat: first_sat,
        previewing: false,
    }));

    // Games
    let games = Rc::new(VecModel::<GameRow>::default());
    rebuild_games(&games, &state.borrow().profiles);
    ui.set_games(ModelRc::from(games.clone()));

    // Running apps (with Steam AppIDs)
    let apps = Rc::new(RefCell::new(proc::running_apps()));
    let running = Rc::new(VecModel::<SharedString>::from(running_strings(&apps.borrow())));
    ui.set_running(ModelRc::from(running.clone()));

    // Output note (honest about what the backend can target).
    ui.set_outputs_note(
        match state.borrow().backend.as_ref().map(|b| b.name()) {
            Some("kwin") | Some("gnome-shell") | Some("hyprland") => {
                "On this compositor it affects all monitors.".into()
            }
            Some("drm-ctm") => "Targets the connected display(s).".into(),
            _ => "".into(),
        },
    );

    // Activity
    let activity = Rc::new(VecModel::<LogRow>::default());
    log(&activity, true, "Started Satur8");
    match state.borrow().backend.as_ref().map(|b| b.name().to_string()) {
        Some(name) => log(&activity, false, &format!("{name} backend active")),
        None => log(&activity, false, "No supported backend in this session"),
    }
    ui.set_activity(ModelRc::from(activity.clone()));

    // Preview (swatches always; real art when the profile is a known Steam game)
    let after = Rc::new(VecModel::<Color>::from(after_swatches(first_sat)));
    ui.set_before_swatches(ModelRc::from(Rc::new(VecModel::from(before_swatches()))));
    ui.set_after_swatches(ModelRc::from(after.clone()));
    if let Some(p) = &first {
        ui.set_current_profile_name(p.name.clone().into());
        ui.set_current_percent(sat_to_percent(p.saturation));
        set_preview(&ui, profile_app_id(p), p.saturation, &after);
    }

    ui.set_default_percent(sat_to_percent(state.borrow().profiles.default_saturation));

    // Off-screen screenshot helper: jump straight to a page (0..3).
    if let Ok(n) = std::env::var("VIBRANCE_GUI_NAV") {
        if let Ok(n) = n.parse::<i32>() {
            ui.set_nav(n);
        }
    }
    if let Ok(value) = std::env::var("SATUR8_GUI_DARK") {
        ui.global::<Pal>().set_dark(matches!(value.as_str(), "1" | "true" | "yes" | "on"));
    }

    // ---------------- callbacks ----------------
    ui.on_dark_mode_changed(|dark_mode| {
        save_gui_config(&GuiConfig { dark_mode });
    });
    ui.on_open_link(|url| {
        open_url(url.as_str());
    });

    ui.on_default_changed({
        let state = state.clone();
        let w = ui.as_weak();
        move |percent| {
            let sat = percent_to_sat(percent);
            {
                let mut st = state.borrow_mut();
                st.profiles.default_saturation = sat;
                save(&st.profiles);
                if let Some(b) = st.backend.as_mut() {
                    let _ = apply_or_reset(b.as_mut(), sat);
                }
                // This is an explicit desktop setting, not a temporary preview.
                st.previewing = false;
            }
            if let Some(ui) = w.upgrade() {
                ui.set_default_percent(percent.round() as i32);
            }
        }
    });

    ui.on_percent_changed({
        let state = state.clone();
        let games = games.clone();
        let after = after.clone();
        let w = ui.as_weak();
        move |i, percent| {
            let sat = percent_to_sat(percent);
            let app_id;
            let name;
            {
                let mut st = state.borrow_mut();
                if let Some(p) = st.profiles.profiles.get_mut(i as usize) {
                    p.saturation = sat;
                }
                st.current_sat = sat;
                let p = st.profiles.profiles.get(i as usize);
                name = p.map(|p| p.name.clone()).unwrap_or_default();
                app_id = p.and_then(profile_app_id);
                save(&st.profiles);
            }
            if let Some(mut row) = games.row_data(i as usize) {
                row.percent = percent.round() as i32;
                games.set_row_data(i as usize, row);
            }
            if let Some(ui) = w.upgrade() {
                ui.set_current_profile_name(name.into());
                ui.set_current_percent(sat_to_percent(sat));
                set_preview(&ui, app_id, sat, &after);
            }
        }
    });

    ui.on_toggle_enabled({
        let state = state.clone();
        let activity = activity.clone();
        move |i, enabled| {
            // Editing a profile only saves config - it never touches the live
            // desktop. The profile applies during the game via the launch wrapper.
            let st = state.borrow();
            let name = st.profiles.profiles.get(i as usize).map(|p| p.name.clone()).unwrap_or_default();
            log(&activity, false, &format!("{} {}", name, if enabled { "enabled" } else { "disabled" }));
        }
    });

    ui.on_remove({
        let state = state.clone();
        let games = games.clone();
        let activity = activity.clone();
        move |i| {
            let name;
            {
                let mut st = state.borrow_mut();
                let i = i as usize;
                name = st.profiles.profiles.get(i).map(|p| p.name.clone()).unwrap_or_default();
                if i < st.profiles.profiles.len() {
                    st.profiles.profiles.remove(i);
                }
                save(&st.profiles);
            }
            rebuild_games(&games, &state.borrow().profiles);
            log(&activity, false, &format!("Removed {name}"));
        }
    });

    ui.on_add_running({
        let state = state.clone();
        let games = games.clone();
        let apps = apps.clone();
        let activity = activity.clone();
        move |i| {
            let app = apps.borrow().get(i as usize).map(|a| (a.exe.clone(), a.app_id));
            let Some((exe, app_id)) = app else { return };
            {
                let mut st = state.borrow_mut();
                add_or_update(&mut st.profiles, &exe, app_id, 1.4);
                save(&st.profiles);
            }
            rebuild_games(&games, &state.borrow().profiles);
            log(&activity, true, &format!("Added {exe}"));
        }
    });

    ui.on_refresh_running({
        let apps = apps.clone();
        let running = running.clone();
        move || {
            *apps.borrow_mut() = proc::running_apps();
            running.set_vec(running_strings(&apps.borrow()));
        }
    });

    ui.on_preview_profile({
        let state = state.clone();
        let after = after.clone();
        let activity = activity.clone();
        let w = ui.as_weak();
        move |i| {
            let (name, sat, app_id) = {
                let st = state.borrow();
                match st.profiles.profiles.get(i as usize) {
                    Some(p) => (p.name.clone(), p.saturation, profile_app_id(p)),
                    None => return,
                }
            };
            {
                let mut st = state.borrow_mut();
                st.current_sat = sat;
                if let Some(b) = st.backend.as_mut() {
                    let _ = b.apply(&all_outputs(), Saturation::new(sat));
                }
                st.previewing = true;
            }
            if let Some(ui) = w.upgrade() {
                ui.set_current_profile_name(name.clone().into());
                ui.set_current_percent(sat_to_percent(sat));
                set_preview(&ui, app_id, sat, &after);
            }
            log(&activity, true, &format!("Previewing {name} on desktop"));
        }
    });

    ui.on_apply_output({
        let state = state.clone();
        let activity = activity.clone();
        move || {
            let mut st = state.borrow_mut();
            let sat = st.current_sat;
            if let Some(b) = st.backend.as_mut() {
                match b.apply(&all_outputs(), Saturation::new(sat)) {
                    Ok(()) => log(&activity, true, &format!("Applied {:+.0}%", (sat - 1.0) * 100.0)),
                    Err(e) => log(&activity, false, &format!("Apply failed: {e}")),
                }
            }
            st.previewing = true;
        }
    });

    ui.on_preview_output({
        let state = state.clone();
        let activity = activity.clone();
        move || {
            let mut st = state.borrow_mut();
            let sat = st.current_sat;
            if let Some(b) = st.backend.as_mut() {
                let _ = b.apply(&all_outputs(), Saturation::new(sat));
                log(&activity, false, &format!("Previewing {:+.0}% on desktop", (sat - 1.0) * 100.0));
            }
            st.previewing = true;
        }
    });

    ui.on_restore_output({
        let state = state.clone();
        let activity = activity.clone();
        move || {
            let mut st = state.borrow_mut();
            restore_desktop_default(&mut st);
            st.previewing = false;
            log(&activity, false, "Restored desktop");
        }
    });

    ui.on_clear_activity({
        let activity = activity.clone();
        move || activity.set_vec(Vec::<LogRow>::new())
    });

    // Never leave a live preview applied after the window closes.
    ui.window().on_close_requested({
        let state = state.clone();
        move || {
            let mut st = state.borrow_mut();
            if st.previewing {
                restore_desktop_default(&mut st);
                st.previewing = false;
            }
            slint::CloseRequestResponse::HideWindow
        }
    });

    ui.run()
}

// ---------------- helpers ----------------

fn all_outputs() -> Output {
    Output { id: "all".into(), human_name: "All outputs".into() }
}
fn apply_or_reset(backend: &mut dyn Backend, sat: f32) -> Result<(), satur8_core::BackendError> {
    if (sat - 1.0).abs() < 1e-3 {
        backend.reset(&all_outputs())
    } else {
        backend.apply(&all_outputs(), Saturation::new(sat))
    }
}
fn restore_desktop_default(st: &mut State) {
    let sat = st.profiles.default_saturation;
    if let Some(b) = st.backend.as_mut() {
        let _ = apply_or_reset(b.as_mut(), sat);
    }
}
fn percent_to_sat(percent: f32) -> f32 {
    (1.0 + percent / 100.0).clamp(0.0, 4.0)
}
fn sat_to_percent(sat: f32) -> i32 {
    ((sat - 1.0) * 100.0).round() as i32
}

fn profile_app_id(p: &Profile) -> Option<u32> {
    p.match_rule.steam_app_id.or_else(|| {
        let key = p.match_rule.exe.as_deref().unwrap_or(p.name.as_str());
        steam::known_app_id(key)
    })
}

fn rebuild_games(model: &VecModel<GameRow>, profiles: &Profiles) {
    let rows: Vec<GameRow> = profiles
        .profiles
        .iter()
        .map(|p| {
            let exe = p.match_rule.exe.clone().unwrap_or_else(|| p.name.clone());
            let icon = profile_app_id(p).and_then(steam::icon_path).and_then(|path| load_icon(&path));
            GameRow {
                name: p.name.clone().into(),
                exe: exe.into(),
                percent: sat_to_percent(p.saturation),
                enabled: true,
                accent: tile_color(&p.name),
                initial: p.name.chars().next().unwrap_or('?').to_uppercase().to_string().into(),
                has_icon: icon.is_some(),
                icon: icon.unwrap_or_default(),
            }
        })
        .collect();
    model.set_vec(rows);
}

/// Set the right-panel before/after: real Steam art if we have it, swatches else.
fn set_preview(ui: &AppWindow, app_id: Option<u32>, sat: f32, after_swatch_model: &VecModel<Color>) {
    after_swatch_model.set_vec(after_swatches(sat));
    if let Some(path) = app_id.and_then(steam::preview_path) {
        if let Some((before, after)) = load_preview_pair(&path, sat) {
            ui.set_before_image(before);
            ui.set_after_image(after);
            ui.set_has_image(true);
            return;
        }
    }
    ui.set_has_image(false);
}

fn to_slint(img: &image::RgbaImage) -> Image {
    let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(img.as_raw(), img.width(), img.height());
    Image::from_rgba8(buf)
}

/// Centre-cropped square icon.
fn load_icon(path: &Path) -> Option<Image> {
    let img = image::open(path).ok()?.into_rgba8();
    let (w, h) = (img.width(), img.height());
    let side = w.min(h);
    let cropped = image::imageops::crop_imm(&img, (w - side) / 2, (h - side) / 2, side, side).to_image();
    let small = image::imageops::thumbnail(&cropped, 96, 96);
    Some(to_slint(&small))
}

/// (before, after) preview pair from a wide art image, after = saturation applied.
fn load_preview_pair(path: &Path, sat: f32) -> Option<(Image, Image)> {
    let base = image::open(path).ok()?.into_rgba8();
    let tw = 360u32;
    let th = (tw as f32 * base.height() as f32 / base.width() as f32).round() as u32;
    let base = image::imageops::thumbnail(&base, tw, th.max(1));
    let before = to_slint(&base);

    let m = Saturation::new(sat).matrix();
    let mut after = base.clone();
    for px in after.pixels_mut() {
        let (r, g, b) = (px[0] as f32 / 255.0, px[1] as f32 / 255.0, px[2] as f32 / 255.0);
        let o = |row: [f32; 3]| ((row[0] * r + row[1] * g + row[2] * b).clamp(0.0, 1.0) * 255.0).round() as u8;
        px[0] = o(m[0]);
        px[1] = o(m[1]);
        px[2] = o(m[2]);
    }
    Some((before, to_slint(&after)))
}

fn before_swatches() -> Vec<Color> {
    SWATCHES.iter().map(|&(r, g, b)| Color::from_rgb_u8(r, g, b)).collect()
}
fn after_swatches(sat: f32) -> Vec<Color> {
    let m = Saturation::new(sat).matrix();
    SWATCHES
        .iter()
        .map(|&(r, g, b)| {
            let (rf, gf, bf) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
            let o = |row: [f32; 3]| ((row[0] * rf + row[1] * gf + row[2] * bf).clamp(0.0, 1.0) * 255.0).round() as u8;
            Color::from_rgb_u8(o(m[0]), o(m[1]), o(m[2]))
        })
        .collect()
}

fn tile_color(name: &str) -> Color {
    let h = name.bytes().fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    let (r, g, b) = TILE_COLORS[(h as usize) % TILE_COLORS.len()];
    Color::from_rgb_u8(r, g, b)
}

fn log(model: &VecModel<LogRow>, ok: bool, text: &str) {
    model.insert(0, LogRow { ok, text: text.into(), time: now_hms().into() });
}

fn now_hms() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let day = secs % 86_400;
    format!("{:02}:{:02}:{:02}", day / 3600, (day % 3600) / 60, day % 60)
}

fn running_strings(apps: &[proc::RunningApp]) -> Vec<SharedString> {
    apps.iter().map(|a| a.exe.clone().into()).collect()
}

fn add_or_update(profiles: &mut Profiles, exe: &str, app_id: Option<u32>, saturation: f32) {
    let name = exe.trim_end_matches(".exe").trim_end_matches(".x86_64").to_string();
    if let Some(p) = profiles
        .profiles
        .iter_mut()
        .find(|p| p.match_rule.exe.as_deref() == Some(exe) || p.name == name)
    {
        p.saturation = saturation;
        p.match_rule.exe = Some(exe.to_string());
        if app_id.is_some() {
            p.match_rule.steam_app_id = app_id;
        }
        return;
    }
    profiles.profiles.push(Profile {
        name,
        saturation,
        match_rule: MatchRule {
            exe: Some(exe.to_string()),
            window_class: None,
            steam_app_id: app_id,
        },
        outputs: vec![],
    });
}

fn select_backend() -> Option<Box<dyn Backend>> {
    use satur8_drm_ctm::DrmCtmBackend;
    use satur8_gnome::GnomeBackend;
    use satur8_hyprland::HyprlandBackend;
    use satur8_kwin::KwinBackend;
    use satur8_nv_control::NvControlBackend;

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
            return PathBuf::from(x).join("satur8");
        }
    }
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config").join("satur8")
}
fn load_profiles() -> Profiles {
    match std::fs::read_to_string(config_dir().join("profiles.toml")) {
        Ok(s) => Profiles::from_toml(&s).unwrap_or_default(),
        Err(_) => Profiles::default(),
    }
}
fn load_gui_config() -> GuiConfig {
    match std::fs::read_to_string(config_dir().join("gui.toml")) {
        Ok(s) => toml::from_str(&s).unwrap_or_default(),
        Err(_) => GuiConfig::default(),
    }
}
fn save(profiles: &Profiles) {
    let dir = config_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(toml) = profiles.to_toml() {
        if std::fs::write(dir.join("profiles.toml"), toml).is_ok() {
            reload_daemon_profiles();
        }
    }
}
fn save_gui_config(config: &GuiConfig) {
    let dir = config_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(toml) = toml::to_string(config) {
        let _ = std::fs::write(dir.join("gui.toml"), toml);
    }
}

fn open_url(url: &str) {
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

fn reload_daemon_profiles() {
    let Ok(conn) = zbus::blocking::Connection::session() else {
        return;
    };
    let Ok(proxy) = zbus::blocking::Proxy::new(
        &conn,
        "org.satur8.Daemon",
        "/org/satur8/Daemon",
        "org.satur8.Daemon",
    ) else {
        return;
    };
    let _: Result<(), _> = proxy.call("Reload", &());
}
