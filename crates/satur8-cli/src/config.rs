//! Where the profiles file lives and how we read/write it.

use anyhow::{Context, Result};
use std::path::PathBuf;
use vibrance_core::Profiles;

/// `$XDG_CONFIG_HOME/vibrance/` (or `~/.config/vibrance/`).
pub fn config_dir() -> Result<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Ok(PathBuf::from(xdg).join("vibrance"));
        }
    }
    let home = std::env::var("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".config").join("vibrance"))
}

pub fn profiles_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("profiles.toml"))
}

/// Load profiles, returning defaults if the file doesn't exist yet.
pub fn load_profiles() -> Result<Profiles> {
    let path = profiles_path()?;
    match std::fs::read_to_string(&path) {
        Ok(s) => Profiles::from_toml(&s)
            .with_context(|| format!("parsing {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Profiles::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Write profiles back to disk, creating the config dir if needed.
pub fn save_profiles(profiles: &Profiles) -> Result<()> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating {}", dir.display()))?;
    let path = profiles_path()?;
    let toml = profiles.to_toml().context("serializing profiles")?;
    std::fs::write(&path, toml).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}
