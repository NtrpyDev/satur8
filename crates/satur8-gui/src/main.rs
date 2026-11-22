//! `satur8-gui` - the Satur8 desktop app (Slint).
//!
//! Main page: the desktop vibrance strip and per-game cards with the game's
//! real Steam icon, a saturation slider and an enable toggle. Status page:
//! health checks, repair, per-game switching and an activity log. About page.
//! Reads/writes the same `profiles.toml` the CLI and daemon use.
//!
//! Game art comes from the user's *local* Steam cache - nothing is bundled.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod status;
mod steam;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use satur8_backend::{all_outputs, select_backend, RestoreGuard};
use satur8_core::proc;
use satur8_core::{Backend, MatchRule, Profile, Profiles, Saturation};
use serde::{Deserialize, Serialize};
use slint::{Color, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};

slint::include_modules!();

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
    backend: Option<RestoreGuard>,
    profiles: Profiles,
}

/// Saturation a newly added running game starts at, unless the user picked
/// another value on the Status page. (+40%; this was hard-coded before.)
const NEW_GAME_SATURATION: f32 = 1.4;

fn default_new_game_saturation() -> f32 {
    NEW_GAME_SATURATION
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
struct GuiConfig {
    #[serde(default)]
    dark_mode: bool,
    /// Status page > Settings > "Starting saturation for new games".
    #[serde(default = "default_new_game_saturation")]
    new_game_saturation: f32,
}

impl Default for GuiConfig {
    fn default() -> Self {
        // Existing saved choices still take precedence. Fresh installs match the
        // app and site’s near-black visual language.
        Self {
            dark_mode: true,
            new_game_saturation: NEW_GAME_SATURATION,
        }
    }
}

impl GuiConfig {
    /// The saturation to give a newly added game, kept inside the control's
    /// -100%..+100% range (0.0..2.0). A broken value falls back to the default.
    fn new_game_saturation(&self) -> f32 {
        if self.new_game_saturation.is_finite() {
            self.new_game_saturation.clamp(0.0, 2.0)
        } else {
            NEW_GAME_SATURATION
        }
    }
}

/// True while the Status page is shown; the "Right now" poll runs only then.
static STATUS_VISIBLE: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// The latest health facts, for "Report a bug". UI thread only.
    static LAST_FACTS: RefCell<Option<status::Facts>> = const { RefCell::new(None) };
}

fn main() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    ui.set_version_text(format!("v{}", env!("CARGO_PKG_VERSION")).into());
    let gui_config = Rc::new(RefCell::new(load_gui_config()));
    ui.global::<Pal>().set_dark(gui_config.borrow().dark_mode);
    ui.set_new_game_percent(sat_to_percent(gui_config.borrow().new_game_saturation()));

    let profiles = load_profiles(Profiles::default());
    let backend = match select_backend() {
        Ok(backend) => Some(RestoreGuard::inactive(
            backend,
            profiles.default_saturation(),
        )),
        Err(error) => {
            eprintln!("satur8-gui: {error}");
            None
        }
    };
    ui.set_backend_ok(backend.is_some());
    ui.set_backend_name(
        backend
            .as_ref()
            .map(|b| b.backend().name().to_string())
            .unwrap_or_else(|| "no".into())
            .into(),
    );

    let state = Rc::new(RefCell::new(State { backend, profiles }));

    // Games
    let games = Rc::new(VecModel::<GameRow>::default());
    rebuild_games(&games, &state.borrow().profiles);
    ui.set_games(ModelRc::from(games.clone()));

    // Running apps (with Steam AppIDs)
    let apps = Rc::new(RefCell::new(proc::running_apps()));
    let running = Rc::new(VecModel::<SharedString>::from(running_strings(
        &apps.borrow(),
    )));
    ui.set_running(ModelRc::from(running.clone()));

    // Activity
    let activity = Rc::new(VecModel::<LogRow>::default());
    log(&activity, true, "Started Satur8");
    match state
        .borrow()
        .backend
        .as_ref()
        .map(|b| b.backend().name().to_string())
    {
        Some(name) => log(&activity, false, &format!("{name} backend active")),
        None => log(&activity, false, "No supported backend in this session"),
    }
    ui.set_activity(ModelRc::from(activity.clone()));

    ui.set_default_percent(sat_to_percent(state.borrow().profiles.default_saturation));

    // Off-screen screenshot helper: jump straight to a page
    // (0 = Main, 1 = Status, 2 = About).
    if let Ok(n) = std::env::var("VIBRANCE_GUI_NAV") {
        if let Ok(n) = n.parse::<i32>() {
            ui.set_nav(n);
        }
    }
    if let Ok(value) = std::env::var("SATUR8_GUI_DARK") {
        ui.global::<Pal>()
            .set_dark(matches!(value.as_str(), "1" | "true" | "yes" | "on"));
    }

    // ---------------- callbacks ----------------
    ui.on_dark_mode_changed({
        let gui_config = gui_config.clone();
        move |dark_mode| {
            let mut config = gui_config.borrow_mut();
            config.dark_mode = dark_mode;
            save_gui_config(&config);
        }
    });

    // ---------------- Status page ----------------
    // Read-only checks at startup; the live poll thread idles until the page
    // is shown.
    spawn_check(&ui, None);
    STATUS_VISIBLE.store(ui.get_nav() == 1, Ordering::Release);
    spawn_live_poll(ui.as_weak());
    ui.on_page_shown({
        let w = ui.as_weak();
        move |page| {
            STATUS_VISIBLE.store(page == 1, Ordering::Release);
            if page == 1 {
                if let Some(ui) = w.upgrade() {
                    if !ui.get_checking() && !ui.get_repairing() && !ui.get_switch_busy() {
                        spawn_check(&ui, None);
                    }
                }
            }
        }
    });
    ui.on_check_health({
        let w = ui.as_weak();
        move || {
            if let Some(ui) = w.upgrade() {
                spawn_check(&ui, Some("Checked system health"));
            }
        }
    });
    ui.on_repair({
        let w = ui.as_weak();
        move || {
            if let Some(ui) = w.upgrade() {
                spawn_repair(&ui);
            }
        }
    });
    ui.on_set_switching({
        let w = ui.as_weak();
        move |on| {
            if let Some(ui) = w.upgrade() {
                spawn_switching(&ui, on);
            }
        }
    });
    ui.on_new_game_changed({
        let w = ui.as_weak();
        move |percent| {
            if let Some(ui) = w.upgrade() {
                ui.set_new_game_percent(percent.round() as i32);
            }
        }
    });
    ui.on_new_game_released({
        let gui_config = gui_config.clone();
        let activity = activity.clone();
        move |percent| {
            let Some(sat) = percent_to_sat(percent.round()) else {
                return;
            };
            let mut config = gui_config.borrow_mut();
            config.new_game_saturation = sat;
            save_gui_config(&config);
            log(
                &activity,
                false,
                &format!("New games now start at {:+.0}%", (sat - 1.0) * 100.0),
            );
        }
    });
    ui.on_sysinfo_copied({
        let activity = activity.clone();
        move || log(&activity, true, "Copied system info to the clipboard")
    });
    ui.on_report_bug({
        let activity = activity.clone();
        move || {
            let info = LAST_FACTS.with(|facts| {
                facts.borrow().as_ref().map(|facts| {
                    status::system_info_for_this_machine(facts, &status::health_rows(facts))
                })
            });
            match info {
                Some(info) => {
                    open_url(&status::bug_report_url(&info));
                    log(
                        &activity,
                        true,
                        "Opened a new bug report in your browser (nothing was sent)",
                    );
                }
                None => log(
                    &activity,
                    false,
                    "Still checking; try Report a bug again in a moment",
                ),
            }
        }
    });
    ui.on_open_link(|url| {
        open_url(url.as_str());
    });
    ui.on_default_changed({
        let state = state.clone();
        let w = ui.as_weak();
        move |percent| {
            let Some(sat) = percent_to_sat(percent) else {
                eprintln!("satur8-gui: ignored non-finite default saturation input");
                return;
            };
            // Runs on every slider move: apply live, keep the value in memory.
            // profiles.toml is written once, in on_default_released.
            {
                let mut st = state.borrow_mut();
                st.profiles.default_saturation = sat;
                if let Some(b) = st.backend.as_mut() {
                    b.set_restore_to(Saturation::new(sat));
                    if apply_or_reset(b.backend_mut(), sat).is_ok() {
                        b.disarm();
                    }
                }
            }
            if let Some(ui) = w.upgrade() {
                ui.set_default_percent(percent.round() as i32);
            }
        }
    });

    // End of a desktop-slider drag (or the Restore button): save once.
    ui.on_default_released({
        let state = state.clone();
        move |percent| {
            let Some(sat) = percent_to_sat(percent) else {
                eprintln!("satur8-gui: ignored non-finite default saturation input");
                return;
            };
            let mut st = state.borrow_mut();
            st.profiles.default_saturation = sat;
            save(&st.profiles);
        }
    });

    ui.on_percent_changed({
        let state = state.clone();
        let games = games.clone();
        move |i, percent| {
            let Some(sat) = percent_to_sat(percent) else {
                eprintln!("satur8-gui: ignored non-finite profile saturation input");
                return;
            };
            {
                let mut st = state.borrow_mut();
                if let Some(p) = st.profiles.profiles.get_mut(i as usize) {
                    p.saturation = sat;
                }
                save(&st.profiles);
            }
            if let Some(mut row) = games.row_data(i as usize) {
                row.percent = percent.round() as i32;
                games.set_row_data(i as usize, row);
            }
        }
    });

    ui.on_toggle_enabled({
        let state = state.clone();
        let activity = activity.clone();
        move |i, enabled| {
            let name = {
                let mut st = state.borrow_mut();
                let Some(profile) = st.profiles.profiles.get_mut(i as usize) else {
                    return;
                };
                profile.enabled = enabled;
                let name = profile.name.clone();
                save(&st.profiles);
                name
            };
            log(
                &activity,
                false,
                &format!("{} {}", name, if enabled { "enabled" } else { "disabled" }),
            );
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
                name = st
                    .profiles
                    .profiles
                    .get(i)
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
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
        let gui_config = gui_config.clone();
        move |i| {
            let app = apps
                .borrow()
                .get(i as usize)
                .map(|a| (a.exe.clone(), a.app_id));
            let Some((exe, app_id)) = app else { return };
            {
                let mut st = state.borrow_mut();
                let saturation = gui_config.borrow().new_game_saturation();
                add_or_update(&mut st.profiles, &exe, app_id, saturation);
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

    ui.on_clear_activity({
        let activity = activity.clone();
        move || activity.set_vec(Vec::<LogRow>::new())
    });

    // Closing hides the window while the tray is running; otherwise it quits.
    ui.window().on_close_requested({
        move || {
            if satur8_tray_is_running() {
                slint::CloseRequestResponse::HideWindow
            } else {
                let _ = slint::quit_event_loop();
                slint::CloseRequestResponse::HideWindow
            }
        }
    });

    ui.run()
}

// ---------------- Status page helpers ----------------

/// Add a line to the activity log from a closure that only has the window.
fn log_ui(ui: &AppWindow, ok: bool, text: &str) {
    let model = ui.get_activity();
    if let Some(model) = model.as_any().downcast_ref::<VecModel<LogRow>>() {
        log(model, ok, text);
    }
}

/// Show freshly gathered facts: health rows, toggle, and system info.
fn apply_facts(ui: &AppWindow, facts: status::Facts) {
    let rows = status::health_rows(&facts);
    let switch = status::switch_state(&facts);
    ui.set_health(ModelRc::from(Rc::new(VecModel::from(
        rows.iter()
            .map(|row| HealthRowUi {
                label: row.label.into(),
                detail: row.detail.clone().into(),
                level: match row.level {
                    status::Level::Ok => 0,
                    status::Level::Problem => 1,
                    status::Level::NotApplicable => 2,
                },
            })
            .collect::<Vec<_>>(),
    ))));
    ui.set_health_problem(rows.iter().any(|r| r.level == status::Level::Problem));
    ui.set_switch_available(switch.available);
    ui.set_switch_on(switch.on);
    ui.set_switch_detail(switch.detail.into());
    ui.set_sysinfo_text(status::system_info_for_this_machine(&facts, &rows).into());
    LAST_FACTS.with(|last| *last.borrow_mut() = Some(facts));
}

/// Read-only health check off the UI thread.
fn spawn_check(ui: &AppWindow, done_message: Option<&'static str>) {
    ui.set_checking(true);
    let w = ui.as_weak();
    std::thread::spawn(move || {
        let facts = status::probe();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = w.upgrade() {
                apply_facts(&ui, facts);
                ui.set_checking(false);
                if let Some(message) = done_message {
                    log_ui(&ui, true, message);
                }
            }
        });
    });
}

/// Run a plan built from fresh facts off the UI thread, log every step, then
/// re-check so the page shows the real result.
fn spawn_plan(
    ui: &AppWindow,
    busy: fn(&AppWindow, bool),
    title: &'static str,
    plan_for: fn(&status::Facts) -> Vec<status::Action>,
) {
    busy(ui, true);
    let w = ui.as_weak();
    std::thread::spawn(move || {
        let before = status::probe();
        let plan = plan_for(&before);
        let results = status::execute(&mut status::RealSystem, &plan);
        // Found in the live test: right after `systemctl start` the daemon is
        // not on the bus yet, so the re-check showed it as "Not running".
        // Its start can include `satur8 repair` (ExecStartPre), hence 20 s.
        if status::plan_starts_daemon(&plan) {
            status::wait_for_daemon(std::time::Duration::from_secs(20));
        }
        let after = status::probe();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = w.upgrade() {
                if results.is_empty() {
                    log_ui(&ui, false, &format!("{title}: nothing to do"));
                }
                for (ok, line) in &results {
                    log_ui(&ui, *ok, line);
                }
                apply_facts(&ui, after);
                busy(&ui, false);
            }
        });
    });
}

fn spawn_repair(ui: &AppWindow) {
    spawn_plan(
        ui,
        |ui, on| ui.set_repairing(on),
        "Repair",
        status::plan_repair,
    );
}

fn spawn_switching(ui: &AppWindow, on: bool) {
    // The plan is only run when the toggle is usable in this session.
    fn enable(facts: &status::Facts) -> Vec<status::Action> {
        if status::switch_state(facts).available {
            status::plan_enable(facts)
        } else {
            Vec::new()
        }
    }
    fn disable(facts: &status::Facts) -> Vec<status::Action> {
        if status::switch_state(facts).available {
            status::plan_disable(facts)
        } else {
            Vec::new()
        }
    }
    let (title, plan): (&'static str, fn(&status::Facts) -> Vec<status::Action>) = if on {
        ("Turn on per-game switching", enable)
    } else {
        ("Turn off per-game switching", disable)
    };
    spawn_plan(ui, |ui, busy| ui.set_switch_busy(busy), title, plan);
}

fn percent_text(sat: f32) -> String {
    let percent = sat_to_percent(sat);
    if percent > 0 {
        format!("+{percent}%")
    } else {
        format!("{percent}%")
    }
}

/// "Right now" poll: every 2 seconds while the Status page is shown, read the
/// daemon's properties and the compositor's current saturation (read-only).
fn spawn_live_poll(w: slint::Weak<AppWindow>) {
    std::thread::spawn(move || loop {
        if STATUS_VISIBLE.load(Ordering::Acquire) {
            let live = status::probe_live();
            let (saturation, note) = match live.applied {
                Some(sat) => (
                    percent_text(sat),
                    if (sat - 1.0).abs() < 1e-3 {
                        "normal colors, read from the compositor".to_string()
                    } else {
                        "read from the compositor".to_string()
                    },
                ),
                None => {
                    // Not readable here: show what should be applied.
                    let profiles = load_profiles(Profiles::default());
                    let sat = live
                        .active_profile
                        .as_deref()
                        .filter(|name| !name.is_empty())
                        .and_then(|name| profiles.by_name(name))
                        .map(|p| p.saturation)
                        .unwrap_or(profiles.default_saturation);
                    (
                        percent_text(sat),
                        "expected; this session cannot report it".to_string(),
                    )
                }
            };
            let active = status::describe_active(&live);
            let focused = status::describe_focused(&live);
            let w = w.clone();
            let keep_going = slint::invoke_from_event_loop(move || {
                if let Some(ui) = w.upgrade() {
                    ui.set_live_daemon(live.daemon_running);
                    ui.set_live_profile(active.into());
                    ui.set_live_saturation(saturation.into());
                    ui.set_live_saturation_note(note.into());
                    ui.set_live_has_focused(focused.is_some());
                    ui.set_live_focused(focused.unwrap_or_default().into());
                }
            });
            if keep_going.is_err() {
                return;
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    });
}

// ---------------- helpers ----------------

fn apply_or_reset(backend: &mut dyn Backend, sat: f32) -> Result<(), satur8_core::BackendError> {
    if (sat - 1.0).abs() < 1e-3 {
        backend.reset(&all_outputs())
    } else {
        backend.apply(&all_outputs(), Saturation::new(sat))
    }
}
fn percent_to_sat(percent: f32) -> Option<f32> {
    Saturation::try_new(1.0 + percent / 100.0)
        .ok()
        .map(Saturation::get)
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
            let icon = profile_app_id(p)
                .and_then(steam::icon_path)
                .and_then(|path| load_icon(&path));
            GameRow {
                name: p.name.clone().into(),
                exe: exe.into(),
                percent: sat_to_percent(p.saturation),
                enabled: p.enabled,
                accent: tile_color(&p.name),
                initial: p
                    .name
                    .chars()
                    .next()
                    .unwrap_or('?')
                    .to_uppercase()
                    .to_string()
                    .into(),
                has_icon: icon.is_some(),
                icon: icon.unwrap_or_default(),
            }
        })
        .collect();
    model.set_vec(rows);
}

fn to_slint(img: &image::RgbaImage) -> Image {
    let buf =
        SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(img.as_raw(), img.width(), img.height());
    Image::from_rgba8(buf)
}

/// Centre-cropped square icon.
fn load_icon(path: &Path) -> Option<Image> {
    let img = image::open(path).ok()?.into_rgba8();
    let (w, h) = (img.width(), img.height());
    let side = w.min(h);
    let cropped =
        image::imageops::crop_imm(&img, (w - side) / 2, (h - side) / 2, side, side).to_image();
    // Steam's game icon is only 32x32; enlarge it smoothly instead of with the
    // thumbnail filter, which is meant for shrinking.
    let small = if side < 96 {
        image::imageops::resize(&cropped, 96, 96, image::imageops::FilterType::Lanczos3)
    } else {
        image::imageops::thumbnail(&cropped, 96, 96)
    };
    Some(to_slint(&small))
}

fn tile_color(name: &str) -> Color {
    let h = name
        .bytes()
        .fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    let (r, g, b) = TILE_COLORS[(h as usize) % TILE_COLORS.len()];
    Color::from_rgb_u8(r, g, b)
}

fn log(model: &VecModel<LogRow>, ok: bool, text: &str) {
    model.insert(
        0,
        LogRow {
            ok,
            text: text.into(),
            time: now_hms().into(),
        },
    );
}

fn now_hms() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as libc::time_t)
        .unwrap_or(0);
    let mut local = std::mem::MaybeUninit::<libc::tm>::uninit();
    // SAFETY: both pointers are valid for this call and localtime_r initializes
    // `local` before returning a non-null pointer.
    let result = unsafe { libc::localtime_r(&timestamp, local.as_mut_ptr()) };
    if result.is_null() {
        return "--:--:--".into();
    }
    // SAFETY: localtime_r returned non-null, so `local` was initialized.
    let local = unsafe { local.assume_init() };
    format!(
        "{:02}:{:02}:{:02}",
        local.tm_hour, local.tm_min, local.tm_sec
    )
}

/// The display this process draws on, used to tell this session's tray from a
/// tray that belongs to another login.
struct SessionDisplay {
    wayland: Option<Vec<u8>>,
    x11: Option<Vec<u8>>,
}

impl SessionDisplay {
    fn from_env() -> Self {
        use std::os::unix::ffi::OsStringExt;
        let var = |name: &str| {
            std::env::var_os(name)
                .map(OsStringExt::into_vec)
                .filter(|value| !value.is_empty())
        };
        Self {
            wayland: var("WAYLAND_DISPLAY"),
            x11: var("DISPLAY"),
        }
    }

    /// Whether a process with this `/proc/<pid>/environ` content draws on the
    /// same display. Wayland decides when this session has one, X11 otherwise.
    /// With no display known at all, the caller's same-user check is all there
    /// is to go on.
    fn matches(&self, environ: &[u8]) -> bool {
        let theirs = |name: &[u8]| {
            environ
                .split(|byte| *byte == 0)
                .find_map(|entry| entry.strip_prefix(name)?.strip_prefix(b"="))
        };
        match (&self.wayland, &self.x11) {
            (Some(ours), _) => theirs(b"WAYLAND_DISPLAY") == Some(ours.as_slice()),
            (None, Some(ours)) => theirs(b"DISPLAY") == Some(ours.as_slice()),
            (None, None) => true,
        }
    }
}

/// True when a tray icon is running for this user in this session. A tray that
/// belongs to another user, or to this user's other session, cannot reopen
/// this window, so it must not count.
fn satur8_tray_is_running() -> bool {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    let display = SessionDisplay::from_env();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.bytes().all(|byte| byte.is_ascii_digit()))
            && std::fs::read_to_string(path.join("comm"))
                .is_ok_and(|comm| comm.trim() == "satur8-tray")
            && std::fs::metadata(&path).is_ok_and(|meta| meta.uid() == uid)
            && std::fs::read(path.join("environ")).is_ok_and(|environ| display.matches(&environ))
    })
}

fn running_strings(apps: &[proc::RunningApp]) -> Vec<SharedString> {
    apps.iter().map(|a| a.exe.clone().into()).collect()
}

fn add_or_update(profiles: &mut Profiles, exe: &str, app_id: Option<u32>, saturation: f32) {
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
        if app_id.is_some() {
            p.match_rule.steam_app_id = app_id;
        }
        return;
    }
    profiles.profiles.push(Profile {
        name,
        enabled: true,
        saturation,
        match_rule: MatchRule {
            exe: Some(exe.to_string()),
            window_class: None,
            steam_app_id: app_id,
        },
        outputs: vec![],
    });
}

fn config_dir() -> PathBuf {
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x).join("satur8");
        }
    }
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join(".config")
        .join("satur8")
}
fn load_profiles(previous: Profiles) -> Profiles {
    match std::fs::read_to_string(config_dir().join("profiles.toml")) {
        Ok(source) => match Profiles::from_toml(&source) {
            Ok(profiles) => profiles,
            Err(error) => {
                eprintln!(
                    "satur8-gui: failed to parse profiles.toml; keeping last-known-good profiles: {error}"
                );
                previous
            }
        },
        Err(_) => previous,
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
    if let Err(error) = std::fs::create_dir_all(&dir) {
        eprintln!("satur8-gui: failed to create {}: {error}", dir.display());
        return;
    }
    let Ok(toml) = profiles.to_toml() else {
        eprintln!("satur8-gui: failed to serialize profiles");
        return;
    };
    let path = dir.join("profiles.toml");
    let temporary = dir.join(format!(".profiles.toml.{}.tmp", std::process::id()));
    if let Err(error) = std::fs::write(&temporary, toml) {
        eprintln!(
            "satur8-gui: failed to write {}: {error}",
            temporary.display()
        );
        return;
    }
    if let Err(error) = std::fs::rename(&temporary, &path) {
        let _ = std::fs::remove_file(&temporary);
        eprintln!("satur8-gui: failed to replace {}: {error}", path.display());
        return;
    }
    reload_daemon_profiles();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_on_the_same_wayland_display_counts() {
        let ours = SessionDisplay {
            wayland: Some(b"wayland-0".to_vec()),
            x11: Some(b":0".to_vec()),
        };
        assert!(ours.matches(b"HOME=/home/a\0WAYLAND_DISPLAY=wayland-0\0DISPLAY=:0\0"));
    }

    #[test]
    fn tray_on_another_wayland_display_does_not_count() {
        let ours = SessionDisplay {
            wayland: Some(b"wayland-0".to_vec()),
            x11: Some(b":0".to_vec()),
        };
        // Same X display name, different compositor: another session.
        assert!(!ours.matches(b"WAYLAND_DISPLAY=wayland-1\0DISPLAY=:0\0"));
        // A longer name that merely starts the same must not match.
        assert!(!ours.matches(b"WAYLAND_DISPLAY=wayland-01\0"));
        assert!(!ours.matches(b"HOME=/home/a\0"));
    }

    #[test]
    fn x11_session_compares_the_x_display() {
        let ours = SessionDisplay {
            wayland: None,
            x11: Some(b":1".to_vec()),
        };
        assert!(ours.matches(b"DISPLAY=:1\0"));
        assert!(!ours.matches(b"DISPLAY=:0\0"));
        assert!(!ours.matches(b"XDISPLAY=:1\0"));
    }

    #[test]
    fn with_no_display_known_the_same_user_check_decides() {
        let ours = SessionDisplay {
            wayland: None,
            x11: None,
        };
        assert!(ours.matches(b"DISPLAY=:0\0"));
        assert!(ours.matches(b""));
    }

    #[test]
    fn new_game_default_is_the_old_hard_coded_value() {
        assert_eq!(GuiConfig::default().new_game_saturation(), 1.4);
        // A gui.toml written before the setting existed keeps the old value.
        let old: GuiConfig = toml::from_str("dark_mode = true\n").unwrap();
        assert_eq!(old.new_game_saturation(), 1.4);
        assert_eq!(sat_to_percent(old.new_game_saturation()), 40);
    }

    #[test]
    fn new_game_setting_round_trips_through_gui_toml() {
        let config = GuiConfig {
            dark_mode: false,
            new_game_saturation: 0.75,
        };
        let text = toml::to_string(&config).unwrap();
        let back: GuiConfig = toml::from_str(&text).unwrap();
        assert_eq!(back, config);
        assert_eq!(sat_to_percent(back.new_game_saturation()), -25);
    }

    #[test]
    fn out_of_range_new_game_value_is_clamped_and_broken_falls_back() {
        let low: GuiConfig = toml::from_str("new_game_saturation = -3.0\n").unwrap();
        assert_eq!(low.new_game_saturation(), 0.0);
        let high: GuiConfig = toml::from_str("new_game_saturation = 9.0\n").unwrap();
        assert_eq!(high.new_game_saturation(), 2.0);
        let broken = GuiConfig {
            dark_mode: true,
            new_game_saturation: f32::NAN,
        };
        assert_eq!(broken.new_game_saturation(), 1.4);
    }

    #[test]
    fn added_game_uses_the_given_saturation() {
        let mut profiles = Profiles::default();
        add_or_update(&mut profiles, "EscapeFromTarkov.exe", None, 0.5);
        assert_eq!(profiles.profiles[0].saturation, 0.5);
        assert_eq!(profiles.profiles[0].name, "EscapeFromTarkov");
    }
}
