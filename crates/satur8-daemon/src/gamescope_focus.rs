//! Focus changes in Steam Game Mode.
//!
//! gamescope publishes the focused app on its first Xwayland root, and Steam
//! Game Mode has no KWin script to forward it, so this thread watches those
//! root properties and feeds each change to the same `react` the KWin script
//! reaches over D-Bus. It sleeps in the X event wait while connected.
//!
//! It only drives the daemon while the selected backend is gamescope, so a
//! nested gamescope window on a desktop never fights the desktop's own focus
//! script, and it stops for good once a desktop backend is selected, since a
//! selected backend is never replaced. A lost X connection is logged and
//! re-probed; it never ends the daemon.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::Result;
use satur8_gamescope::session::{self, Focus, X11Root, FOCUS_PROPERTIES};
use zbus::blocking::object_server::InterfaceRef;
use zbus::blocking::Connection;

use crate::{Daemon, GamescopeSync, PATH};

/// How often to probe while a gamescope session is expected soon.
const PROBE_INTERVAL: Duration = Duration::from_secs(2);
/// How long to keep probing after startup, a lost connection, or a change in
/// the X11 sockets. gamescope creates its socket before it publishes the root
/// properties the probe looks for, so one probe is not enough.
const PROBE_WINDOW: Duration = Duration::from_secs(60);
/// How often to look for new X11 sockets once probing has given up.
const SOCKET_RECHECK: Duration = Duration::from_secs(30);
/// How often to retry the first sync while backend detection is pending.
const SYNC_RETRY: Duration = Duration::from_millis(250);

/// The window class and pid `react` takes for a gamescope focus. The class is
/// `steam_app_<id>`, which profiles already match by Steam AppID.
pub fn activation(focus: Focus) -> (String, u32) {
    let class = focus
        .app_id
        .map(|app_id| format!("steam_app_{app_id}"))
        .unwrap_or_default();
    (class, focus.pid.unwrap_or(0))
}

/// Start the focus thread. It runs until the process exits and stops acting
/// once `running` is false.
pub fn spawn(connection: Connection, running: Arc<AtomicBool>) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("gamescope-focus".into())
        .spawn(move || run(&connection, &running))
}

fn run(connection: &Connection, running: &AtomicBool) {
    let daemon = match connection.object_server().interface::<_, Daemon>(PATH) {
        Ok(daemon) => daemon,
        Err(error) => {
            eprintln!("satur8-daemon: gamescope focus watcher not started: {error}");
            return;
        }
    };
    let mut sockets = session::x11_sockets();
    let mut probe_until = Instant::now() + PROBE_WINDOW;
    while running.load(Ordering::Acquire) {
        if !daemon.get_mut().may_use_gamescope() {
            return;
        }
        if Instant::now() < probe_until {
            if let Some((display, root)) = session::find_session() {
                eprintln!("satur8-daemon: watching gamescope focus on {display}");
                if let Err(error) = watch(&root, &daemon, running) {
                    eprintln!("satur8-daemon: lost gamescope on {display}: {error:#}");
                }
                drop(root);
                forget_current(&daemon, running);
                sockets = session::x11_sockets();
                probe_until = Instant::now() + PROBE_WINDOW;
            }
            thread::sleep(PROBE_INTERVAL);
        } else {
            thread::sleep(SOCKET_RECHECK);
            let now = session::x11_sockets();
            if now != sockets {
                sockets = now;
                probe_until = Instant::now() + PROBE_WINDOW;
            }
        }
    }
}

/// Follow focus on `root` until the connection fails. The first focus read on
/// a fresh connection resyncs, which re-applies the focused profile after a
/// reconnect and clears a table a crashed daemon left behind. When backend
/// detection has not finished yet, the sync is retried every `SYNC_RETRY`
/// instead of waiting for the next focus change.
fn watch(root: &X11Root, daemon: &InterfaceRef<Daemon>, running: &AtomicBool) -> Result<()> {
    root.watch_properties()?;
    let mut focus = session::read_focus(root)?;
    loop {
        let (class, pid) = activation(focus);
        let mut daemon = daemon.get_mut();
        // Checked under the interface lock, which shutdown holds while it
        // restores, so nothing is applied after the restore.
        if !running.load(Ordering::Acquire) {
            return Ok(());
        }
        match daemon.sync_gamescope(&class, pid) {
            GamescopeSync::Done => break,
            GamescopeSync::NotGamescope => return Ok(()),
            GamescopeSync::NotYet => {}
        }
        drop(daemon);
        thread::sleep(SYNC_RETRY);
        root.drain_events()?;
        focus = session::read_focus(root)?;
    }

    let mut last = focus;
    loop {
        root.wait_for_change(&FOCUS_PROPERTIES)?;
        let focus = session::read_focus(root)?;
        if focus == last {
            continue;
        }
        last = focus;
        let (class, pid) = activation(focus);
        let mut daemon = daemon.get_mut();
        if !running.load(Ordering::Acquire) {
            return Ok(());
        }
        if daemon.uses_gamescope() {
            daemon.react(&class, pid);
        }
    }
}

/// After a lost connection the applied profile can no longer be trusted, so
/// the next focus applies again.
fn forget_current(daemon: &InterfaceRef<Daemon>, running: &AtomicBool) {
    let mut daemon = daemon.get_mut();
    if running.load(Ordering::Acquire) && daemon.uses_gamescope() {
        daemon.invalidate_current();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_game_activates_as_its_steam_app_class_with_its_pid() {
        let focus = Focus {
            app_id: Some(4555040),
            pid: Some(16797),
        };
        assert_eq!(activation(focus), ("steam_app_4555040".to_string(), 16797));
    }

    #[test]
    fn nothing_focused_activates_as_no_class_and_no_pid() {
        assert_eq!(activation(Focus::default()), (String::new(), 0));
    }
}
