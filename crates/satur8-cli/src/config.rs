//! Where the profiles file lives and how we read/write it.

use anyhow::{Context, Result};
use satur8_core::Profiles;
use std::path::PathBuf;

/// `$XDG_CONFIG_HOME/satur8/` (or `~/.config/satur8/`).
pub fn config_dir() -> Result<PathBuf> {
    let xdg = std::env::var("XDG_CONFIG_HOME").ok();
    let home = std::env::var("HOME").ok();
    config_dir_from_env(xdg.as_deref(), home.as_deref())
}

fn config_dir_from_env(xdg_config_home: Option<&str>, home: Option<&str>) -> Result<PathBuf> {
    if let Some(xdg) = xdg_config_home {
        if !xdg.is_empty() {
            return Ok(PathBuf::from(xdg).join("satur8"));
        }
    }
    let home = home.context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".config").join("satur8"))
}

pub fn profiles_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("profiles.toml"))
}

#[cfg(test)]
fn profiles_path_from_env(xdg_config_home: Option<&str>, home: Option<&str>) -> Result<PathBuf> {
    Ok(config_dir_from_env(xdg_config_home, home)?.join("profiles.toml"))
}

/// Load profiles, returning defaults if the file doesn't exist yet.
pub fn load_profiles() -> Result<Profiles> {
    let path = profiles_path()?;
    match std::fs::read_to_string(&path) {
        Ok(s) => Profiles::from_toml(&s).with_context(|| format!("parsing {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Profiles::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Write profiles back to disk, creating the config dir if needed.
pub fn save_profiles(profiles: &Profiles) -> Result<()> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = profiles_path()?;
    let toml = profiles.to_toml().context("serializing profiles")?;
    std::fs::write(&path, toml).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use satur8_core::{MatchRule, Profile};

    #[test]
    fn config_dir_prefers_xdg_config_home() {
        assert_eq!(
            config_dir_from_env(Some("/tmp/xdg"), Some("/home/noah")).unwrap(),
            PathBuf::from("/tmp/xdg/satur8")
        );
    }

    #[test]
    fn config_dir_falls_back_to_home_when_xdg_missing_or_empty() {
        assert_eq!(
            config_dir_from_env(None, Some("/home/noah")).unwrap(),
            PathBuf::from("/home/noah/.config/satur8")
        );
        assert_eq!(
            config_dir_from_env(Some(""), Some("/home/noah")).unwrap(),
            PathBuf::from("/home/noah/.config/satur8")
        );
    }

    #[test]
    fn config_dir_errors_without_home_fallback() {
        assert!(config_dir_from_env(None, None).is_err());
        assert!(config_dir_from_env(Some(""), None).is_err());
    }

    #[test]
    fn profiles_path_adds_profiles_toml() {
        assert_eq!(
            profiles_path_from_env(Some("/tmp/xdg"), Some("/home/noah")).unwrap(),
            PathBuf::from("/tmp/xdg/satur8/profiles.toml")
        );
    }

    #[test]
    fn profiles_toml_round_trips() {
        let profiles = Profiles {
            default_saturation: 1.1,
            profiles: vec![Profile {
                name: "Portal".into(),
                saturation: 1.6,
                match_rule: MatchRule {
                    exe: Some("portal2_linux".into()),
                    window_class: Some("portal2".into()),
                    steam_app_id: Some(620),
                },
                outputs: vec!["DP-1".into()],
            }],
        };

        let toml = profiles.to_toml().unwrap();
        let parsed = Profiles::from_toml(&toml).unwrap();
        assert_eq!(parsed, profiles);
    }
}
