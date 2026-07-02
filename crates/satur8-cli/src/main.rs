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
use satur8_core::{CostNote, Environment, Saturation};
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
        #[arg(default_value_t = 1.5)]
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
        #[arg(long)]
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();
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
    }
}

fn cmd_set(saturation: f32, linear: bool, output: Option<String>) -> Result<()> {
    let clamped = Saturation::new(saturation);
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
    let envr = Environment::detect();
    println!("environment:");
    println!("  session:  {}", envr.session);
    println!("  desktop:  {}", envr.desktop);
    println!("  gpu:      {}", envr.gpu);
    println!("  prefers:  {} backend", envr.preferred_backend());

    match KwinBackend::detect() {
        Some(kwin) => {
            let loaded = kwin.is_loaded().unwrap_or(false);
            print!(
                "backend kwin: available, effect {}",
                if loaded { "loaded" } else { "not loaded" }
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

fn cmd_doctor() -> Result<()> {
    let envr = Environment::detect();
    println!("satur8 doctor");
    println!("  session type : {}", envr.session);
    println!("  desktop      : {}", envr.desktop);
    println!("  gpu          : {}", envr.gpu);
    println!("  preferred    : {}", envr.preferred_backend());
    println!();

    match KwinBackend::detect() {
        Some(kwin) => {
            println!("  [ok] KWin reachable over D-Bus");
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

    let profiles = config::load_profiles().unwrap_or_default();
    println!();
    println!("  profiles file: {}", config::profiles_path()?.display());
    println!("  profiles loaded: {}", profiles.profiles.len());
    Ok(())
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
}
