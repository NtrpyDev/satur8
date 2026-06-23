//! `vibrance` - per-game digital vibrance for Linux.
//!
//! M1 scope: detect the environment, pick a backend (KWin on KDE Wayland), and
//! drive saturation from the command line. The launch wrapper (`run`) and
//! profile management grow in later milestones.

mod config;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use vibrance_core::{Backend, CostNote, Environment, Output, Saturation};
use vibrance_kwin::KwinBackend;

#[derive(Parser)]
#[command(name = "vibrance", version, about = "Per-game digital vibrance for Linux")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Set the saturation now (loads the backend if needed). 1.0 = unchanged,
    /// >1 = more vivid, 0 = greyscale. Range 0.0..=4.0.
    Set { saturation: f32 },
    /// Turn vibrance on using a saturation value (default 1.5).
    On {
        #[arg(default_value_t = 1.5)]
        saturation: f32,
    },
    /// Turn vibrance off and release any per-frame cost.
    Off,
    /// Show the current environment, chosen backend, and state.
    Status,
    /// Diagnose the environment and backend availability.
    Doctor,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Set { saturation } => cmd_set(saturation),
        Command::On { saturation } => cmd_set(saturation),
        Command::Off => cmd_off(),
        Command::Status => cmd_status(),
        Command::Doctor => cmd_doctor(),
    }
}

/// The single output we act on for now (per-output targeting is M7).
fn all_outputs() -> Output {
    Output {
        id: "all".into(),
        human_name: "All outputs".into(),
    }
}

/// Resolve the backend for this environment. Only KWin exists in M1; other
/// environments get a clear, honest error pointing at the roadmap.
fn select_backend() -> Result<Box<dyn Backend>> {
    if let Some(kwin) = KwinBackend::detect() {
        return Ok(Box::new(kwin));
    }
    let envr = Environment::detect();
    bail!(
        "no usable backend for this session ({}, {}, {}).\n\
         The preferred backend here is '{}', which isn't implemented yet \
         (KWin/KDE Wayland is the M1 target). See PLAN.md for the roadmap.",
        envr.session,
        envr.desktop,
        envr.gpu,
        envr.preferred_backend()
    )
}

fn cmd_set(saturation: f32) -> Result<()> {
    let clamped = Saturation::new(saturation);
    let mut backend = select_backend()?;
    backend
        .apply(&all_outputs(), clamped)
        .with_context(|| "applying saturation")?;
    println!(
        "vibrance: saturation {:.2} via {} backend{}",
        clamped.get(),
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

fn cmd_off() -> Result<()> {
    let mut backend = select_backend()?;
    backend.reset(&all_outputs()).with_context(|| "turning vibrance off")?;
    println!("vibrance: off ({} backend released)", backend.name());
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
            print!("backend kwin: available, effect {}", if loaded { "loaded" } else { "not loaded" });
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
    println!("vibrance doctor");
    println!("  session type : {}", envr.session);
    println!("  desktop      : {}", envr.desktop);
    println!("  gpu          : {}", envr.gpu);
    println!("  preferred    : {}", envr.preferred_backend());
    println!();

    match KwinBackend::detect() {
        Some(kwin) => {
            println!("  [ok] KWin reachable over D-Bus");
            match kwin.is_loaded() {
                Ok(true) => println!("  [ok] vibrance effect is loaded"),
                Ok(false) => println!(
                    "  [..] vibrance effect installed but not loaded (run `vibrance on`)"
                ),
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
