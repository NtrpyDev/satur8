//! The launch wrapper: `satur8 run [...] -- <game command>`.
//!
//! This is the primary, zero-polling trigger (PLAN.md section 6). We apply the
//! saturation, spawn the game, wait, and restore on exit - there is no watcher
//! process running during play. It is the Steam launch-option path:
//!
//!   satur8 run --profile cs2 -- %command%
//!
//! Restore is best-effort-guaranteed: whatever ends the game (normal exit,
//! Ctrl-C, or Steam's SIGTERM), we forward the signal to the game and always
//! reset the backend once it has exited.

use anyhow::{bail, Context, Result};
use satur8_core::{Profiles, Saturation};
use std::path::Path;
use std::process::Command;
use std::thread;

use crate::backend::{all_outputs, select_backend};
use crate::config;

pub struct RunArgs {
    pub profile: Option<String>,
    pub saturation: Option<f32>,
    /// Force a specific run strategy. Currently "gamescope" or "gamescope-native".
    pub via: Option<String>,
    /// Extra args passed to gamescope before `--` (e.g. -W 2560 -H 1440).
    pub gamescope_args: Vec<String>,
    pub command: Vec<String>,
}

pub fn run(args: RunArgs) -> Result<i32> {
    if args.command.is_empty() {
        bail!("nothing to run. Usage: satur8 run [--profile NAME | --saturation S] -- <command>");
    }

    let profiles = config::load_profiles().unwrap_or_default();
    let resolved = resolve_saturation(&profiles, &args)?;

    // SteamOS/Bazzite path: drive the running gamescope compositor at runtime.
    if args.via.as_deref() == Some("gamescope-native") {
        let sat = resolved.unwrap_or(Saturation::IDENTITY);
        let session = satur8_gamescope::native_session()
            .context("connecting to the running gamescope compositor")?;
        session
            .apply(sat)
            .context("applying native gamescope LUT override")?;
        eprintln!(
            "satur8: {:.2} via gamescope-native (running compositor), launching {}",
            sat.get(),
            args.command[0]
        );
        return launch_with_restore(&args.command, move || {
            if let Err(e) = session.restore() {
                eprintln!("satur8: warning, failed to restore gamescope color state: {e}");
            }
        });
    }

    // The gamescope fallback wraps the whole launch in a nested compositor and
    // exits with the game, so it bypasses the apply/restore backend entirely.
    if args.via.as_deref() == Some("gamescope") {
        let sat = resolved.unwrap_or(Saturation::IDENTITY);
        eprintln!(
            "satur8: {:.2} via gamescope (nested compositor: extra pass + latency), launching {}",
            sat.get(),
            args.command[0]
        );
        return satur8_gamescope::run(sat, &args.gamescope_args, &args.command)
            .context("running via gamescope");
    }
    if let Some(other) = &args.via {
        bail!("unknown --via '{other}' (supported: gamescope, gamescope-native)");
    }

    // Apply before launch (if we have something to apply).
    let mut backend = select_backend()?;
    if let Some(sat) = resolved {
        backend
            .apply(&all_outputs(), sat)
            .context("applying saturation before launch")?;
        eprintln!(
            "satur8: {:.2} via {} backend, launching {}",
            sat.get(),
            backend.name(),
            args.command[0]
        );
    } else {
        eprintln!(
            "satur8: no matching profile and no --saturation; launching {} without changes",
            args.command[0]
        );
    }

    let restore = |b: &mut Box<dyn satur8_core::Backend>| {
        let restore_to = profiles.default_saturation();
        let result = if restore_to.is_identity() {
            b.reset(&all_outputs())
        } else {
            b.apply(&all_outputs(), restore_to)
        };
        if let Err(e) = result {
            eprintln!("satur8: warning, failed to restore saturation: {e}");
        }
    };

    // Launch the game as a child. It shares our process group, so an interactive
    // Ctrl-C reaches it directly; we additionally forward SIGTERM/SIGINT that
    // arrive at the wrapper (e.g. Steam stopping the game) on to the child.
    let mut child = match Command::new(&args.command[0])
        .args(&args.command[1..])
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            restore(&mut backend);
            return Err(e).with_context(|| format!("launching {}", args.command[0]));
        }
    };
    let pid = child.id() as libc::pid_t;

    let mut signals = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
    ])
    .context("installing signal handlers")?;
    let handle = signals.handle();
    let forwarder = thread::spawn(move || {
        for sig in signals.forever() {
            // Forward to the game; when it exits, child.wait() below returns and
            // we restore. SAFETY: kill() with a known pid and signal number.
            unsafe {
                libc::kill(pid, sig);
            }
        }
    });

    let status = child.wait().context("waiting for game to exit");

    // Stop the forwarder thread and always restore, whatever happened.
    handle.close();
    let _ = forwarder.join();
    restore(&mut backend);

    let code = status?.code().unwrap_or(1);
    Ok(code)
}

fn launch_with_restore<F>(command: &[String], restore: F) -> Result<i32>
where
    F: FnOnce(),
{
    let mut restore = Some(restore);
    let mut child = match Command::new(&command[0]).args(&command[1..]).spawn() {
        Ok(c) => c,
        Err(e) => {
            if let Some(restore) = restore.take() {
                restore();
            }
            return Err(e).with_context(|| format!("launching {}", command[0]));
        }
    };
    let pid = child.id() as libc::pid_t;

    let mut signals = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
    ])
    .context("installing signal handlers")?;
    let handle = signals.handle();
    let forwarder = thread::spawn(move || {
        for sig in signals.forever() {
            unsafe {
                libc::kill(pid, sig);
            }
        }
    });

    let status = child.wait().context("waiting for game to exit");

    handle.close();
    let _ = forwarder.join();
    if let Some(restore) = restore.take() {
        restore();
    }

    Ok(status?.code().unwrap_or(1))
}

/// Decide the saturation to apply, in priority order:
/// explicit `--saturation` > named `--profile` > match the command's exe.
fn resolve_saturation(profiles: &Profiles, args: &RunArgs) -> Result<Option<Saturation>> {
    if let Some(s) = args.saturation {
        return Ok(Some(Saturation::new(s)));
    }
    if let Some(name) = &args.profile {
        let p = profiles
            .by_name(name)
            .with_context(|| format!("no profile named '{name}' (see `satur8 profile list`)"))?;
        return Ok(Some(p.saturation()));
    }
    // Try to auto-match by the launched executable's basename.
    let exe = exe_basename(&args.command[0]);
    if let Some(p) = profiles.match_exe(&exe) {
        eprintln!("satur8: matched profile '{}' by exe '{exe}'", p.name);
        return Ok(Some(p.saturation()));
    }
    Ok(None)
}

fn exe_basename(cmd: &str) -> String {
    Path::new(cmd)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| cmd.to_string())
}
