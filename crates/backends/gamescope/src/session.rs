//! The running gamescope session, read and written through its X11 root
//! properties.
//!
//! gamescope publishes its state on the root window of each Xwayland server it
//! runs, and reads color settings from there. Steam Game Mode writes focus only
//! on server 0, and the look is one global no matter which root set it, so
//! Satur8 reads and writes on server 0's root and nowhere else: deleting the
//! look on another root would clear it for everyone.
//!
//! Everything here is plain functions over [`RootProps`], so the logic is
//! tested against an in-memory root; [`X11Root`] is the live implementation.

use std::cell::RefCell;
use std::collections::HashMap;

use anyhow::{Context, Result};
use satur8_core::Saturation;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt, EventMask, PropMode, Window,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use crate::look;

/// The look table path gamescope applies to SDR content.
pub const LOOK: &str = "GAMESCOPE_COLOR_LOOK_G22";
const PID: &str = "GAMESCOPE_PID";
const SERVER_ID: &str = "GAMESCOPE_XWAYLAND_SERVER_ID";
const FOCUSED_APP_GFX: &str = "GAMESCOPE_FOCUSED_APP_GFX";
const FOCUSED_WINDOW: &str = "GAMESCOPE_FOCUSED_WINDOW";
const FOCUSABLE_WINDOWS: &str = "GAMESCOPE_FOCUSABLE_WINDOWS";
const FOCUSABLE_APPS: &str = "GAMESCOPE_FOCUSABLE_APPS";
const COLOR_MANAGEMENT_DISABLE: &str = "GAMESCOPE_COLOR_MANAGEMENT_DISABLE";
const SDR_GAMUT_WIDENESS: &str = "GAMESCOPE_COLOR_SDR_GAMUT_WIDENESS";
const VIRTUAL_WHITE: &str = "GAMESCOPE_DISPLAY_VIRTUAL_WHITE";
const NIGHT_MODE: &str = "GAMESCOPE_COLOR_NIGHT_MODE";

/// The root properties whose change can move focus.
pub const FOCUS_PROPERTIES: [&str; 3] = [FOCUSED_APP_GFX, FOCUSED_WINDOW, FOCUSABLE_WINDOWS];

/// Where gamescope's Xwayland servers put their sockets.
const X11_SOCKET_DIR: &str = "/tmp/.X11-unix";

/// Property access on one X11 root window.
pub trait RootProps {
    /// A CARDINAL property, `None` when it is absent or of another type.
    fn cardinals(&self, name: &str) -> Result<Option<Vec<u32>>>;
    /// A text property, `None` when it is absent.
    fn string(&self, name: &str) -> Result<Option<String>>;
    /// Replace a property with a STRING value.
    fn set_string(&mut self, name: &str, value: &str) -> Result<()>;
    /// Remove a property.
    fn delete(&mut self, name: &str) -> Result<()>;
}

/// What gamescope is showing: the Steam AppID of the window being drawn and
/// that window's process id. `None` where gamescope reports nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Focus {
    /// `GAMESCOPE_FOCUSED_APP_GFX`: 769 is the Steam UI.
    pub app_id: Option<u32>,
    /// The focused window's process, for matching by executable.
    pub pid: Option<u32>,
}

/// Who owns the look property right now.
#[derive(Debug, Clone, PartialEq)]
pub enum Look {
    /// No look, or an empty path.
    None,
    /// A table Satur8 wrote, with the saturation its name records.
    Satur8(Saturation),
    /// A path something else set.
    Foreign(String),
}

/// gamescope's color settings as `satur8 doctor` reports them.
#[derive(Debug, Clone, PartialEq)]
pub struct ColorState {
    /// `GAMESCOPE_PID`.
    pub pid: Option<u32>,
    /// gamescope ignores every color setting, the look included, while set.
    pub color_management_disabled: bool,
    /// Who set `GAMESCOPE_COLOR_LOOK_G22`.
    pub look: Look,
    /// Steam's vibrancy slider.
    pub sdr_gamut_wideness: Option<f32>,
    /// Steam's white point, CIE x and y.
    pub virtual_white: Option<(f32, f32)>,
    /// Steam's night mode: amount, hue, saturation.
    pub night_mode: Option<(f32, f32, f32)>,
}

fn first(values: Option<Vec<u32>>) -> Option<u32> {
    values.and_then(|v| v.first().copied())
}

/// Whether this root belongs to the first Xwayland server of a Steam-mode
/// gamescope, the one Steam Game Mode reports focus on. gamescope writes
/// `GAMESCOPE_FOCUSED_APP_GFX` only with `--steam`, so a gamescope nested in a
/// desktop, which has no Steam focus to follow, does not count.
pub fn is_root_server(props: &dyn RootProps) -> Result<bool> {
    let has_pid = props.cardinals(PID)?.is_some_and(|v| !v.is_empty());
    let server_zero = first(props.cardinals(SERVER_ID)?) == Some(0);
    let steam_mode = props.cardinals(FOCUSED_APP_GFX)?.is_some();
    Ok(has_pid && server_zero && steam_mode)
}

/// The focused app, with its pid taken from the `[window, appid, pid]`
/// triplets in `GAMESCOPE_FOCUSABLE_WINDOWS` for `GAMESCOPE_FOCUSED_WINDOW`.
pub fn read_focus(props: &dyn RootProps) -> Result<Focus> {
    let app_id = first(props.cardinals(FOCUSED_APP_GFX)?);
    let pid = match first(props.cardinals(FOCUSED_WINDOW)?) {
        Some(window) => props.cardinals(FOCUSABLE_WINDOWS)?.and_then(|triplets| {
            triplets
                .chunks_exact(3)
                .find(|triplet| triplet[0] == window)
                .map(|triplet| triplet[2])
        }),
        None => None,
    };
    Ok(Focus {
        app_id,
        pid: pid.filter(|pid| *pid != 0),
    })
}

/// The AppIDs that have a window right now: `GAMESCOPE_FOCUSABLE_APPS`, plus
/// the AppID of every `[window, appid, pid]` triplet in
/// `GAMESCOPE_FOCUSABLE_WINDOWS`. Steam's own interface (769) and the calling
/// program's own shortcut are in the list too; the caller leaves them out.
/// Each ID once, in the order gamescope lists them.
pub fn running_apps(props: &dyn RootProps) -> Result<Vec<u32>> {
    let mut apps = props.cardinals(FOCUSABLE_APPS)?.unwrap_or_default();
    if let Some(triplets) = props.cardinals(FOCUSABLE_WINDOWS)? {
        apps.extend(triplets.as_chunks::<3>().0.iter().map(|triplet| triplet[1]));
    }
    let mut seen = Vec::new();
    apps.retain(|app| {
        *app != 0 && !seen.contains(app) && {
            seen.push(*app);
            true
        }
    });
    Ok(apps)
}

fn classify_look(value: Option<String>) -> Look {
    match value {
        None => Look::None,
        Some(value) if value.is_empty() => Look::None,
        Some(value) => match look::saturation_from_path(&value) {
            Some(saturation) => Look::Satur8(saturation),
            None => Look::Foreign(value),
        },
    }
}

/// The current look when something other than Satur8 set it.
pub fn foreign_look(props: &dyn RootProps) -> Result<Option<String>> {
    Ok(match classify_look(props.string(LOOK)?) {
        Look::Foreign(value) => Some(value),
        Look::None | Look::Satur8(_) => None,
    })
}

/// Point the look at `path`. Returns the value it replaced when that was
/// someone else's, so the caller can put it back later.
pub fn apply_look(props: &mut dyn RootProps, path: &str) -> Result<Option<String>> {
    let previous = classify_look(props.string(LOOK)?);
    props.set_string(LOOK, path)?;
    Ok(match previous {
        Look::Foreign(value) => Some(value),
        Look::None | Look::Satur8(_) => None,
    })
}

/// Take Satur8's table off: restore `foreign` if one was replaced, otherwise
/// remove the property. A value something else set since, or a property
/// something else removed, is left as it is.
pub fn clear_look(props: &mut dyn RootProps, foreign: Option<&str>) -> Result<()> {
    match classify_look(props.string(LOOK)?) {
        Look::None | Look::Foreign(_) => Ok(()),
        Look::Satur8(_) => match foreign {
            Some(value) => props.set_string(LOOK, value),
            None => props.delete(LOOK),
        },
    }
}

/// Read gamescope's color settings, Satur8's and Steam's.
pub fn read_color_state(props: &dyn RootProps) -> Result<ColorState> {
    let floats = |name: &str| -> Result<Option<Vec<f32>>> {
        Ok(props
            .cardinals(name)?
            .map(|values| values.into_iter().map(f32::from_bits).collect()))
    };
    Ok(ColorState {
        pid: first(props.cardinals(PID)?),
        color_management_disabled: first(props.cardinals(COLOR_MANAGEMENT_DISABLE)?)
            .is_some_and(|value| value != 0),
        look: classify_look(props.string(LOOK)?),
        sdr_gamut_wideness: floats(SDR_GAMUT_WIDENESS)?.and_then(|v| v.first().copied()),
        virtual_white: floats(VIRTUAL_WHITE)?
            .filter(|v| v.len() >= 2)
            .map(|v| (v[0], v[1])),
        night_mode: floats(NIGHT_MODE)?
            .filter(|v| v.len() == 3)
            .map(|v| (v[0], v[1], v[2])),
    })
}

/// Displays to try, in order: `$DISPLAY` first, then one per `X<n>` socket.
pub fn candidate_displays(env_display: Option<&str>, socket_names: &[String]) -> Vec<String> {
    let mut displays = Vec::new();
    if let Some(display) = env_display.map(str::trim).filter(|d| !d.is_empty()) {
        displays.push(display.to_string());
    }
    let mut numbers: Vec<u32> = socket_names
        .iter()
        .filter_map(|name| name.strip_prefix('X')?.parse().ok())
        .collect();
    numbers.sort_unstable();
    numbers.dedup();
    for number in numbers {
        let display = format!(":{number}");
        if !displays.contains(&display) {
            displays.push(display);
        }
    }
    displays
}

/// The X11 sockets present now, with their inode numbers so a server that
/// restarted under the same name still reads as a change.
pub fn x11_sockets() -> Vec<(String, u64)> {
    use std::os::unix::fs::MetadataExt;

    let Ok(entries) = std::fs::read_dir(X11_SOCKET_DIR) else {
        return Vec::new();
    };
    let mut sockets: Vec<(String, u64)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            let inode = entry.metadata().ok()?.ino();
            Some((name, inode))
        })
        .collect();
    sockets.sort();
    sockets
}

/// Find gamescope's first Xwayland server. Does not trust `DISPLAY` alone:
/// the session scripts unset it for systemd units, and over SSH it is unset.
pub fn find_session() -> Option<(String, X11Root)> {
    let env_display = std::env::var("DISPLAY").ok();
    let names: Vec<String> = x11_sockets().into_iter().map(|(name, _)| name).collect();
    candidate_displays(env_display.as_deref(), &names)
        .into_iter()
        .find_map(|display| {
            let root = X11Root::connect(&display).ok()?;
            is_root_server(&root).ok()?.then_some((display, root))
        })
}

/// A connection to one X server's root window.
pub struct X11Root {
    conn: RustConnection,
    root: Window,
    atoms: RefCell<HashMap<String, Atom>>,
}

impl X11Root {
    /// Connect to `display` (for example `:0`) and use its first screen's root.
    pub fn connect(display: &str) -> Result<X11Root> {
        let (conn, screen) = RustConnection::connect(Some(display))
            .with_context(|| format!("connecting to X display {display}"))?;
        let root = conn
            .setup()
            .roots
            .get(screen)
            .map(|screen| screen.root)
            .with_context(|| format!("X display {display} has no screen {screen}"))?;
        Ok(X11Root {
            conn,
            root,
            atoms: RefCell::new(HashMap::new()),
        })
    }

    /// The root window's size in pixels right now. In a gamescope session
    /// this is the size games are given, which is what a window has to fit.
    pub fn size(&self) -> Result<(u32, u32)> {
        let geometry = self
            .conn
            .get_geometry(self.root)?
            .reply()
            .context("reading the root window's size")?;
        Ok((u32::from(geometry.width), u32::from(geometry.height)))
    }

    fn atom(&self, name: &str) -> Result<Atom> {
        let cached = self.atoms.borrow().get(name).copied();
        if let Some(atom) = cached {
            return Ok(atom);
        }
        let atom = self
            .conn
            .intern_atom(false, name.as_bytes())?
            .reply()
            .with_context(|| format!("interning {name}"))?
            .atom;
        self.atoms.borrow_mut().insert(name.to_string(), atom);
        Ok(atom)
    }

    /// Ask the server for a PropertyNotify whenever a root property changes.
    pub fn watch_properties(&self) -> Result<()> {
        let attributes = ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE);
        self.conn
            .change_window_attributes(self.root, &attributes)?
            .check()
            .context("selecting root property changes")?;
        Ok(())
    }

    /// Discard queued events without waiting. An error means the connection
    /// is gone.
    pub fn drain_events(&self) -> Result<()> {
        while self.conn.poll_for_event()?.is_some() {}
        Ok(())
    }

    /// Block until one of `names` changes on the root. Events already queued
    /// are drained first so a burst of changes is handled once. An error means
    /// the connection is gone.
    pub fn wait_for_change(&self, names: &[&str]) -> Result<()> {
        let wanted = names
            .iter()
            .map(|name| self.atom(name))
            .collect::<Result<Vec<Atom>>>()?;
        let is_wanted = |event: &Event| match event {
            Event::PropertyNotify(notify) => {
                notify.window == self.root && wanted.contains(&notify.atom)
            }
            _ => false,
        };
        loop {
            let mut changed = is_wanted(&self.conn.wait_for_event()?);
            while let Some(event) = self.conn.poll_for_event()? {
                changed |= is_wanted(&event);
            }
            if changed {
                return Ok(());
            }
        }
    }
}

impl RootProps for X11Root {
    fn cardinals(&self, name: &str) -> Result<Option<Vec<u32>>> {
        let atom = self.atom(name)?;
        let reply = self
            .conn
            .get_property(false, self.root, atom, AtomEnum::CARDINAL, 0, u32::MAX)?
            .reply()
            .with_context(|| format!("reading {name}"))?;
        if reply.type_ != u32::from(AtomEnum::CARDINAL) {
            return Ok(None);
        }
        Ok(Some(
            reply
                .value32()
                .map(|values| values.collect())
                .unwrap_or_default(),
        ))
    }

    fn string(&self, name: &str) -> Result<Option<String>> {
        let atom = self.atom(name)?;
        let reply = self
            .conn
            .get_property(false, self.root, atom, AtomEnum::ANY, 0, u32::MAX)?
            .reply()
            .with_context(|| format!("reading {name}"))?;
        if reply.type_ == u32::from(AtomEnum::NONE) {
            return Ok(None);
        }
        let text = String::from_utf8_lossy(&reply.value);
        Ok(Some(text.trim_end_matches('\0').to_string()))
    }

    fn set_string(&mut self, name: &str, value: &str) -> Result<()> {
        let atom = self.atom(name)?;
        self.conn
            .change_property8(
                PropMode::REPLACE,
                self.root,
                atom,
                AtomEnum::STRING,
                value.as_bytes(),
            )?
            .check()
            .with_context(|| format!("setting {name}"))?;
        Ok(())
    }

    fn delete(&mut self, name: &str) -> Result<()> {
        let atom = self.atom(name)?;
        self.conn
            .delete_property(self.root, atom)?
            .check()
            .with_context(|| format!("deleting {name}"))?;
        Ok(())
    }
}

/// An in-memory root for tests.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct FakeRoot {
    pub cardinals: HashMap<String, Vec<u32>>,
    pub strings: HashMap<String, String>,
}

#[cfg(test)]
impl RootProps for FakeRoot {
    fn cardinals(&self, name: &str) -> Result<Option<Vec<u32>>> {
        Ok(self.cardinals.get(name).cloned())
    }

    fn string(&self, name: &str) -> Result<Option<String>> {
        Ok(self.strings.get(name).cloned())
    }

    fn set_string(&mut self, name: &str, value: &str) -> Result<()> {
        self.strings.insert(name.to_string(), value.to_string());
        Ok(())
    }

    fn delete(&mut self, name: &str) -> Result<()> {
        self.strings.remove(name);
        self.cardinals.remove(name);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OURS: &str = "/home/deck/.local/state/satur8/gamescope/look-g22-s2000.cube";
    const STEAMS: &str = "/home/deck/looks/steam.cube";

    impl FakeRoot {
        fn with(mut self, name: &str, values: &[u32]) -> FakeRoot {
            self.cardinals.insert(name.to_string(), values.to_vec());
            self
        }
    }

    fn root() -> FakeRoot {
        FakeRoot::default()
    }

    fn with_look(value: &str) -> FakeRoot {
        let mut root = FakeRoot::default();
        root.strings.insert(LOOK.into(), value.into());
        root
    }

    /// The Deck with Sushiamo and ELDEN RING running and Satur8's own Steam
    /// shortcut (3367991943) in front, 2026-11-22.
    #[test]
    fn running_apps_lists_each_app_with_a_window_once() {
        let root = root()
            .with(FOCUSABLE_APPS, &[3367991943, 4555040, 1245620, 769])
            .with(
                FOCUSABLE_WINDOWS,
                &[
                    100663299, 3367991943, 20114, 33554435, 4555040, 16797, 31457333, 769, 6788,
                ],
            );
        assert_eq!(
            running_apps(&root).unwrap(),
            vec![3367991943, 4555040, 1245620, 769]
        );
        // Only the triplets, as older gamescope publishes.
        let older =
            FakeRoot::default().with(FOCUSABLE_WINDOWS, &[33554435, 4555040, 16797, 1, 0, 2]);
        assert_eq!(running_apps(&older).unwrap(), vec![4555040]);
        assert_eq!(
            running_apps(&FakeRoot::default()).unwrap(),
            Vec::<u32>::new()
        );
    }

    /// Values read on the Deck in Sushiamo, 2026-11-21.
    #[test]
    fn focus_joins_the_focused_window_to_its_triplet() {
        let root = root()
            .with(FOCUSED_APP_GFX, &[4555040])
            .with(FOCUSED_WINDOW, &[33554435])
            .with(
                FOCUSABLE_WINDOWS,
                &[33554435, 4555040, 16797, 31457333, 769, 6788],
            );
        assert_eq!(
            read_focus(&root).unwrap(),
            Focus {
                app_id: Some(4555040),
                pid: Some(16797)
            }
        );
    }

    /// Values read on the Deck at the Steam home screen, 2026-11-21.
    #[test]
    fn focus_at_the_steam_home_screen_is_769() {
        let root = root()
            .with(FOCUSED_APP_GFX, &[769])
            .with(FOCUSED_WINDOW, &[31457333])
            .with(FOCUSABLE_WINDOWS, &[31457333, 769, 6788]);
        assert_eq!(
            read_focus(&root).unwrap(),
            Focus {
                app_id: Some(769),
                pid: Some(6788)
            }
        );
    }

    #[test]
    fn empty_focus_properties_mean_nothing_is_focused() {
        let root = root()
            .with(FOCUSED_APP_GFX, &[])
            .with(FOCUSED_WINDOW, &[])
            .with(FOCUSABLE_WINDOWS, &[31457333, 769, 6788]);
        assert_eq!(read_focus(&root).unwrap(), Focus::default());
        assert_eq!(read_focus(&FakeRoot::default()).unwrap(), Focus::default());
    }

    #[test]
    fn focused_window_missing_from_the_triplets_has_no_pid() {
        let root = root()
            .with(FOCUSED_APP_GFX, &[730])
            .with(FOCUSED_WINDOW, &[42])
            .with(FOCUSABLE_WINDOWS, &[31457333, 769, 6788]);
        assert_eq!(
            read_focus(&root).unwrap(),
            Focus {
                app_id: Some(730),
                pid: None
            }
        );
    }

    /// `:0` and `:1` as read on the Deck, plus a plain X server.
    #[test]
    fn only_server_zero_of_gamescope_is_the_root_server() {
        let display0 = root()
            .with(PID, &[6366])
            .with(SERVER_ID, &[0])
            .with(FOCUSED_APP_GFX, &[769]);
        let display1 = root().with(PID, &[6366]).with(SERVER_ID, &[1]);
        let plain = FakeRoot::default();
        let no_pid = root().with(SERVER_ID, &[0]);
        assert!(is_root_server(&display0).unwrap());
        assert!(!is_root_server(&display1).unwrap());
        assert!(!is_root_server(&plain).unwrap());
        assert!(!is_root_server(&no_pid).unwrap());
    }

    /// A gamescope nested in a desktop runs without `--steam`, so its server 0
    /// root has no `GAMESCOPE_FOCUSED_APP_GFX`.
    #[test]
    fn a_gamescope_without_steam_mode_is_not_the_root_server() {
        let nested = root().with(PID, &[4242]).with(SERVER_ID, &[0]);
        assert!(!is_root_server(&nested).unwrap());
    }

    /// gamescope writes the property with no value when nothing is focused;
    /// it is still present, so the session still counts.
    #[test]
    fn steam_mode_with_nothing_focused_is_still_the_root_server() {
        let idle = root()
            .with(PID, &[6366])
            .with(SERVER_ID, &[0])
            .with(FOCUSED_APP_GFX, &[]);
        assert!(is_root_server(&idle).unwrap());
    }

    #[test]
    fn apply_then_clear_leaves_no_property() {
        let mut root = FakeRoot::default();
        assert_eq!(apply_look(&mut root, OURS).unwrap(), None);
        assert_eq!(root.strings.get(LOOK).map(String::as_str), Some(OURS));
        clear_look(&mut root, None).unwrap();
        assert_eq!(root.strings.get(LOOK), None);
    }

    #[test]
    fn replacing_our_own_table_is_not_a_foreign_value() {
        let mut root = with_look(OURS);
        let next = "/home/deck/.local/state/satur8/gamescope/look-g22-s1500.cube";
        assert_eq!(apply_look(&mut root, next).unwrap(), None);
    }

    #[test]
    fn a_foreign_look_is_returned_and_restored() {
        let mut root = with_look(STEAMS);
        let foreign = apply_look(&mut root, OURS).unwrap();
        assert_eq!(foreign.as_deref(), Some(STEAMS));
        clear_look(&mut root, foreign.as_deref()).unwrap();
        assert_eq!(root.strings.get(LOOK).map(String::as_str), Some(STEAMS));
    }

    #[test]
    fn clearing_never_touches_a_look_set_by_something_else() {
        let mut root = with_look(STEAMS);
        clear_look(&mut root, None).unwrap();
        assert_eq!(root.strings.get(LOOK).map(String::as_str), Some(STEAMS));
    }

    #[test]
    fn clearing_removes_a_stale_table_left_by_a_crash() {
        let mut root = with_look(OURS);
        clear_look(&mut root, None).unwrap();
        assert_eq!(root.strings.get(LOOK), None);
    }

    #[test]
    fn color_state_reads_steams_settings_and_our_table() {
        // Raw values read on the Deck, 2026-11-21.
        let mut root = root()
            .with(PID, &[6366])
            .with(SDR_GAMUT_WIDENESS, &[1056964608])
            .with(VIRTUAL_WHITE, &[1050682510, 1051232957]);
        root.strings.insert(LOOK.into(), OURS.into());
        let state = read_color_state(&root).unwrap();
        assert_eq!(state.pid, Some(6366));
        assert!(!state.color_management_disabled);
        assert_eq!(state.look, Look::Satur8(Saturation::new(2.0)));
        assert_eq!(state.sdr_gamut_wideness, Some(0.5));
        let (x, y) = state.virtual_white.unwrap();
        assert!(
            (x - 0.3127).abs() < 0.001 && (y - 0.329).abs() < 0.001,
            "{x} {y}"
        );
        assert_eq!(state.night_mode, None);
    }

    #[test]
    fn color_state_reports_disabled_color_management_and_foreign_looks() {
        let mut root = root().with(COLOR_MANAGEMENT_DISABLE, &[1]);
        root.strings.insert(LOOK.into(), STEAMS.into());
        let state = read_color_state(&root).unwrap();
        assert!(state.color_management_disabled);
        assert_eq!(state.look, Look::Foreign(STEAMS.into()));
    }

    #[test]
    fn displays_try_the_environment_first_then_each_socket_in_order() {
        let sockets: Vec<String> = ["X1", "X0", "X10", "lock", "X1"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            candidate_displays(Some(":1"), &sockets),
            [":1", ":0", ":10"]
        );
        assert_eq!(candidate_displays(Some(""), &sockets), [":0", ":1", ":10"]);
        assert_eq!(candidate_displays(None, &[]), Vec::<String>::new());
    }
}
