//! `satur8-gui` - the Satur8 desktop app (Slint).
//!
//! Main page: the desktop vibrance strip and per-game cards with the game's
//! real Steam icon, a saturation slider and an enable toggle; games are added
//! by name from one list of the installed Steam games and what is running.
//! Status page: health checks, repair, per-game switching and an activity
//! log. About page. Reads/writes the same `profiles.toml` the CLI and daemon
//! use.
//!
//! In a Steam gaming session (Steam Deck Game Mode) the same app shows its
//! own layout for a touch screen and the Deck's buttons (`ui/touch.slint`,
//! `gm.rs`, `pad.rs`), at a size that fits the screen (`mode.rs`).
//!
//! Game art comes from the user's *local* Steam cache - nothing is bundled.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod gm;
mod mode;
mod pad;
mod picker;
mod status;
mod steam;

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use satur8_backend::{all_outputs, select_backend, RestoreGuard};
use satur8_core::levels::desktop_picture;
use satur8_core::proc;
use satur8_core::{Backend, Levels, Profile, Profiles, Saturation};
use serde::{Deserialize, Serialize};
use slint::{Color, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

slint::include_modules!();

/// The app ID the compositor matches the window to: the base name of the
/// desktop entry `packaging/satur8.desktop`, whose `Icon=` names the icon the
/// installers put in the hicolor theme. Slint sends it as the Wayland app ID
/// and as the class half of the X11 `WM_CLASS`.
const APP_ID: &str = "satur8";

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

/// What a change to "Starting saturation for new games" does to the settings
/// file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NewGameSave {
    /// Write it now.
    Now,
    /// Write it once the changes stop.
    Soon,
    /// The file already has it.
    Nothing,
}

/// Take a value from the new-game slider. The setting in memory follows every
/// change, so a game added right away starts at what the slider shows.
/// `unsaved` says the settings file does not have the value yet. A change
/// that is `done` is written now. One that is not must still be written:
/// L2 and R2 change the slider and are never followed by a `done`.
fn new_game_change(
    config: &mut GuiConfig,
    unsaved: &mut bool,
    saturation: f32,
    done: bool,
) -> NewGameSave {
    if config.new_game_saturation != saturation {
        config.new_game_saturation = saturation;
        *unsaved = true;
    }
    match (*unsaved, done) {
        (false, _) => NewGameSave::Nothing,
        (true, true) => NewGameSave::Now,
        (true, false) => NewGameSave::Soon,
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

/// Everything the window's callbacks act on. One of these lives for the whole
/// run, on the UI thread; the desktop layout and the Game Mode layout both
/// drive it.
struct App {
    ui: slint::Weak<AppWindow>,
    /// A gamescope session: the Game Mode layout is shown.
    touch: bool,
    /// The backend is gamescope's: Steam Game Mode, where what is running
    /// comes from gamescope and not from the process list.
    game_mode: bool,
    /// The AppID Steam gave this app's own library shortcut, if it was
    /// started from one.
    own_app_id: Option<u32>,
    state: RefCell<State>,
    games: Rc<VecModel<GameRow>>,
    activity: Rc<VecModel<LogRow>>,
    gui_config: RefCell<GuiConfig>,
    // The "Add game" list: what was read when it opened, and what it shows.
    library: RefCell<picker::Library>,
    running: RefCell<picker::Running>,
    query: RefCell<String>,
    rows: RefCell<Vec<picker::Row>>,
    steam_rows: Rc<VecModel<SteamRow>>,
    // Game Mode.
    gm: RefCell<gm::State>,
    gm_games: Rc<VecModel<GmGame>>,
    gm_picks: Rc<VecModel<GmPick>>,
    gm_prompts: Rc<VecModel<GmPrompt>>,
    /// Saves a slider change made with the pad once the changes stop.
    save_timer: slint::Timer,
    /// The new-game setting differs from the settings file.
    new_game_unsaved: std::cell::Cell<bool>,
    new_game_timer: slint::Timer,
    /// How often the key held on the keyboard has repeated.
    key_repeat: Cell<u32>,
    input_debug: bool,
}

thread_local! {
    /// The app, for closures that arrive from other threads through the
    /// event loop (the gamepad reader) and for timers. UI thread only.
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
}

fn with_app(run: impl FnOnce(&App)) {
    let app = APP.with(|app| app.borrow().clone());
    if let Some(app) = app {
        run(&app);
    }
}

fn main() -> Result<(), slint::PlatformError> {
    // Before the window exists: Slint reads the scale when it creates it.
    let touch = mode::in_gamescope_session(|name| std::env::var(name).ok());
    let sizing = if touch { gamescope_sizing() } else { None };
    let ui = AppWindow::new()?;
    ui.set_touch(touch);
    if let Some(sizing) = sizing {
        fit_window(&ui, sizing);
    }
    // After AppWindow::new(), which creates Slint's platform (before that
    // set_xdg_app_id only returns NoPlatform), and before ui.run() shows the
    // window, which is when the ID is read. A failure only costs the icon.
    if let Err(error) = slint::set_xdg_app_id(APP_ID) {
        eprintln!("satur8-gui: could not set the app ID {APP_ID}: {error}");
    }
    ui.set_version_text(format!("v{}", env!("CARGO_PKG_VERSION")).into());
    let gui_config = load_gui_config();
    ui.global::<Pal>().set_dark(gui_config.dark_mode);
    ui.set_new_game_percent(sat_to_percent(gui_config.new_game_saturation()));

    let profiles = load_profiles(Profiles::default());
    let backend = match select_backend() {
        Ok(backend) => {
            let desktop = desktop_picture(&profiles, backend.per_output()).levels;
            Some(RestoreGuard::inactive(backend, desktop))
        }
        Err(error) => {
            eprintln!("satur8-gui: {error}");
            None
        }
    };
    let backend_name = backend.as_ref().map(|b| b.backend().name().to_string());
    ui.set_backend_ok(backend.is_some());
    ui.set_backend_name(backend_name.clone().unwrap_or_else(|| "no".into()).into());

    // Activity
    let activity = Rc::new(VecModel::<LogRow>::default());
    log(&activity, true, "Started Satur8");
    match &backend_name {
        Some(name) => log(&activity, false, &format!("{name} backend active")),
        None => log(&activity, false, "No supported backend in this session"),
    }
    ui.set_activity(ModelRc::from(activity.clone()));
    ui.set_default_percent(sat_to_percent(profiles.default_saturation));

    let input_debug = std::env::var_os("SATUR8_GUI_INPUT_DEBUG").is_some_and(|v| !v.is_empty())
        || config_dir().join("input-debug").exists();
    let app = Rc::new(App {
        ui: ui.as_weak(),
        touch,
        game_mode: backend_name.as_deref() == Some("gamescope"),
        own_app_id: std::env::var("SteamAppId")
            .ok()
            .and_then(|id| id.trim().parse().ok())
            .filter(|id| *id != 0),
        state: RefCell::new(State { backend, profiles }),
        games: Rc::new(VecModel::default()),
        activity,
        gui_config: RefCell::new(gui_config),
        library: RefCell::new(picker::Library::Games(Vec::new())),
        running: RefCell::new(picker::Running::Procs(Vec::new())),
        query: RefCell::new(String::new()),
        rows: RefCell::new(Vec::new()),
        steam_rows: Rc::new(VecModel::default()),
        gm: RefCell::new(gm::State::new(&gm::Layout::default())),
        gm_games: Rc::new(VecModel::default()),
        gm_picks: Rc::new(VecModel::default()),
        gm_prompts: Rc::new(VecModel::default()),
        save_timer: slint::Timer::default(),
        new_game_unsaved: std::cell::Cell::new(false),
        new_game_timer: slint::Timer::default(),
        key_repeat: Cell::new(0),
        input_debug,
    });
    APP.with(|slot| *slot.borrow_mut() = Some(app.clone()));
    ui.set_games(ModelRc::from(app.games.clone()));
    ui.set_steam_rows(ModelRc::from(app.steam_rows.clone()));
    app.rebuild();

    // Off-screen screenshot helper: jump straight to a page
    // (0 = Main, 1 = Status, 2 = About).
    let start_page = std::env::var("VIBRANCE_GUI_NAV")
        .ok()
        .and_then(|n| n.parse::<i32>().ok());
    if let Some(page) = start_page {
        ui.set_nav(page);
    }
    if let Ok(value) = std::env::var("SATUR8_GUI_DARK") {
        ui.global::<Pal>()
            .set_dark(matches!(value.as_str(), "1" | "true" | "yes" | "on"));
    }

    // ---------------- callbacks ----------------
    ui.on_dark_mode_changed(|dark_mode| {
        with_app(|app| {
            let mut config = app.gui_config.borrow_mut();
            config.dark_mode = dark_mode;
            save_gui_config(&config);
        })
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
    ui.on_new_game_changed(|percent| with_app(|app| app.set_new_game(percent, false)));
    ui.on_new_game_released(|percent| with_app(|app| app.set_new_game(percent, true)));
    ui.on_sysinfo_copied(|| {
        with_app(|app| log(&app.activity, true, "Copied system info to the clipboard"))
    });
    ui.on_report_bug(|| {
        with_app(|app| {
            let info = LAST_FACTS.with(|facts| {
                facts.borrow().as_ref().map(|facts| {
                    status::system_info_for_this_machine(facts, &status::health_rows(facts))
                })
            });
            match info {
                // The page copied the system info to the clipboard before
                // calling this; the address opened here carries none of it.
                Some(_) => {
                    open_url(&status::bug_report_url());
                    log(
                        &app.activity,
                        true,
                        "Opened a new bug report in your browser; paste the copied system info into it",
                    );
                }
                None => log(
                    &app.activity,
                    false,
                    "Still checking; try Report a bug again in a moment",
                ),
            }
        })
    });
    ui.on_open_link(|url| {
        open_url(url.as_str());
    });
    // Runs on every slider move: apply live, keep the value in memory.
    // profiles.toml is written once, at the end of the drag (or by the
    // Restore button).
    ui.on_default_changed(|percent| with_app(|app| app.set_rest(percent, false)));
    ui.on_default_released(|percent| with_app(|app| app.set_rest(percent, true)));
    ui.on_percent_changed(|i, percent| with_app(|app| app.set_game(i as usize, percent, true)));
    ui.on_toggle_enabled(|i, enabled| with_app(|app| app.set_enabled(i as usize, enabled)));
    ui.on_remove(|i| with_app(|app| app.remove(i as usize)));

    // ---------------- "Add game" list ----------------
    // The library and what is running are read when the list opens and on
    // its Refresh button, never on a timer.
    ui.on_steam_picker_opened(|| with_app(|app| app.open_list()));
    ui.on_steam_refresh(|| with_app(|app| app.read_list()));
    ui.on_steam_filter_changed(|text| {
        with_app(|app| {
            *app.query.borrow_mut() = text.to_string();
            app.show_list();
        })
    });
    ui.on_pick_steam(|index| {
        with_app(|app| {
            if let Ok(index) = usize::try_from(index) {
                app.pick(index);
            }
        })
    });
    // Enter in the filter field; true closes the list.
    ui.on_steam_filter_accepted(|| {
        let mut picked = false;
        with_app(|app| {
            let only = picker::enter_pick(&app.rows.borrow());
            if let Some(index) = only {
                app.pick(index);
                picked = true;
            }
        });
        picked
    });

    ui.on_clear_activity(|| with_app(|app| app.activity.set_vec(Vec::<LogRow>::new())));

    // ---------------- Steam Game Mode ----------------
    if touch {
        app.start_game_mode(&ui, start_page);
    }
    // Started from its own Steam library entry: give that entry its artwork,
    // once. Steam told this process the entry's AppID.
    if let Some(app_id) = app.own_app_id {
        let placed = satur8_core::steam_shortcut::place_artwork_for_running_app(app_id);
        if placed > 0 {
            log(
                &app.activity,
                true,
                "Put Satur8's artwork on its Steam library entry; Steam shows it after it restarts",
            );
        }
    }

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

impl App {
    /// Draw the games again from the profiles, in both layouts.
    fn rebuild(&self) {
        rebuild_games(&self.games, &self.state.borrow().profiles);
        self.gm_games.set_vec(
            self.games
                .iter()
                .map(|row| GmGame {
                    name: row.name,
                    percent: row.percent,
                    enabled: row.enabled,
                    initial: row.initial,
                    icon: row.icon,
                    has_icon: row.has_icon,
                })
                .collect::<Vec<_>>(),
        );
    }

    fn save(&self) {
        self.save_timer.stop();
        save(&self.state.borrow().profiles);
    }

    /// Save once the changes stop: a slider held down on the pad changes many
    /// times a second.
    fn save_soon(&self) {
        self.save_timer.start(
            slint::TimerMode::SingleShot,
            std::time::Duration::from_millis(400),
            || with_app(|app| app.save()),
        );
    }

    /// The saturation of everything without a profile ("Desktop vibrance";
    /// "General" in Game Mode): show it on screen now, and save it
    /// when the change is `done`.
    fn set_rest(&self, percent: f32, done: bool) {
        let Some(sat) = percent_to_sat(percent) else {
            eprintln!("satur8-gui: ignored non-finite default saturation input");
            return;
        };
        {
            let mut st = self.state.borrow_mut();
            st.profiles.default_saturation = sat;
            let st = &mut *st;
            let profiles = &st.profiles;
            if let Some(b) = st.backend.as_mut() {
                let desktop = desktop_picture(profiles, b.backend().per_output()).levels;
                b.set_restore_to(desktop.clone());
                if show_desktop(b.backend_mut(), &desktop).is_ok() {
                    b.disarm();
                }
            }
        }
        if let Some(ui) = self.ui.upgrade() {
            ui.set_default_percent(percent.round() as i32);
            ui.global::<Gm>().set_rest_percent(percent.round() as i32);
        }
        if done {
            self.save();
        } else if self.touch {
            self.save_soon();
        }
    }

    /// A game's saturation. The desktop layout saves every change; Game Mode
    /// saves when the change is `done` or has stopped.
    fn set_game(&self, index: usize, percent: f32, done: bool) {
        let Some(sat) = percent_to_sat(percent) else {
            eprintln!("satur8-gui: ignored non-finite profile saturation input");
            return;
        };
        {
            let mut st = self.state.borrow_mut();
            let Some(profile) = st.profiles.profiles.get_mut(index) else {
                return;
            };
            profile.saturation = sat;
        }
        let percent = percent.round() as i32;
        if let Some(mut row) = self.games.row_data(index) {
            row.percent = percent;
            self.games.set_row_data(index, row);
        }
        if let Some(mut row) = self.gm_games.row_data(index) {
            row.percent = percent;
            self.gm_games.set_row_data(index, row);
        }
        if done {
            self.save();
        } else {
            self.save_soon();
        }
    }

    /// Status page > Settings > "Starting saturation for new games". A change
    /// that is `done` is written to the settings file at once; one that is
    /// not (a drag in progress, or L2 and R2, which have no release) is
    /// written once the changes stop, like the other sliders.
    fn set_new_game(&self, percent: f32, done: bool) {
        if let Some(ui) = self.ui.upgrade() {
            ui.set_new_game_percent(percent.round() as i32);
        }
        let Some(sat) = percent_to_sat(percent.round()) else {
            return;
        };
        let mut unsaved = self.new_game_unsaved.get();
        let step = new_game_change(&mut self.gui_config.borrow_mut(), &mut unsaved, sat, done);
        self.new_game_unsaved.set(unsaved);
        match step {
            NewGameSave::Now => {
                self.new_game_timer.stop();
                self.save_new_game();
            }
            NewGameSave::Soon => self.new_game_timer.start(
                slint::TimerMode::SingleShot,
                std::time::Duration::from_millis(400),
                || with_app(|app| app.save_new_game()),
            ),
            NewGameSave::Nothing => {}
        }
    }

    /// Write the new-game setting to the settings file, if it is not there.
    fn save_new_game(&self) {
        if !self.new_game_unsaved.replace(false) {
            return;
        }
        let config = *self.gui_config.borrow();
        save_gui_config(&config);
        log(
            &self.activity,
            false,
            &format!(
                "New games now start at {:+.0}%",
                (config.new_game_saturation - 1.0) * 100.0
            ),
        );
    }

    fn set_enabled(&self, index: usize, enabled: bool) {
        let name = {
            let mut st = self.state.borrow_mut();
            let Some(profile) = st.profiles.profiles.get_mut(index) else {
                return;
            };
            profile.enabled = enabled;
            profile.name.clone()
        };
        self.save();
        if let Some(mut row) = self.gm_games.row_data(index) {
            row.enabled = enabled;
            self.gm_games.set_row_data(index, row);
        }
        if self.touch {
            // The desktop card keeps its own switch state; a rebuild there
            // would only redraw it.
            if let Some(mut row) = self.games.row_data(index) {
                row.enabled = enabled;
                self.games.set_row_data(index, row);
            }
        }
        log(
            &self.activity,
            false,
            &format!("{} {}", name, if enabled { "enabled" } else { "disabled" }),
        );
    }

    fn remove(&self, index: usize) {
        // Indices shift, so the card the list marked is no longer known.
        if let Some(ui) = self.ui.upgrade() {
            ui.set_picked_index(-1);
        }
        let name = {
            let mut st = self.state.borrow_mut();
            if index >= st.profiles.profiles.len() {
                return;
            }
            st.profiles.profiles.remove(index).name
        };
        self.save();
        self.rebuild();
        log(&self.activity, false, &format!("Removed {name}"));
    }

    /// The "Add game" list opens: start with an empty filter and read the
    /// library and what is running.
    fn open_list(&self) {
        self.query.borrow_mut().clear();
        if let Some(ui) = self.ui.upgrade() {
            ui.set_picked_index(-1);
        }
        self.read_list();
    }

    fn read_list(&self) {
        *self.library.borrow_mut() = read_steam_library();
        *self.running.borrow_mut() = read_running(self.game_mode);
        self.show_list();
    }

    fn show_list(&self) {
        let view = picker::view(
            &self.library.borrow(),
            &self.state.borrow().profiles,
            &self.running.borrow(),
            self.own_app_id,
            &self.query.borrow(),
            self.game_mode,
        );
        self.steam_rows.set_vec(
            view.rows
                .iter()
                .map(|row| SteamRow {
                    name: row.name.clone().into(),
                    detail: row.detail.clone().into(),
                    has_profile: row.has_profile,
                    running: row.running,
                })
                .collect::<Vec<_>>(),
        );
        self.gm_picks.set_vec(
            view.rows
                .iter()
                .map(|row| GmPick {
                    name: row.name.clone().into(),
                    detail: row.detail.clone().into(),
                    has_profile: row.has_profile,
                    running: row.running,
                })
                .collect::<Vec<_>>(),
        );
        if let Some(ui) = self.ui.upgrade() {
            ui.set_steam_message(view.message.clone().into());
            ui.set_steam_filter_shown(view.show_filter);
            ui.global::<Gm>().set_pick_message(view.message.into());
        }
        *self.rows.borrow_mut() = view.rows;
    }

    /// A row of the list was picked: add its profile, or open the one it
    /// has, and show it. Returns the profile's index.
    fn pick(&self, row: usize) -> Option<usize> {
        let row = self.rows.borrow().get(row).cloned()?;
        let (index, text, changed) = {
            let mut st = self.state.borrow_mut();
            let saturation = self.gui_config.borrow().new_game_saturation();
            let choice = picker::pick(&st.profiles, &row, saturation);
            let created = matches!(choice, picker::Pick::Create(_));
            let (index, changed) = picker::apply(&mut st.profiles, choice);
            let text = picker::log_line(&st.profiles, index, changed, created);
            (index, text, changed)
        };
        if changed {
            self.save();
        }
        self.rebuild();
        log(&self.activity, changed, &text);
        if let Some(ui) = self.ui.upgrade() {
            ui.set_picked_index(index as i32);
            ui.set_picked_serial(ui.get_picked_serial().wrapping_add(1));
        }
        Some(index)
    }

    // ---------------------------------------------------------- Game Mode

    /// Wire the Game Mode layout: its state, its touches, the keyboard and
    /// the gamepad.
    fn start_game_mode(&self, ui: &AppWindow, start_page: Option<i32>) {
        let gm_ui = ui.global::<Gm>();
        gm_ui.set_games(ModelRc::from(self.gm_games.clone()));
        gm_ui.set_picks(ModelRc::from(self.gm_picks.clone()));
        gm_ui.set_prompts(ModelRc::from(self.gm_prompts.clone()));
        gm_ui.set_rest_percent(ui.get_default_percent());
        *self.gm.borrow_mut() = gm::State::new(&self.gm_layout());
        if let Some(page) = start_page {
            self.gm_show_page(gm::Page::from_index(page), false);
        }

        gm_ui.on_touched(|kind, index, part| {
            with_app(|app| {
                if let Some(focus) = focus_from_ui(kind, index, part) {
                    app.gm.borrow_mut().touched(focus);
                    app.gm_sync(false);
                }
            })
        });
        gm_ui.on_percent(|slider, index, value, done| {
            with_app(|app| {
                let slider = match slider {
                    0 => gm::Slider::Rest,
                    1 => gm::Slider::Game(index.max(0) as usize),
                    _ => gm::Slider::NewGame,
                };
                app.gm_set_percent(slider, value.round() as i32, done);
            })
        });
        gm_ui.on_toggle_game(|index| with_app(|app| app.gm_toggle(index.max(0) as usize)));
        gm_ui.on_ask_remove(|index| {
            with_app(|app| {
                app.gm
                    .borrow_mut()
                    .touched(gm::Focus::Confirm(index.max(0) as usize, false));
                app.gm_sync(false);
            })
        });
        gm_ui.on_answer_remove(|index, remove| {
            with_app(|app| {
                let index = index.max(0) as usize;
                if remove {
                    app.gm_remove(index);
                } else {
                    app.gm
                        .borrow_mut()
                        .touched(gm::Focus::Game(index, gm::Part::Remove));
                }
                app.gm_sync(false);
            })
        });
        gm_ui.on_open_picker(|| {
            with_app(|app| {
                app.open_list();
                app.gm.borrow_mut().picker_opened_by_touch();
                app.gm_sync(true);
            })
        });
        gm_ui.on_close_picker(|| {
            with_app(|app| {
                app.gm.borrow_mut().picker_closed();
                app.gm.borrow_mut().highlight = false;
                app.gm_sync(false);
            })
        });
        gm_ui.on_pick(|row| {
            with_app(|app| {
                app.gm.borrow_mut().highlight = false;
                app.gm_pick(row.max(0) as usize);
            })
        });
        gm_ui.on_show_page(|page| {
            with_app(|app| app.gm_show_page(gm::Page::from_index(page), true))
        });
        gm_ui.on_key(|text, repeat| {
            let mut handled = false;
            with_app(|app| {
                if let Some(button) = gm::button_for_key(text.as_str()) {
                    let count = if repeat { app.key_repeat.get() + 1 } else { 0 };
                    app.key_repeat.set(count);
                    app.gm_press(gm::Press {
                        button,
                        repeat: count,
                    });
                    handled = true;
                }
            });
            handled
        });
        self.gm_sync(false);

        // The gamepad: Steam shows the Deck's controls as one.
        let log_path = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
            })
            .map(|state| state.join("satur8/input-debug.log"));
        let debug = self.input_debug;
        // The pad is one device for the whole session, and a game in front
        // gets the same presses: act on them only while this app is the one
        // in front. In Steam Game Mode gamescope says which app that is.
        let watched = match (self.game_mode, self.own_app_id) {
            (true, Some(own)) => {
                spawn_front_watch(own);
                true
            }
            _ => false,
        };
        pad::spawn(pad::DebugLog::new(debug, log_path), move |press| {
            if watched && !IN_FRONT.load(Ordering::Acquire) {
                if debug {
                    eprintln!("satur8-gui input: {press:?} ignored, another app is in front");
                }
                return;
            }
            let _ = slint::invoke_from_event_loop(move || {
                with_app(|app| {
                    // Anywhere else the window's own keyboard focus decides.
                    let active = watched
                        || app
                            .ui
                            .upgrade()
                            .is_some_and(|ui| ui.global::<Gm>().get_window_active());
                    if active {
                        app.gm_press(press);
                    } else if debug {
                        eprintln!(
                            "satur8-gui input: {press:?} ignored, the window is not in front"
                        );
                    }
                })
            });
        });
    }

    /// What the Game Mode screen holds, for the focus model.
    fn gm_layout(&self) -> gm::Layout {
        let st = self.state.borrow();
        let ui = self.ui.upgrade();
        let busy = ui
            .as_ref()
            .is_some_and(|ui| ui.get_checking() || ui.get_repairing() || ui.get_switch_busy());
        let button = |enabled: bool| gm::Item::Button { enabled };
        gm::Layout {
            rest_percent: sat_to_percent(st.profiles.default_saturation),
            games: st
                .profiles
                .profiles
                .iter()
                .map(|p| (sat_to_percent(p.saturation), p.enabled))
                .collect(),
            picks: self
                .rows
                .borrow()
                .iter()
                .map(|row| row.has_profile)
                .collect(),
            status: vec![
                button(!busy),
                button(!busy),
                gm::Item::Toggle {
                    enabled: !busy && ui.as_ref().is_some_and(|ui| ui.get_switch_available()),
                },
                gm::Item::Slider,
                button(
                    ui.as_ref()
                        .is_some_and(|ui| !ui.get_sysinfo_text().is_empty()),
                ),
                button(true),
            ],
            about: vec![button(true); 3],
            new_game_percent: ui.as_ref().map_or(0, |ui| ui.get_new_game_percent()),
        }
    }

    /// Hand the Game Mode state to the layout. `reveal` scrolls the row in
    /// focus into view.
    fn gm_sync(&self, reveal: bool) {
        let Some(ui) = self.ui.upgrade() else { return };
        let layout = self.gm_layout();
        self.gm.borrow_mut().settle(&layout);
        let state = self.gm.borrow().clone();
        let gm_ui = ui.global::<Gm>();
        let (kind, index, part) = focus_to_ui(state.focus);
        gm_ui.set_page(state.page.index());
        gm_ui.set_focus_kind(kind);
        gm_ui.set_focus_index(index);
        gm_ui.set_focus_part(part);
        gm_ui.set_highlight(state.highlight);
        gm_ui.set_adjusting(state.adjusting.is_some());
        gm_ui.set_picker_open(state.picker_open);
        self.gm_prompts.set_vec(
            gm::prompts(&state, &layout)
                .into_iter()
                .map(|prompt| GmPrompt {
                    glyph: prompt.glyphs.first().copied().unwrap_or_default().into(),
                    glyph2: prompt.glyphs.get(1).copied().unwrap_or_default().into(),
                    label: prompt.label.into(),
                })
                .collect::<Vec<_>>(),
        );
        if reveal {
            gm_ui.set_reveal(gm_ui.get_reveal().wrapping_add(1));
        }
    }

    /// A button press, from the pad or the keyboard.
    fn gm_press(&self, press: gm::Press) {
        let layout = self.gm_layout();
        let effects = gm::press(&mut self.gm.borrow_mut(), &layout, press);
        if self.input_debug {
            eprintln!(
                "satur8-gui input: {press:?} -> {:?} {effects:?}",
                self.gm.borrow().focus
            );
        }
        for effect in effects {
            self.gm_run(effect);
        }
        self.gm_sync(true);
    }

    fn gm_run(&self, effect: gm::Effect) {
        match effect {
            gm::Effect::Percent {
                slider,
                value,
                done,
            } => self.gm_set_percent(slider, value, done),
            gm::Effect::ToggleGame(index) => self.gm_toggle(index),
            gm::Effect::RemoveGame(index) => self.gm_remove(index),
            gm::Effect::OpenPicker => self.open_list(),
            gm::Effect::ClosePicker => {}
            gm::Effect::PickRow(row) => self.gm_pick(row),
            gm::Effect::ShowPage(page) => {
                STATUS_VISIBLE.store(page == gm::Page::Status, Ordering::Release);
                if let Some(ui) = self.ui.upgrade() {
                    ui.set_nav(page.index());
                }
            }
            gm::Effect::PressItem(item) => self.gm_press_item(item),
        }
    }

    fn gm_set_percent(&self, slider: gm::Slider, value: i32, done: bool) {
        match slider {
            gm::Slider::Rest => self.set_rest(value as f32, done),
            gm::Slider::Game(index) => self.set_game(index, value as f32, done),
            gm::Slider::NewGame => self.set_new_game(value as f32, done),
        }
    }

    fn gm_toggle(&self, index: usize) {
        let enabled = self
            .state
            .borrow()
            .profiles
            .profiles
            .get(index)
            .map(|profile| profile.enabled);
        if let Some(enabled) = enabled {
            self.set_enabled(index, !enabled);
            self.gm_sync(false);
        }
    }

    fn gm_remove(&self, index: usize) {
        self.remove(index);
        let remaining = self.state.borrow().profiles.profiles.len();
        self.gm.borrow_mut().removed(index, remaining);
    }

    fn gm_pick(&self, row: usize) {
        if let Some(index) = self.pick(row) {
            let percent = self
                .state
                .borrow()
                .profiles
                .profiles
                .get(index)
                .map_or(0, |profile| sat_to_percent(profile.saturation));
            self.gm.borrow_mut().picked(index, percent);
        } else {
            self.gm.borrow_mut().picker_closed();
        }
        self.gm_sync(true);
    }

    /// Open a page. `by_touch` hides the highlight.
    fn gm_show_page(&self, page: gm::Page, by_touch: bool) {
        let layout = self.gm_layout();
        {
            let mut state = self.gm.borrow_mut();
            let highlight = state.highlight;
            state.page_touched(page, &layout);
            state.highlight = highlight && !by_touch;
        }
        self.gm_run(gm::Effect::ShowPage(page));
        self.gm_sync(false);
    }

    /// A on a Status or About item.
    fn gm_press_item(&self, item: usize) {
        let Some(ui) = self.ui.upgrade() else { return };
        let page = self.gm.borrow().page;
        match (page, item) {
            (gm::Page::Status, 0) => ui.invoke_check_health(),
            (gm::Page::Status, 1) => ui.invoke_repair(),
            (gm::Page::Status, 2) => ui.invoke_set_switching(!ui.get_switch_on()),
            (gm::Page::Status, 4) => {
                let gm_ui = ui.global::<Gm>();
                gm_ui.set_copy(gm_ui.get_copy().wrapping_add(1));
            }
            (gm::Page::Status, 5) => ui.invoke_report_bug(),
            (gm::Page::About, 0) => open_url("https://satur8.app"),
            (gm::Page::About, 1) => open_url("https://x.com/NtrpyDev"),
            (gm::Page::About, 2) => open_url("https://github.com/NtrpyDev/satur8"),
            _ => {}
        }
    }
}

/// Whether gamescope's focused app is this one. True until gamescope says
/// otherwise: the app has just been started, so it is in front.
static IN_FRONT: AtomicBool = AtomicBool::new(true);

/// Keep [`IN_FRONT`] up to date: follow gamescope's focus properties on its
/// first Xwayland server and compare the focused app with `own`, the AppID
/// Steam gave this app's shortcut. Sleeps in the X event wait; reconnects if
/// gamescope restarts.
fn spawn_front_watch(own: u32) {
    use satur8_gamescope::session;
    let result = std::thread::Builder::new()
        .name("front-watch".into())
        .spawn(move || loop {
            if let Some((_, root)) = session::find_session() {
                if root.watch_properties().is_ok() {
                    loop {
                        let Ok(focus) = session::read_focus(&root) else {
                            break;
                        };
                        IN_FRONT.store(focus.app_id == Some(own), Ordering::Release);
                        if root.wait_for_change(&session::FOCUS_PROPERTIES).is_err() {
                            break;
                        }
                    }
                }
            }
            // No session to ask: do not lock the pad out.
            IN_FRONT.store(true, Ordering::Release);
            std::thread::sleep(std::time::Duration::from_secs(2));
        });
    if let Err(error) = result {
        eprintln!("satur8-gui: could not watch gamescope's focus: {error}");
    }
}

/// The focus as the three numbers `ui/touch.slint` draws from.
fn focus_to_ui(focus: gm::Focus) -> (i32, i32, i32) {
    use gm::{Focus, Part, RestPart};
    match focus {
        Focus::Rest(RestPart::Slider) => (0, 0, 0),
        Focus::Rest(RestPart::Reset) => (0, 0, 1),
        Focus::AddGame => (1, 0, 0),
        Focus::Game(index, part) => (
            2,
            index as i32,
            match part {
                Part::Slider => 0,
                Part::Toggle => 1,
                Part::Remove => 2,
            },
        ),
        Focus::Confirm(index, remove) => (3, index as i32, i32::from(remove)),
        Focus::Pick(index) => (4, index as i32, 0),
        Focus::Item(index) => (5, index as i32, 0),
    }
}

/// The other way round, for a touch the layout reports.
fn focus_from_ui(kind: i32, index: i32, part: i32) -> Option<gm::Focus> {
    use gm::{Focus, Part, RestPart};
    let index = usize::try_from(index).ok()?;
    Some(match (kind, part) {
        (0, 0) => Focus::Rest(RestPart::Slider),
        (0, _) => Focus::Rest(RestPart::Reset),
        (1, _) => Focus::AddGame,
        (2, 0) => Focus::Game(index, Part::Slider),
        (2, 1) => Focus::Game(index, Part::Toggle),
        (2, _) => Focus::Game(index, Part::Remove),
        (3, part) => Focus::Confirm(index, part == 1),
        (4, _) => Focus::Pick(index),
        (5, _) => Focus::Item(index),
        _ => return None,
    })
}

/// What is running right now. In Steam Game Mode gamescope says which apps
/// have a window; on a desktop the process list does.
fn read_running(game_mode: bool) -> picker::Running {
    if game_mode {
        let apps = satur8_gamescope::session::find_session()
            .and_then(|(_, root)| satur8_gamescope::session::running_apps(&root).ok())
            .unwrap_or_default();
        picker::Running::Apps(apps)
    } else {
        picker::Running::Procs(
            proc::running_apps()
                .into_iter()
                .map(|app| picker::Proc {
                    exe: app.exe,
                    app_id: app.app_id,
                    is_game: app.is_game,
                })
                .collect(),
        )
    }
}

/// In a gamescope session (Steam Game Mode), fit the window to the output:
/// read the size games are given from the X display this window opens on, set
/// the scale Slint draws at unless the user set one, and return the window
/// size to ask for. `None` anywhere else, and when the size cannot be read.
fn gamescope_sizing() -> Option<mode::Sizing> {
    if !mode::in_gamescope_session(|name| std::env::var(name).ok()) {
        return None;
    }
    let output = std::env::var("DISPLAY").ok().and_then(|display| {
        satur8_gamescope::session::X11Root::connect(&display)
            .and_then(|root| root.size())
            .map_err(|error| eprintln!("satur8-gui: could not read the screen size: {error:#}"))
            .ok()
    });
    let user_scale = std::env::var_os("SLINT_SCALE_FACTOR").is_some_and(|value| !value.is_empty());
    let sizing = mode::window_sizing(output, user_scale)?;
    if let Some(scale) = sizing.scale {
        // Slint's winit backend reads this when it creates the window
        // (i-slint-backend-winit, winitwindowadapter.rs). No other thread
        // exists yet.
        std::env::set_var("SLINT_SCALE_FACTOR", mode::scale_text(scale));
        // winit sizes a window it has not created yet with its own X11 scale
        // (winit, x11/util/randr.rs); give it the same one, or the window
        // opens larger than the output and is only then resized.
        if std::env::var_os("WINIT_X11_SCALE_FACTOR").is_none() {
            std::env::set_var("WINIT_X11_SCALE_FACTOR", mode::scale_text(scale));
        }
    }
    Some(sizing)
}

/// Make the window the whole output, so a touch lands on what is drawn under
/// it. Asked for twice: before the window exists, so it opens at that size,
/// and once it is on screen, in device pixels, which is exact whatever scale
/// is in use.
fn fit_window(ui: &AppWindow, sizing: mode::Sizing) {
    let window = ui.window();
    window.set_position(slint::PhysicalPosition::new(0, 0));
    if let Some(scale) = sizing.scale {
        // Before the window exists Slint keeps a requested size in logical
        // pixels and applies the scale when it creates the window.
        let (width, height) = (sizing.width as f32 / scale, sizing.height as f32 / scale);
        ui.set_open_width(width);
        ui.set_open_height(height);
        window.set_size(slint::LogicalSize::new(width, height));
    }
    let weak = ui.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_millis(50), move || {
        let Some(ui) = weak.upgrade() else { return };
        let wanted = slint::PhysicalSize::new(sizing.width, sizing.height);
        let opened = ui.window().size();
        if opened != wanted {
            eprintln!(
                "satur8-gui: window opened at {}x{}, resizing to the {}x{} output",
                opened.width, opened.height, wanted.width, wanted.height
            );
            ui.window().set_size(wanted);
        }
        ui.window().set_position(slint::PhysicalPosition::new(0, 0));
    });
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
    let mut rows = status::health_rows(&facts);
    // The system info for a bug report keeps every row.
    let sysinfo = status::system_info_for_this_machine(&facts, &rows);
    if ui.get_touch() {
        rows = status::game_mode_rows(rows);
    }
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
    ui.set_sysinfo_text(sysinfo.into());
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
                    status::applied_note(sat, live.screen_problem).to_string(),
                ),
                None => {
                    // Not readable here: show what should be applied.
                    let profiles = load_profiles(Profiles::default());
                    let sat = live
                        .active_profile
                        .as_deref()
                        .filter(|name| !name.is_empty())
                        .and_then(|name| profiles.by_name(name))
                        .map(|p| p.shown_saturation().get())
                        .unwrap_or(profiles.default_saturation);
                    (
                        percent_text(sat),
                        "expected; this session cannot report it".to_string(),
                    )
                }
            };
            // A profile that is off is still the game's profile; say so. And
            // name it the way its row does.
            let mut live = live;
            let mut active_is_off = false;
            if let Some(name) = live.active_profile.clone().filter(|name| !name.is_empty()) {
                if let Some(profile) = load_profiles(Profiles::default()).by_name(&name) {
                    active_is_off = !profile.enabled;
                    let library = std::cell::OnceCell::new();
                    live.active_profile = Some(picker::display_name(profile, |app_id| {
                        library_name(&library, app_id)
                    }));
                }
            }
            let active = status::describe_active(&live, live.game_mode, active_is_off);
            let focused = status::describe_focused(&live);
            let w = w.clone();
            let keep_going = slint::invoke_from_event_loop(move || {
                if let Some(ui) = w.upgrade() {
                    ui.set_live_daemon(live.daemon_running);
                    ui.set_live_profile(active.into());
                    ui.set_live_saturation(saturation.into());
                    ui.set_live_saturation_note(note.into());
                    ui.set_live_color_off(live.screen_problem.is_some());
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

/// Show the desktop picture. An unchanged one (within the slider's step) is
/// a reset, which lets the backend release its per-frame cost.
fn show_desktop(
    backend: &mut dyn Backend,
    desktop: &Levels,
) -> Result<(), satur8_core::BackendError> {
    match desktop.uniform_value() {
        Some(sat) if (sat.get() - 1.0).abs() < 1e-3 => backend.reset(&all_outputs()),
        _ => backend.apply_levels(desktop),
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

/// The Steam library name of an installed game. The library is read at most
/// once per `library`, and only when a profile needs a name from it.
fn library_name(
    library: &std::cell::OnceCell<Vec<satur8_core::steam::SteamGame>>,
    app_id: u32,
) -> Option<String> {
    library
        .get_or_init(satur8_core::steam::installed_games)
        .iter()
        .find(|game| game.app_id == app_id)
        .map(|game| game.name.clone())
}

fn rebuild_games(model: &VecModel<GameRow>, profiles: &Profiles) {
    let library = std::cell::OnceCell::new();
    let rows: Vec<GameRow> = profiles
        .profiles
        .iter()
        .map(|p| {
            let exe = picker::card_subtitle(p);
            let name = picker::display_name(p, |app_id| library_name(&library, app_id));
            let icon = profile_app_id(p)
                .and_then(steam::icon_path)
                .and_then(|path| load_icon(&path));
            GameRow {
                name: name.as_str().into(),
                exe: exe.into(),
                percent: sat_to_percent(p.saturation),
                enabled: p.enabled,
                accent: tile_color(&name),
                initial: name
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

/// What the Steam library holds right now: no install, or its games.
fn read_steam_library() -> picker::Library {
    if satur8_core::steam::steam_roots().is_empty() {
        picker::Library::NoSteam
    } else {
        picker::Library::Games(satur8_core::steam::installed_games())
    }
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

/// Open an address in the browser. The child is waited for on a thread of
/// its own, so it does not stay behind as a zombie.
fn open_url(url: &str) {
    match std::process::Command::new("xdg-open").arg(url).spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(error) => eprintln!("satur8-gui: could not open {url}: {error}"),
    }
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
    fn app_id_names_the_desktop_entry_and_its_icon() {
        let desktop = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packaging")
            .join(format!("{APP_ID}.desktop"));
        let entry = std::fs::read_to_string(&desktop)
            .unwrap_or_else(|error| panic!("no desktop entry at {}: {error}", desktop.display()));
        let value = |key: &str| {
            entry
                .lines()
                .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
                .map(str::trim)
        };
        assert_eq!(value("Icon"), Some(APP_ID));
        assert_eq!(value("StartupWMClass"), Some(APP_ID));
    }

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

    /// L2 and R2 on the Status page's slider send changes that are never
    /// `done`. Each must still reach the setting, and the file once they stop.
    #[test]
    fn a_new_game_change_that_is_never_done_is_still_saved() {
        let mut config = GuiConfig::default();
        let mut unsaved = false;
        // R2 once: +40% to +41%.
        assert_eq!(
            new_game_change(&mut config, &mut unsaved, 1.41, false),
            NewGameSave::Soon
        );
        assert_eq!(config.new_game_saturation, 1.41);
        assert!(unsaved);
        // Held: every step moves the setting and keeps the save pending.
        assert_eq!(
            new_game_change(&mut config, &mut unsaved, 1.42, false),
            NewGameSave::Soon
        );
        assert_eq!(config.new_game_saturation(), 1.42);
        // The delayed save happens (the app clears the flag); nothing is left.
        unsaved = false;
        assert_eq!(
            new_game_change(&mut config, &mut unsaved, 1.42, true),
            NewGameSave::Nothing
        );
    }

    #[test]
    fn a_new_game_change_that_is_done_is_saved_at_once() {
        let mut config = GuiConfig::default();
        let mut unsaved = false;
        // A drag: steps that are not done, then the release.
        assert_eq!(
            new_game_change(&mut config, &mut unsaved, 1.6, false),
            NewGameSave::Soon
        );
        assert_eq!(
            new_game_change(&mut config, &mut unsaved, 1.7, true),
            NewGameSave::Now
        );
        assert_eq!(config.new_game_saturation, 1.7);
        // A release at the value already in the file writes nothing.
        let mut saved = GuiConfig::default();
        let mut unsaved = false;
        assert_eq!(
            new_game_change(&mut saved, &mut unsaved, NEW_GAME_SATURATION, true),
            NewGameSave::Nothing
        );
        assert!(!unsaved);
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
}
