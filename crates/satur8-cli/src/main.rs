//! `satur8` - per-game digital vibrance for Linux.
//!
//! Detects the environment, picks a backend (KWin on KDE Wayland), and drives
//! saturation from the command line - directly (`set`/`on`/`off`), as a launch
//! wrapper (`run`), or via per-game profiles (`profile`).

mod config;
mod games_cmd;
mod gnome_report;
mod outputs_cmd;
mod outputs_report;
mod profile_cmd;
mod run;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use satur8_backend::{all_outputs, select_backend};
use satur8_core::{CostNote, Desktop, Environment, Gpu, Saturation, SessionType};
use satur8_gamescope::session::{self, ColorState, Focus, Look, X11Root};
use satur8_kwin::KwinBackend;

#[derive(Parser)]
#[command(
    name = "satur8",
    version,
    about = "Per-game digital vibrance for Linux"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Set the saturation now (loads the backend if needed). 1.0 = unchanged,
    /// >1 = more vivid, 0 = greyscale. Range 0.0..=4.0.
    Set {
        #[arg(value_parser = parse_saturation)]
        saturation: f32,
        /// Blend in linear light (more correct) instead of gamma sRGB.
        #[arg(long)]
        linear: bool,
        /// Target a specific output id (see `satur8 outputs`); default all.
        #[arg(long)]
        output: Option<String>,
    },
    /// Turn satur8 on using a saturation value (default 1.5).
    On {
        #[arg(default_value_t = 1.5, value_parser = parse_saturation)]
        saturation: f32,
        /// Blend in linear light (more correct) instead of gamma sRGB.
        #[arg(long)]
        linear: bool,
        /// Target a specific output id (see `satur8 outputs`); default all.
        #[arg(long)]
        output: Option<String>,
    },
    /// List the outputs the active backend can target, or change the
    /// per-output settings.
    Outputs {
        #[command(subcommand)]
        cmd: Option<outputs_cmd::OutputsCmd>,
    },
    /// Turn satur8 off and release any per-frame cost.
    Off,
    /// Apply satur8, launch a game, and restore on exit. The Steam launch
    /// option: `satur8 run --profile cs2 -- %command%`.
    Run {
        /// Use a named profile's saturation.
        #[arg(long)]
        profile: Option<String>,
        /// Override saturation directly (wins over --profile).
        #[arg(long, value_parser = parse_saturation)]
        saturation: Option<f32>,
        /// Force a run strategy instead of the native backend. Supported:
        /// `gamescope` (nested fallback) and `gamescope-native` (running compositor).
        #[arg(long)]
        via: Option<String>,
        /// Extra args for gamescope before `--`, comma-separated
        /// (e.g. --gamescope-args=-W,2560,-H,1440,-r,240). Only with --via gamescope.
        #[arg(long, value_delimiter = ',')]
        gamescope_args: Vec<String>,
        /// The game command, after `--`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
    /// List the installed Steam games and the profile each one uses.
    Games {
        /// Print a JSON array instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Manage per-game profiles.
    Profile {
        #[command(subcommand)]
        cmd: profile_cmd::ProfileCmd,
    },
    /// Show the current environment, chosen backend, and state.
    Status,
    /// Diagnose the environment and backend availability.
    Doctor,
    /// Rebuild the KWin effect if the running KWin rejects it (this happens
    /// after every KWin update). Runs automatically when the daemon starts.
    Repair,
}

fn parse_saturation(value: &str) -> Result<f32, String> {
    let value = value
        .parse::<f32>()
        .map_err(|error| format!("invalid saturation '{value}': {error}"))?;
    Saturation::try_new(value).map_err(|error| error.to_string())?;
    Ok(value)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    // The read-only report commands should end quietly when their output is
    // piped into something that stops reading (`satur8 doctor | head`), not
    // panic on the broken pipe. Commands that change the display keep Rust's
    // default so they always reach their restore path.
    if matches!(
        cli.command,
        Command::Status | Command::Doctor | Command::Outputs { cmd: None } | Command::Games { .. }
    ) {
        // SAFETY: called once at startup, before any other thread exists.
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        }
    }
    match cli.command {
        Command::Set {
            saturation,
            linear,
            output,
        } => cmd_set(saturation, linear, output),
        Command::On {
            saturation,
            linear,
            output,
        } => cmd_set(saturation, linear, output),
        Command::Outputs { cmd: None } => cmd_outputs(),
        Command::Outputs { cmd: Some(cmd) } => outputs_cmd::run(cmd),
        Command::Off => cmd_off(),
        Command::Run {
            profile,
            saturation,
            via,
            gamescope_args,
            command,
        } => {
            let code = run::run(run::RunArgs {
                profile,
                saturation,
                via,
                gamescope_args,
                command,
            })?;
            std::process::exit(code);
        }
        Command::Games { json } => games_cmd::run(json),
        Command::Profile { cmd } => profile_cmd::run(cmd),
        Command::Status => cmd_status(),
        Command::Doctor => cmd_doctor(),
        Command::Repair => cmd_repair(),
    }
}

fn cmd_set(saturation: f32, linear: bool, output: Option<String>) -> Result<()> {
    let clamped = Saturation::try_new(saturation)?;
    let mut backend = select_backend()?;

    let target = set_target(backend.name(), &backend.outputs(), output.as_deref())?;

    let linear_active = apply_with_linear_flag(backend.as_mut(), &target, clamped, linear)?;
    if linear && !linear_active {
        eprintln!(
            "note: the {} backend works in its native color space and ignores --linear",
            backend.name()
        );
    }

    println!(
        "satur8: saturation {:.2}{} on {} via {} backend{}",
        clamped.get(),
        if linear_active { " (linear light)" } else { "" },
        target.human_name,
        backend.name(),
        cost_suffix(backend.cost())
    );
    if (clamped.get() - saturation).abs() > f32::EPSILON {
        eprintln!(
            "note: requested {saturation:.2} was clamped to {:.2} (valid range 0.0..=4.0)",
            clamped.get()
        );
    }
    Ok(())
}

/// The output `satur8 set` targets: every output with no `--output` or with
/// `--output all`, else the named one, which must be in the backend's list. A
/// backend that shows one saturation everywhere lists no single outputs, so a
/// name there is refused rather than changing every screen.
fn set_target(
    backend: &str,
    outputs: &[satur8_core::Output],
    output: Option<&str>,
) -> Result<satur8_core::Output> {
    match output {
        None => Ok(all_outputs()),
        Some(id) if id == satur8_core::levels::ALL_OUTPUTS => Ok(all_outputs()),
        Some(id) => outputs
            .iter()
            .find(|o| o.id == id)
            .cloned()
            .with_context(|| {
                format!("no output '{id}' on the {backend} backend (see `satur8 outputs`)")
            }),
    }
}

fn apply_with_linear_flag(
    backend: &mut dyn satur8_core::Backend,
    target: &satur8_core::Output,
    saturation: Saturation,
    linear: bool,
) -> Result<bool> {
    let linear_active = linear && backend.supports_linear_light();
    if linear_active {
        backend
            .set_linear_light(true)
            .with_context(|| "enabling linear-light blending")?;
    } else if !linear {
        let _ = backend.set_linear_light(false);
    }
    backend
        .apply(target, saturation)
        .with_context(|| "applying saturation")?;
    Ok(linear_active)
}

fn cmd_outputs() -> Result<()> {
    let backend = select_backend()?;
    let profiles = config::load_profiles().unwrap_or_default();
    let outputs = backend.outputs();
    let current = backend.current_levels();
    let reason = backend.per_output_reason();
    let view = outputs_report::BackendView {
        name: backend.name(),
        per_output: backend.per_output(),
        outputs: &outputs,
        current: current.as_ref(),
        reason: reason.as_deref(),
    };
    for line in outputs_report::outputs_lines(&view, &profiles) {
        println!("{line}");
    }
    Ok(())
}

/// The per-output part of `satur8 doctor` and `satur8 status`, after a blank
/// line. Prints nothing, and selects no backend, unless the file uses the
/// per-output settings.
fn print_per_output_settings() {
    let profiles = config::load_profiles().unwrap_or_default();
    if !outputs_report::uses_per_output_settings(&profiles) {
        return;
    }
    let lines = match select_backend() {
        Ok(backend) => {
            let outputs = backend.outputs();
            let current = backend.current_levels();
            let reason = backend.per_output_reason();
            let view = outputs_report::BackendView {
                name: backend.name(),
                per_output: backend.per_output(),
                outputs: &outputs,
                current: current.as_ref(),
                reason: reason.as_deref(),
            };
            outputs_report::settings_section(Ok(&view), &profiles)
        }
        Err(error) => {
            let reason = error.to_string();
            let first_line = reason.lines().next().unwrap_or("").trim_end_matches('.');
            outputs_report::settings_section(Err(first_line), &profiles)
        }
    };
    println!();
    for line in lines {
        println!("{line}");
    }
}

fn cmd_off() -> Result<()> {
    let mut backend = select_backend()?;
    backend
        .reset(&all_outputs())
        .with_context(|| "turning satur8 off")?;
    println!("satur8: off ({} backend released)", backend.name());
    Ok(())
}

fn cmd_status() -> Result<()> {
    let envr = Environment::detect();
    let report = choose_report(
        satur8_backend::desktop_backend_present,
        session::find_session,
        KwinBackend::connect,
        envr.session,
        envr.desktop,
    );
    if let Report::GameMode((display, root)) = &report {
        println!("satur8 status");
        for line in gamescope_lines(display, root) {
            println!("{line}");
        }
        print_active_output();
        print_per_output_settings();
        return Ok(());
    }

    println!("environment:");
    println!("  session:  {}", envr.session);
    println!("  desktop:  {}", envr.desktop);
    println!("  gpu:      {}", envr.gpu);
    println!("  prefers:  {} backend", envr.preferred_backend());

    match report {
        Report::Kwin(kwin) => {
            let supported = kwin.is_supported().unwrap_or(false);
            let loaded = kwin.is_loaded().unwrap_or(false);
            print!(
                "backend kwin: {}, effect {}",
                if supported {
                    "available"
                } else {
                    "unavailable"
                },
                if loaded {
                    "loaded"
                } else if supported {
                    "not loaded"
                } else {
                    "missing or ABI-incompatible"
                }
            );
            if loaded {
                if let Ok(sat) = kwin.current_saturation() {
                    print!(", saturation {:.2}", sat.get());
                }
            }
            println!();
            // Outputs showing another value than the one above, whatever set
            // them (a manual `set --output`, a profile, an output's default).
            if loaded {
                if let Some(levels) = satur8_core::Backend::current_levels(&kwin) {
                    for line in outputs_report::current_output_lines(&levels) {
                        println!("{line}");
                    }
                }
            }
        }
        Report::Gnome => println!("{}", gnome_report::status_line(&gnome_report::gather())),
        Report::GameMode(_) | Report::NoDesktopBackend => {
            println!("backend kwin: not available in this session")
        }
    }
    print_active_output();
    print_per_output_settings();
    Ok(())
}

/// The `satur8 status` line for a profile limited to one output, from the
/// daemon's `ActiveProfile` and `ActiveOutput` properties. None when no profile
/// is applied or it changes every output.
fn active_output_line(profile: &str, output: &str) -> Option<String> {
    let output = output.trim();
    (!profile.is_empty() && !output.is_empty())
        .then(|| format!("active profile: {profile}, on output {output} only"))
}

/// Print [`active_output_line`] when the daemon is running and reports one.
fn print_active_output() {
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
    let profile: String = proxy.get_property("ActiveProfile").unwrap_or_default();
    let output: String = proxy.get_property("ActiveOutput").unwrap_or_default();
    if let Some(line) = active_output_line(&profile, &output) {
        println!("{line}");
    }
}

fn cmd_repair() -> Result<()> {
    match satur8_kwin::repair::repair() {
        Ok(outcome) => {
            println!("satur8: {}", outcome.describe());
            Ok(())
        }
        Err(message) => anyhow::bail!(message),
    }
}

fn cmd_doctor() -> Result<()> {
    let envr = Environment::detect();
    let report = choose_report(
        satur8_backend::desktop_backend_present,
        session::find_session,
        KwinBackend::connect,
        envr.session,
        envr.desktop,
    );
    if let Report::GameMode((display, root)) = &report {
        println!("satur8 doctor");
        for line in gamescope_lines(display, root) {
            println!("{line}");
        }
        print_per_output_settings();
        return print_profiles_summary();
    }

    println!("satur8 doctor");
    println!("  session type : {}", envr.session);
    println!("  desktop      : {}", envr.desktop);
    println!("  gpu          : {}", envr.gpu);
    println!("  preferred    : {}", envr.preferred_backend());
    println!();

    match report {
        Report::Kwin(kwin) => {
            println!("  [ok] KWin reachable over D-Bus");
            match kwin.is_supported() {
                Ok(true) => {
                    let state = satur8_kwin::repair::EffectState {
                        running: kwin.running_version(),
                        ..satur8_kwin::repair::inspect(true)
                    };
                    let step = satur8_kwin::repair::decide(&state);
                    let advice = satur8_kwin::repair::advice_for(&state);
                    for line in supported_effect_lines(&state, step, &advice) {
                        println!("{line}");
                    }
                }
                Ok(false) => {
                    let state = satur8_kwin::repair::inspect(false);
                    let built_for = |abi: &Option<String>| match abi {
                        Some(version) => format!("built for KWin {version}"),
                        None => "not installed".to_string(),
                    };
                    println!("  [!!] the running KWin rejects the satur8 effect.");
                    println!("       system effect  : {}", built_for(&state.system));
                    println!("       per-user effect: {}", built_for(&state.user));
                    println!("       effect origin  : {}", effect_origin(&state));
                    println!(
                        "       installed KWin : {}",
                        state
                            .headers
                            .as_deref()
                            .unwrap_or("unknown (no KWin headers)")
                    );
                    println!("       Next step: {}.", satur8_kwin::repair::advice_here());
                    return Ok(());
                }
                Err(error) => {
                    println!("  [!!] couldn't query effect compatibility: {error}");
                    return Ok(());
                }
            }
            match kwin.is_loaded() {
                Ok(true) => {
                    let profiles = config::load_profiles().unwrap_or_default();
                    let state = outputs_report::uses_per_output_settings(&profiles)
                        .then(|| kwin.effect_outputs());
                    println!("{}", loaded_effect_line(state));
                }
                Ok(false) => {
                    println!("  [..] satur8 effect installed but not loaded (run `satur8 on`)")
                }
                Err(e) => println!("  [!!] couldn't query effect state: {e}"),
            }
        }
        Report::Gnome => {
            for line in gnome_report::doctor_lines(&gnome_report::gather()) {
                println!("{line}");
            }
        }
        Report::GameMode(_) | Report::NoDesktopBackend => {
            println!("  [!!] KWin backend unavailable.");
            println!(
                "       Expected a KDE Plasma Wayland session. Detected {} / {}.",
                envr.session, envr.desktop
            );
        }
    }

    // Zero-cost DRM CTM availability (read-only probe; never touches master).
    println!();
    match satur8_drm_ctm::probe_ctm() {
        Ok(lines) => {
            println!("  DRM CTM (zero-cost path):");
            for l in lines {
                println!("    {l}");
            }
            println!("    -> {}", ctm_route_note(envr.session, envr.desktop));
        }
        Err(e) => println!("  DRM CTM: not available ({e})"),
    }

    print_per_output_settings();
    println!();
    print_profiles_summary()
}

/// How the DRM CTM is reached in this session, for `satur8 doctor`: only a
/// bare console owns DRM master; otherwise the display server does.
fn ctm_route_note(session: SessionType, desktop: Desktop) -> String {
    match (session, desktop) {
        (SessionType::Tty, _) => "usable now: no display server owns DRM master".to_string(),
        (SessionType::Wayland, Desktop::Kde) => format!(
            "on {session} the display server owns DRM master; CTM is reached through its \
             backend (KWin here), not directly"
        ),
        (SessionType::Wayland, Desktop::Gnome) => format!(
            "on {session} GNOME Shell owns DRM master; Satur8 does not reach CTM there and \
             uses the GNOME Shell extension's shader instead"
        ),
        (other, _) => format!(
            "on {other} the display server owns DRM master; CTM is only reachable through \
             it, not directly"
        ),
    }
}

/// Where the effect KWin loads came from, for `satur8 doctor`. KWin searches
/// the per-user plugin dir ahead of the system one, so a per-user copy is the
/// one named when both exist.
fn effect_origin(state: &satur8_kwin::repair::EffectState) -> String {
    match (&state.user, &state.user_from_set, &state.system) {
        (Some(version), Some(key), _) => format!(
            "per-user copy from the set that came with Satur8 ({key}), built for KWin {version}"
        ),
        (Some(version), None, _) => format!(
            "per-user copy built for KWin {version}, not from the set that came with Satur8 \
             (a local build or an earlier install)"
        ),
        (None, _, Some(version)) => format!(
            "system-wide copy (a package or `install.sh --system`), built for KWin {version}"
        ),
        (None, _, None) => "no effect file found".to_string(),
    }
}

/// Doctor's lines when KWin says it supports the effect. KWin keeps answering
/// yes for an effect it loaded earlier, so a per-user file on disk for another
/// KWin is reported the way `satur8 repair` treats it (`step`, `advice`).
fn supported_effect_lines(
    state: &satur8_kwin::repair::EffectState,
    step: satur8_kwin::repair::Step,
    advice: &str,
) -> Vec<String> {
    use satur8_kwin::repair::Step;
    let Some(on_disk) = satur8_kwin::repair::stale_user_file(state) else {
        return vec![
            "  [ok] satur8 effect matches the running KWin ABI".to_string(),
            format!("       effect origin: {}", effect_origin(state)),
        ];
    };
    let running = state.running.as_deref().unwrap_or("unknown");
    let fix = match step {
        Step::InstallFromSet | Step::Build => "; `satur8 repair` will replace it",
        Step::DropUserCopy => "; `satur8 repair` will remove it",
        _ => "",
    };
    vec![
        format!(
            "  [!!] the per-user effect file on disk is for KWin {on_disk}, not the running \
             KWin {running}{fix}."
        ),
        "       KWin still uses the copy it loaded earlier; the file on disk fails at the next \
         session start."
            .to_string(),
        format!("       effect origin: {}", effect_origin(state)),
        format!("       Next step: {advice}."),
    ]
}

/// The `satur8 doctor` line for a loaded KWin effect. `state` is only asked
/// for when the profiles file uses the per-output settings, so the line stays
/// as before for every other file. A loaded effect from before per-output
/// support gets "[..]" instead of "[ok]"; the per-output section below says
/// why and what to do.
fn loaded_effect_line(state: Option<satur8_kwin::EffectOutputs>) -> &'static str {
    match state {
        Some(satur8_kwin::EffectOutputs::OlderLoaded { .. }) => {
            "  [..] satur8 effect is loaded, but it is one from before per-output \
             support (see per-output settings below)"
        }
        _ => "  [ok] satur8 effect is loaded",
    }
}

fn print_profiles_summary() -> Result<()> {
    let profiles = config::load_profiles().unwrap_or_default();
    println!("  profiles file: {}", config::profiles_path()?.display());
    println!("  profiles loaded: {}", profiles.profiles.len());
    Ok(())
}

/// The gamescope session to report on, in backend selection order: only when
/// no desktop backend would be chosen ahead of gamescope, so a desktop with a
/// gamescope nested in it still gets the desktop report.
fn game_mode_session<T>(
    desktop_present: impl FnOnce() -> bool,
    find_session: impl FnOnce() -> Option<T>,
) -> Option<T> {
    if desktop_present() {
        None
    } else {
        find_session()
    }
}

/// Which report `satur8 doctor` and `satur8 status` print.
#[derive(Debug, PartialEq)]
enum Report<S, K> {
    /// Steam Game Mode, with the gamescope session found.
    GameMode(S),
    /// KDE Plasma, with the KWin connection.
    Kwin(K),
    /// GNOME on Wayland without KWin: the GNOME Shell extension block.
    Gnome,
    /// Anything else keeps the "KWin backend unavailable" lines.
    NoDesktopBackend,
}

/// Pick the report in backend selection order: Game Mode only when no desktop
/// backend comes first, then KWin, then the GNOME block for a GNOME Wayland
/// session. Each probe runs only when the earlier ones did not decide.
fn choose_report<S, K>(
    desktop_present: impl FnOnce() -> bool,
    find_session: impl FnOnce() -> Option<S>,
    connect_kwin: impl FnOnce() -> Option<K>,
    session: SessionType,
    desktop: Desktop,
) -> Report<S, K> {
    if let Some(found) = game_mode_session(desktop_present, find_session) {
        return Report::GameMode(found);
    }
    if let Some(kwin) = connect_kwin() {
        return Report::Kwin(kwin);
    }
    if session == SessionType::Wayland && desktop == Desktop::Gnome {
        Report::Gnome
    } else {
        Report::NoDesktopBackend
    }
}

/// The Game Mode report for the gamescope session on `display`.
fn gamescope_lines(display: &str, root: &X11Root) -> Vec<String> {
    let gpu = Environment::detect().gpu;
    let read = || -> Result<(ColorState, Focus)> {
        Ok((session::read_color_state(root)?, session::read_focus(root)?))
    };
    match read() {
        Ok((state, focus)) => gamescope_report(display, gpu, &state, &focus, daemon_running()),
        Err(error) => vec![
            format!("  session type : Steam Game Mode (gamescope on {display})"),
            format!("  [!!] couldn't read gamescope's state: {error:#}"),
        ],
    }
}

/// Whether the focus daemon owns its bus name; `None` if the bus is unreachable.
fn daemon_running() -> Option<bool> {
    let conn = zbus::blocking::Connection::session().ok()?;
    let proxy = zbus::blocking::Proxy::new(
        &conn,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )
    .ok()?;
    proxy.call("NameHasOwner", &("org.satur8.Daemon",)).ok()
}

/// The `satur8 doctor` / `satur8 status` lines for Steam Game Mode.
fn gamescope_report(
    display: &str,
    gpu: Gpu,
    state: &ColorState,
    focus: &Focus,
    daemon_running: Option<bool>,
) -> Vec<String> {
    let pid = state
        .pid
        .map_or_else(|| "unknown".to_string(), |pid| pid.to_string());
    let reachable = if state.color_management_disabled {
        format!(
            "  [!!] gamescope reachable (pid {pid}), color management off: \
             Satur8 tables have no effect until it is on"
        )
    } else {
        format!("  [ok] gamescope reachable (pid {pid}), color management on")
    };
    let table = match &state.look {
        Look::None => "  [ok] Satur8 table: not loaded".to_string(),
        Look::Satur8(saturation) => {
            format!(
                "  [ok] Satur8 table: loaded, saturation {:.2}",
                saturation.get()
            )
        }
        Look::Foreign(path) => format!("  [ok] Satur8 table: set by something else: {path}"),
    };
    let focused = match focus.app_id {
        Some(app_id) => format!("  [ok] focused app: {app_id}"),
        None => "  [ok] focused app: none".to_string(),
    };
    let service = match daemon_running {
        Some(true) => "  [ok] background service: running".to_string(),
        Some(false) => {
            "  [..] background service: off (systemctl --user enable --now satur8-daemon)"
                .to_string()
        }
        None => "  [..] background service: unknown (session bus unreachable)".to_string(),
    };
    let vibrancy = state
        .sdr_gamut_wideness
        .map_or_else(|| "default".to_string(), |value| format!("{value:.2}"));
    let white_point = state
        .virtual_white
        .map_or_else(|| "default".to_string(), |(x, y)| format!("{x:.4} {y:.4}"));
    let night_mode = match state.night_mode {
        Some((amount, _, _)) if amount > 0.0 => format!("on (amount {amount:.2})"),
        _ => "off".to_string(),
    };
    vec![
        format!("  session type : Steam Game Mode (gamescope on {display})"),
        format!("  gpu          : {gpu}"),
        "  preferred    : gamescope".to_string(),
        reachable,
        table,
        focused,
        service,
        format!(
            "  Steam settings (left alone): vibrancy {vibrancy}, white point {white_point}, \
             night mode {night_mode}"
        ),
    ]
}

fn cost_suffix(cost: CostNote) -> &'static str {
    match cost {
        CostNote::ZeroCost => " (zero per-frame cost)",
        CostNote::CompositorShaderPass => " (one compositor GPU pass)",
        CostNote::NestedCompositor => " (nested compositor: extra pass + latency)",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use satur8_core::{Backend, BackendError, Output};

    #[derive(Default)]
    struct FakeBackend {
        supports_linear: bool,
        linear_light: bool,
        linear_at_apply: Option<bool>,
        set_calls: Vec<bool>,
    }

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

        fn apply(&mut self, _output: &Output, _saturation: Saturation) -> Result<(), BackendError> {
            self.linear_at_apply = Some(self.linear_light);
            Ok(())
        }

        fn reset(&mut self, _output: &Output) -> Result<(), BackendError> {
            Ok(())
        }

        fn set_linear_light(&mut self, enabled: bool) -> Result<(), BackendError> {
            self.linear_light = enabled;
            self.set_calls.push(enabled);
            Ok(())
        }

        fn supports_linear_light(&self) -> bool {
            self.supports_linear
        }
    }

    #[test]
    fn games_command_takes_json_flag() {
        assert!(matches!(
            Cli::try_parse_from(["satur8", "games", "--json"]).map(|cli| cli.command),
            Ok(Command::Games { json: true })
        ));
        assert!(matches!(
            Cli::try_parse_from(["satur8", "games"]).map(|cli| cli.command),
            Ok(Command::Games { json: false })
        ));
    }

    #[test]
    fn profile_add_takes_steam_game_query() {
        let cli = Cli::try_parse_from([
            "satur8",
            "profile",
            "add",
            "sushiamo",
            "1.75",
            "--steam-game",
            "sushi",
        ]);
        assert!(matches!(
            cli.map(|cli| cli.command),
            Ok(Command::Profile {
                cmd: profile_cmd::ProfileCmd::Add {
                    steam_game: Some(ref query),
                    steam_app_id: None,
                    ..
                }
            }) if query == "sushi"
        ));
    }

    #[test]
    fn doctor_marks_an_older_loaded_effect_only_when_the_file_asks_for_per_output() {
        use satur8_kwin::EffectOutputs;
        assert_eq!(loaded_effect_line(None), "  [ok] satur8 effect is loaded");
        assert_eq!(
            loaded_effect_line(Some(EffectOutputs::PerOutput)),
            "  [ok] satur8 effect is loaded"
        );
        for installed_is_newer in [true, false] {
            assert_eq!(
                loaded_effect_line(Some(EffectOutputs::OlderLoaded { installed_is_newer })),
                "  [..] satur8 effect is loaded, but it is one from before per-output support \
                 (see per-output settings below)"
            );
        }
    }

    #[test]
    fn status_names_the_output_only_for_a_profile_limited_to_one() {
        assert_eq!(
            active_output_line("couch", "HDMI-A-1").as_deref(),
            Some("active profile: couch, on output HDMI-A-1 only")
        );
        assert_eq!(active_output_line("couch", ""), None);
        assert_eq!(active_output_line("", "HDMI-A-1"), None);
        assert_eq!(active_output_line("", ""), None);
    }

    #[test]
    fn on_takes_an_output_like_set() {
        assert!(matches!(
            Cli::try_parse_from(["satur8", "on", "1.3", "--output", "DP-2"]).map(|cli| cli.command),
            Ok(Command::On {
                saturation,
                output: Some(ref name),
                ..
            }) if saturation == 1.3 && name == "DP-2"
        ));
        assert!(matches!(
            Cli::try_parse_from(["satur8", "on"]).map(|cli| cli.command),
            Ok(Command::On {
                saturation,
                output: None,
                ..
            }) if saturation == 1.5
        ));
        // `off` takes no output: every output goes back to normal.
        assert!(Cli::try_parse_from(["satur8", "off", "--output", "DP-2"]).is_err());
    }

    #[test]
    fn set_targets_every_output_unless_one_is_named() {
        let listed = [satur8_core::Output {
            id: "DP-1".into(),
            human_name: "Dell U2720Q".into(),
        }];
        assert_eq!(set_target("kwin", &listed, None).unwrap().id, "all");
        assert_eq!(set_target("kwin", &listed, Some("all")).unwrap().id, "all");
        assert_eq!(
            set_target("kwin", &listed, Some("DP-1")).unwrap().id,
            "DP-1"
        );
    }

    /// The message v0.4.1 already printed for a name the backend does not
    /// list: a per-output backend's unknown name, and any name on a backend
    /// that shows one saturation everywhere.
    #[test]
    fn set_refuses_a_name_the_backend_does_not_list() {
        let kwin = [satur8_core::Output {
            id: "DP-1".into(),
            human_name: "Dell U2720Q".into(),
        }];
        assert_eq!(
            set_target("kwin", &kwin, Some("DP-9"))
                .unwrap_err()
                .to_string(),
            "no output 'DP-9' on the kwin backend (see `satur8 outputs`)"
        );
        assert_eq!(
            set_target("gnome-shell", &[all_outputs()], Some("DP-1"))
                .unwrap_err()
                .to_string(),
            "no output 'DP-1' on the gnome-shell backend (see `satur8 outputs`)"
        );
        let drm = [satur8_core::Output {
            id: "42".into(),
            human_name: "CRTC 42".into(),
        }];
        assert_eq!(
            set_target("drm-ctm", &drm, Some("999"))
                .unwrap_err()
                .to_string(),
            "no output '999' on the drm-ctm backend (see `satur8 outputs`)"
        );
    }

    #[test]
    fn outputs_takes_default_and_follow() {
        assert!(matches!(
            Cli::try_parse_from(["satur8", "outputs"]).map(|cli| cli.command),
            Ok(Command::Outputs { cmd: None })
        ));
        assert!(matches!(
            Cli::try_parse_from(["satur8", "outputs", "default", "DP-2", "0.85"])
                .map(|cli| cli.command),
            Ok(Command::Outputs {
                cmd: Some(outputs_cmd::OutputsCmd::Default {
                    ref name,
                    saturation: Some(value),
                    unset: false,
                })
            }) if name == "DP-2" && value == 0.85
        ));
        assert!(matches!(
            Cli::try_parse_from(["satur8", "outputs", "default", "DP-2", "--unset"])
                .map(|cli| cli.command),
            Ok(Command::Outputs {
                cmd: Some(outputs_cmd::OutputsCmd::Default {
                    saturation: None,
                    unset: true,
                    ..
                })
            })
        ));
        for (word, mode) in [
            ("all", outputs_cmd::FollowMode::All),
            ("off", outputs_cmd::FollowMode::All),
            ("focused", outputs_cmd::FollowMode::Focused),
            ("on", outputs_cmd::FollowMode::Focused),
        ] {
            assert!(
                matches!(
                    Cli::try_parse_from(["satur8", "outputs", "follow", word])
                        .map(|cli| cli.command),
                    Ok(Command::Outputs {
                        cmd: Some(outputs_cmd::OutputsCmd::Follow { mode: parsed })
                    }) if parsed == mode
                ),
                "{word}"
            );
        }
    }

    #[test]
    fn outputs_default_needs_a_value_or_unset_but_not_both() {
        for args in [
            &["satur8", "outputs", "default", "DP-2"][..],
            &["satur8", "outputs", "default", "DP-2", "0.85", "--unset"][..],
        ] {
            assert!(Cli::try_parse_from(args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn profile_add_takes_repeated_outputs() {
        let cli = Cli::try_parse_from([
            "satur8", "profile", "add", "couch", "1.3", "--exe", "hades", "--output", "HDMI-A-1",
            "--output", "DP-2",
        ]);
        assert!(matches!(
            cli.map(|cli| cli.command),
            Ok(Command::Profile {
                cmd: profile_cmd::ProfileCmd::Add { ref outputs, .. }
            }) if outputs == &["HDMI-A-1", "DP-2"]
        ));
    }

    #[test]
    fn steam_game_conflicts_with_steam_app_id() {
        let error = Cli::try_parse_from([
            "satur8",
            "profile",
            "add",
            "sushiamo",
            "1.75",
            "--steam-game",
            "sushi",
            "--steam-app-id",
            "4555040",
        ])
        .err()
        .map(|error| error.kind());
        assert_eq!(error, Some(clap::error::ErrorKind::ArgumentConflict));
    }

    #[test]
    fn linear_flag_is_set_before_apply_when_supported() {
        let mut backend = FakeBackend {
            supports_linear: true,
            ..FakeBackend::default()
        };

        let active =
            apply_with_linear_flag(&mut backend, &all_outputs(), Saturation::new(1.5), true)
                .unwrap();

        assert!(active);
        assert_eq!(backend.set_calls, vec![true]);
        assert_eq!(backend.linear_at_apply, Some(true));
    }

    #[test]
    fn linear_flag_is_ignored_without_backend_support() {
        let mut backend = FakeBackend::default();

        let active =
            apply_with_linear_flag(&mut backend, &all_outputs(), Saturation::new(1.5), true)
                .unwrap();

        assert!(!active);
        assert!(backend.set_calls.is_empty());
        assert_eq!(backend.linear_at_apply, Some(false));
    }

    #[test]
    fn non_linear_apply_disables_linear_mode_before_apply() {
        let mut backend = FakeBackend {
            supports_linear: true,
            linear_light: true,
            ..FakeBackend::default()
        };

        let active =
            apply_with_linear_flag(&mut backend, &all_outputs(), Saturation::new(1.5), false)
                .unwrap();

        assert!(!active);
        assert_eq!(backend.set_calls, vec![false]);
        assert_eq!(backend.linear_at_apply, Some(false));
    }

    #[test]
    fn a_desktop_backend_gets_the_desktop_report_even_with_gamescope_running() {
        let session = game_mode_session(
            || true,
            || -> Option<&str> {
                panic!("gamescope must not be probed when a desktop backend is present")
            },
        );
        assert_eq!(session, None);
    }

    #[test]
    fn doctor_names_where_the_effect_came_from() {
        use satur8_kwin::repair::EffectState;
        let v = |s: &str| Some(s.to_string());
        let from_set = EffectState {
            user: v("6.7.3"),
            user_from_set: v("steamos-6.7.3"),
            ..EffectState::default()
        };
        assert_eq!(
            effect_origin(&from_set),
            "per-user copy from the set that came with Satur8 (steamos-6.7.3), built for KWin 6.7.3"
        );
        let built = EffectState {
            user: v("6.8.3"),
            system: v("6.8.2"),
            ..EffectState::default()
        };
        assert!(effect_origin(&built)
            .starts_with("per-user copy built for KWin 6.8.3, not from the set"));
        let package = EffectState {
            system: v("6.8.3"),
            ..EffectState::default()
        };
        assert_eq!(
            effect_origin(&package),
            "system-wide copy (a package or `install.sh --system`), built for KWin 6.8.3"
        );
        assert_eq!(
            effect_origin(&EffectState::default()),
            "no effect file found"
        );
    }

    #[test]
    fn doctor_reports_a_wrong_file_on_disk_even_when_kwin_says_supported() {
        use satur8_kwin::repair::{EffectState, Step};
        let v = |s: &str| Some(s.to_string());
        let wrong = EffectState {
            headers: v("6.7.3"),
            running: v("6.7.3"),
            user: v("6.7.9"),
            supported: true,
            ..EffectState::default()
        };
        let advice =
            "run `satur8 repair` to install the matching effect from the set that came with Satur8";
        let lines = supported_effect_lines(&wrong, Step::InstallFromSet, advice);
        assert_eq!(
            lines[0],
            "  [!!] the per-user effect file on disk is for KWin 6.7.9, not the running KWin \
             6.7.3; `satur8 repair` will replace it."
        );
        assert!(lines
            .iter()
            .all(|line| !line.contains("matches the running KWin")));
        assert_eq!(lines[3], format!("       Next step: {advice}."));

        let right = EffectState {
            user: v("6.7.3"),
            ..wrong
        };
        let lines = supported_effect_lines(&right, Step::Healthy, "run `satur8 repair`");
        assert_eq!(
            lines[0],
            "  [ok] satur8 effect matches the running KWin ABI"
        );
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn ctm_note_names_the_compositor_that_owns_the_display() {
        assert_eq!(
            ctm_route_note(SessionType::Wayland, Desktop::Kde),
            "on Wayland the display server owns DRM master; CTM is reached through its \
             backend (KWin here), not directly"
        );
        let gnome = ctm_route_note(SessionType::Wayland, Desktop::Gnome);
        assert!(gnome.contains("GNOME Shell owns DRM master"));
        assert!(!gnome.contains("KWin"));
        for (session, desktop) in [
            (SessionType::Wayland, Desktop::Hyprland),
            (SessionType::X11, Desktop::Gnome),
            (SessionType::X11, Desktop::Kde),
            (SessionType::Unknown, Desktop::Other),
        ] {
            let note = ctm_route_note(session, desktop);
            assert!(!note.contains("KWin") && !note.contains("GNOME"), "{note}");
        }
        assert_eq!(
            ctm_route_note(SessionType::Tty, Desktop::Other),
            "usable now: no display server owns DRM master"
        );
    }

    fn no_kwin() -> Option<()> {
        None
    }

    #[test]
    fn kde_keeps_the_kwin_report() {
        let report = choose_report(
            || true,
            || -> Option<&str> { panic!("gamescope must not be probed on a KDE desktop") },
            || Some(()),
            SessionType::Wayland,
            Desktop::Kde,
        );
        assert_eq!(report, Report::Kwin(()));
        // KWin reachable wins over a GNOME desktop name, as before.
        let report = choose_report(
            || true,
            || None::<&str>,
            || Some(()),
            SessionType::Wayland,
            Desktop::Gnome,
        );
        assert_eq!(report, Report::Kwin(()));
        // KDE whose KWin does not answer keeps the old "unavailable" lines.
        let report = choose_report(
            || false,
            || None::<&str>,
            no_kwin,
            SessionType::Wayland,
            Desktop::Kde,
        );
        assert_eq!(report, Report::NoDesktopBackend);
    }

    #[test]
    fn game_mode_keeps_its_report() {
        let report = choose_report(
            || false,
            || Some(":0"),
            || -> Option<()> { panic!("KWin must not be probed in Game Mode") },
            SessionType::X11,
            Desktop::Other,
        );
        assert_eq!(report, Report::GameMode(":0"));
        // A GNOME desktop whose extension is not running finds no desktop
        // backend, so a gamescope session there still gets the Game Mode report.
        let report = choose_report(
            || false,
            || Some(":1"),
            no_kwin,
            SessionType::Wayland,
            Desktop::Gnome,
        );
        assert_eq!(report, Report::GameMode(":1"));
    }

    #[test]
    fn gnome_wayland_gets_the_gnome_report() {
        // Extension running: the GNOME probe counts as a desktop backend.
        let report = choose_report(
            || true,
            || -> Option<&str> { panic!("gamescope must not be probed") },
            no_kwin,
            SessionType::Wayland,
            Desktop::Gnome,
        );
        assert_eq!(report, Report::Gnome);
        // Extension missing or off, no gamescope: still the GNOME report.
        let report = choose_report(
            || false,
            || None::<&str>,
            no_kwin,
            SessionType::Wayland,
            Desktop::Gnome,
        );
        assert_eq!(report, Report::Gnome);
    }

    #[test]
    fn other_sessions_keep_the_old_lines() {
        for (session, desktop) in [
            (SessionType::X11, Desktop::Gnome),
            (SessionType::Wayland, Desktop::Hyprland),
            (SessionType::X11, Desktop::Other),
            (SessionType::Tty, Desktop::Other),
        ] {
            let report = choose_report(|| false, || None::<&str>, no_kwin, session, desktop);
            assert_eq!(report, Report::NoDesktopBackend, "{session:?} {desktop:?}");
        }
    }

    /// Over SSH on the Deck no desktop backend is reachable, so the probe runs.
    #[test]
    fn without_a_desktop_backend_the_gamescope_session_is_reported() {
        assert_eq!(game_mode_session(|| false, || Some(":0")), Some(":0"));
        assert_eq!(game_mode_session(|| false, || None::<&str>), None);
    }

    /// Steam's values as read on the Deck, 2026-11-21.
    fn deck_state(look: Look) -> ColorState {
        ColorState {
            pid: Some(6366),
            color_management_disabled: false,
            look,
            sdr_gamut_wideness: Some(f32::from_bits(1056964608)),
            virtual_white: Some((f32::from_bits(1050682510), f32::from_bits(1051232957))),
            night_mode: None,
        }
    }

    fn home() -> Focus {
        Focus {
            app_id: Some(769),
            pid: Some(6788),
        }
    }

    #[test]
    fn game_mode_report_matches_the_design() {
        let lines = gamescope_report(":0", Gpu::Amd, &deck_state(Look::None), &home(), Some(true));
        assert_eq!(
            lines,
            [
                "  session type : Steam Game Mode (gamescope on :0)",
                "  gpu          : AMD",
                "  preferred    : gamescope",
                "  [ok] gamescope reachable (pid 6366), color management on",
                "  [ok] Satur8 table: not loaded",
                "  [ok] focused app: 769",
                "  [ok] background service: running",
                "  Steam settings (left alone): vibrancy 0.50, white point 0.3128 0.3292, \
                 night mode off",
            ]
        );
    }

    #[test]
    fn game_mode_report_names_the_loaded_table() {
        let ours = deck_state(Look::Satur8(Saturation::new(1.75)));
        let lines = gamescope_report(":0", Gpu::Amd, &ours, &home(), Some(true));
        assert_eq!(lines[4], "  [ok] Satur8 table: loaded, saturation 1.75");

        let foreign = deck_state(Look::Foreign("/home/deck/vivid.cube".into()));
        let lines = gamescope_report(":0", Gpu::Amd, &foreign, &home(), Some(true));
        assert_eq!(
            lines[4],
            "  [ok] Satur8 table: set by something else: /home/deck/vivid.cube"
        );
    }

    #[test]
    fn game_mode_report_says_how_to_start_the_service() {
        let lines = gamescope_report(
            ":0",
            Gpu::Amd,
            &deck_state(Look::None),
            &Focus::default(),
            Some(false),
        );
        assert_eq!(lines[5], "  [ok] focused app: none");
        assert_eq!(
            lines[6],
            "  [..] background service: off (systemctl --user enable --now satur8-daemon)"
        );
        let lines = gamescope_report(":0", Gpu::Amd, &deck_state(Look::None), &home(), None);
        assert_eq!(
            lines[6],
            "  [..] background service: unknown (session bus unreachable)"
        );
    }

    #[test]
    fn game_mode_report_flags_disabled_color_management_and_night_mode() {
        let mut state = deck_state(Look::None);
        state.color_management_disabled = true;
        state.night_mode = Some((0.4, 0.1, 0.9));
        state.sdr_gamut_wideness = None;
        state.virtual_white = None;
        let lines = gamescope_report(":0", Gpu::Amd, &state, &home(), Some(true));
        assert!(lines[3].starts_with("  [!!] gamescope reachable (pid 6366), color management off"));
        assert_eq!(
            lines[7],
            "  Steam settings (left alone): vibrancy default, white point default, \
             night mode on (amount 0.40)"
        );
    }
}
