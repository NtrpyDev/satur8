//! `satur8` - per-game digital vibrance for Linux.
//!
//! Detects the environment, picks a backend (KWin on KDE Wayland), and drives
//! saturation from the command line - directly (`set`/`on`/`off`), as a launch
//! wrapper (`run`), or via per-game profiles (`profile`).

mod config;
mod profile_cmd;
mod run;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use satur8_backend::{all_outputs, select_backend};
use satur8_core::{CostNote, Environment, Gpu, Saturation};
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
    },
    /// List the outputs the active backend can target.
    Outputs,
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
        Command::Status | Command::Doctor | Command::Outputs
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
        Command::On { saturation, linear } => cmd_set(saturation, linear, None),
        Command::Outputs => cmd_outputs(),
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
        Command::Profile { cmd } => profile_cmd::run(cmd),
        Command::Status => cmd_status(),
        Command::Doctor => cmd_doctor(),
        Command::Repair => cmd_repair(),
    }
}

fn cmd_set(saturation: f32, linear: bool, output: Option<String>) -> Result<()> {
    let clamped = Saturation::try_new(saturation)?;
    let mut backend = select_backend()?;

    // Pick the target output: a named one (validated against the backend) or all.
    let target = match &output {
        Some(id) => backend
            .outputs()
            .into_iter()
            .find(|o| &o.id == id)
            .with_context(|| {
                format!(
                    "no output '{id}' on the {} backend (see `satur8 outputs`)",
                    backend.name()
                )
            })?,
        None => all_outputs(),
    };

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
    println!("outputs on the {} backend:", backend.name());
    for o in backend.outputs() {
        println!("  {:<8} {}", o.id, o.human_name);
    }
    Ok(())
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
    if let Some((display, root)) = session::find_session() {
        println!("satur8 status");
        for line in gamescope_lines(&display, &root) {
            println!("{line}");
        }
        return Ok(());
    }

    let envr = Environment::detect();
    println!("environment:");
    println!("  session:  {}", envr.session);
    println!("  desktop:  {}", envr.desktop);
    println!("  gpu:      {}", envr.gpu);
    println!("  prefers:  {} backend", envr.preferred_backend());

    match KwinBackend::connect() {
        Some(kwin) => {
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
        }
        None => println!("backend kwin: not available in this session"),
    }
    Ok(())
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
    if let Some((display, root)) = session::find_session() {
        println!("satur8 doctor");
        for line in gamescope_lines(&display, &root) {
            println!("{line}");
        }
        return print_profiles_summary();
    }

    let envr = Environment::detect();
    println!("satur8 doctor");
    println!("  session type : {}", envr.session);
    println!("  desktop      : {}", envr.desktop);
    println!("  gpu          : {}", envr.gpu);
    println!("  preferred    : {}", envr.preferred_backend());
    println!();

    match KwinBackend::connect() {
        Some(kwin) => {
            println!("  [ok] KWin reachable over D-Bus");
            match kwin.is_supported() {
                Ok(true) => println!("  [ok] satur8 effect matches the running KWin ABI"),
                Ok(false) => {
                    let state = satur8_kwin::repair::inspect(false);
                    let built_for = |abi: &Option<String>| match abi {
                        Some(version) => format!("built for KWin {version}"),
                        None => "not installed".to_string(),
                    };
                    println!("  [!!] the running KWin rejects the satur8 effect.");
                    println!("       system effect  : {}", built_for(&state.system));
                    println!("       per-user effect: {}", built_for(&state.user));
                    println!(
                        "       installed KWin : {}",
                        state
                            .headers
                            .as_deref()
                            .unwrap_or("unknown (no KWin headers)")
                    );
                    println!("       Run `satur8 repair` to rebuild it for this KWin version.");
                    return Ok(());
                }
                Err(error) => {
                    println!("  [!!] couldn't query effect compatibility: {error}");
                    return Ok(());
                }
            }
            match kwin.is_loaded() {
                Ok(true) => println!("  [ok] satur8 effect is loaded"),
                Ok(false) => {
                    println!("  [..] satur8 effect installed but not loaded (run `satur8 on`)")
                }
                Err(e) => println!("  [!!] couldn't query effect state: {e}"),
            }
        }
        None => {
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
            match envr.session {
                satur8_core::SessionType::Tty => {
                    println!("    -> usable now: no display server owns DRM master")
                }
                other => println!(
                    "    -> on {other} the display server owns DRM master; CTM is reached \
                     through its backend (KWin here), not directly"
                ),
            }
        }
        Err(e) => println!("  DRM CTM: not available ({e})"),
    }

    println!();
    print_profiles_summary()
}

fn print_profiles_summary() -> Result<()> {
    let profiles = config::load_profiles().unwrap_or_default();
    println!("  profiles file: {}", config::profiles_path()?.display());
    println!("  profiles loaded: {}", profiles.profiles.len());
    Ok(())
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
