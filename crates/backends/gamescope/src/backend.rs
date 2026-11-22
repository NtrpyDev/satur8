//! Steam Game Mode backend: saturation through the running gamescope's look
//! table, which Steam's own vibrancy, white point and night mode run on top of.
//!
//! It holds no X connection between calls. gamescope can restart or the
//! session can switch modes at any time, and a backend, once selected, is
//! never re-detected, so each apply or reset opens a short connection and
//! re-probes when the remembered display is no longer gamescope's.
//!
//! A look something else had set before Satur8 took over is saved in a small
//! file beside the tables, not in memory, because `satur8 on` and a later
//! `satur8 off` are separate processes.

use std::fs;
use std::path::{Path, PathBuf};

use satur8_core::{Backend, BackendError, CostNote, Output, Saturation};

use crate::look;
use crate::session::{self, RootProps, X11Root, LOOK};

/// The saved foreign look, in the table folder.
const FOREIGN_FILE: &str = "foreign-look";

/// The gamescope Game Mode backend.
pub struct GamescopeBackend {
    display: String,
    dir: PathBuf,
}

impl GamescopeBackend {
    /// A backend for the running gamescope session, if there is one.
    pub fn detect() -> Option<GamescopeBackend> {
        let (display, _root) = session::find_session()?;
        Some(GamescopeBackend {
            display,
            dir: look_dir()?,
        })
    }

    fn root(&mut self) -> Result<X11Root, BackendError> {
        if let Ok(root) = X11Root::connect(&self.display) {
            if session::is_root_server(&root).unwrap_or(false) {
                return Ok(root);
            }
        }
        let (display, root) = session::find_session()
            .ok_or_else(|| BackendError::Unavailable("no gamescope session found".into()))?;
        self.display = display;
        Ok(root)
    }

    /// Write the table for `saturation` unless it already exists. Files are
    /// written once, under a temporary name and renamed, so gamescope never
    /// reads a partial table.
    fn write_table(&self, saturation: Saturation) -> Result<PathBuf, BackendError> {
        let name = look::file_name(saturation);
        let path = self.dir.join(&name);
        if path.is_file() {
            return Ok(path);
        }
        fs::create_dir_all(&self.dir)
            .map_err(|e| apply_err(format!("creating {}: {e}", self.dir.display())))?;
        let temporary = self.dir.join(format!(".{name}.{}.tmp", std::process::id()));
        fs::write(&temporary, look::cube_text(saturation))
            .map_err(|e| apply_err(format!("writing {}: {e}", temporary.display())))?;
        if let Err(e) = fs::rename(&temporary, &path) {
            let _ = fs::remove_file(&temporary);
            return Err(apply_err(format!("replacing {}: {e}", path.display())));
        }
        Ok(path)
    }
}

impl Backend for GamescopeBackend {
    fn name(&self) -> &'static str {
        "gamescope"
    }

    fn cost(&self) -> CostNote {
        CostNote::ZeroCost
    }

    fn outputs(&self) -> Vec<Output> {
        vec![Output {
            id: "all".into(),
            human_name: "All outputs".into(),
        }]
    }

    /// gamescope's look is one global table, so the output is ignored.
    fn apply(&mut self, output: &Output, saturation: Saturation) -> Result<(), BackendError> {
        let saturation = look::quantize(saturation);
        if saturation.is_identity() {
            return self.reset(output);
        }
        let path = self.write_table(saturation)?;
        let value = path
            .to_str()
            .ok_or_else(|| apply_err(format!("{} is not valid UTF-8", path.display())))?
            .to_string();
        let mut root = self.root()?;
        take_over(&mut root, &self.dir, &value).map_err(|e| apply_err(format!("{e:#}")))?;
        prune_tables(&self.dir, &path);
        Ok(())
    }

    fn reset(&mut self, _output: &Output) -> Result<(), BackendError> {
        let mut root = self.root()?;
        give_back(&mut root, &self.dir).map_err(|e| apply_err(format!("{e:#}")))
    }
}

/// Point the look at Satur8's table `path`. A look something else set is
/// saved to `dir` first, so a later process can put it back. While Satur8's
/// own table is loaded nothing is saved, so the saved value stays the look
/// that was there before Satur8.
fn take_over(root: &mut dyn RootProps, dir: &Path, path: &str) -> anyhow::Result<()> {
    if let Some(foreign) = session::foreign_look(root)? {
        save_foreign(dir, &foreign)?;
    }
    root.set_string(LOOK, path)
}

/// Take Satur8's table off: put the saved foreign look back, or remove the
/// property when none was saved. A look something else set since, or a
/// property something else removed, is left as it is. The saved value is
/// used up either way.
fn give_back(root: &mut dyn RootProps, dir: &Path) -> anyhow::Result<()> {
    let saved = load_foreign(dir);
    session::clear_look(root, saved.as_deref())?;
    let _ = fs::remove_file(dir.join(FOREIGN_FILE));
    Ok(())
}

fn load_foreign(dir: &Path) -> Option<String> {
    fs::read_to_string(dir.join(FOREIGN_FILE))
        .ok()
        .filter(|value| !value.is_empty())
}

/// Write the saved foreign look under a temporary name and rename it, so a
/// crash never leaves half a path behind.
fn save_foreign(dir: &Path, value: &str) -> anyhow::Result<()> {
    use anyhow::Context as _;

    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(FOREIGN_FILE);
    let temporary = dir.join(format!(".{FOREIGN_FILE}.{}.tmp", std::process::id()));
    fs::write(&temporary, value).with_context(|| format!("writing {}", temporary.display()))?;
    if let Err(error) = fs::rename(&temporary, &path) {
        let _ = fs::remove_file(&temporary);
        return Err(error).with_context(|| format!("replacing {}", path.display()));
    }
    Ok(())
}

fn apply_err(message: String) -> BackendError {
    BackendError::Apply(format!("gamescope: {message}"))
}

/// `$XDG_STATE_HOME/satur8/gamescope`, by default
/// `~/.local/state/satur8/gamescope`: under `$HOME` so gamescope can read it,
/// and the directory the installer's uninstall removes.
fn look_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|home| !home.is_empty())
                .map(|home| PathBuf::from(home).join(".local/state"))
        })?;
    Some(base.join("satur8/gamescope"))
}

/// Remove every other Satur8 table. gamescope reads a table only when the
/// property changes, so once the property points at `keep` the rest are unused.
fn prune_tables(dir: &Path, keep: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let ours = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| look::saturation_from_path(name).is_some());
        if ours && path != keep {
            let _ = fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::FakeRoot;

    const OURS: &str = "/home/deck/.local/state/satur8/gamescope/look-g22-s2000.cube";
    const OURS_TOO: &str = "/home/deck/.local/state/satur8/gamescope/look-g22-s1500.cube";
    const STEAMS: &str = "/home/deck/looks/steam.cube";
    const OTHER: &str = "/home/deck/looks/other.cube";

    /// A state folder of its own under the system temp dir, removed on drop.
    struct StateDir(PathBuf);

    impl StateDir {
        fn new(name: &str) -> StateDir {
            let dir = std::env::temp_dir().join(format!(
                "satur8-gamescope-test-{}-{name}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            StateDir(dir)
        }

        fn saved(&self) -> Option<String> {
            fs::read_to_string(self.0.join(FOREIGN_FILE)).ok()
        }
    }

    impl Drop for StateDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn root_with(look: Option<&str>) -> FakeRoot {
        let mut root = FakeRoot::default();
        if let Some(look) = look {
            root.strings.insert(LOOK.into(), look.into());
        }
        root
    }

    fn look_of(root: &FakeRoot) -> Option<&str> {
        root.strings.get(LOOK).map(String::as_str)
    }

    /// `satur8 on` in one process, `satur8 off` in another: only the folder
    /// carries anything between them.
    #[test]
    fn a_foreign_look_comes_back_when_a_later_process_clears() {
        let state = StateDir::new("restore");
        let mut root = root_with(Some(STEAMS));

        take_over(&mut root, &state.0, OURS).unwrap();
        assert_eq!(look_of(&root), Some(OURS));
        assert_eq!(state.saved().as_deref(), Some(STEAMS));

        give_back(&mut root, &state.0).unwrap();
        assert_eq!(look_of(&root), Some(STEAMS));
        assert_eq!(state.saved(), None);
    }

    #[test]
    fn a_second_take_over_keeps_the_original_foreign_look() {
        let state = StateDir::new("twice");
        let mut root = root_with(Some(STEAMS));

        take_over(&mut root, &state.0, OURS).unwrap();
        take_over(&mut root, &state.0, OURS_TOO).unwrap();
        assert_eq!(look_of(&root), Some(OURS_TOO));
        assert_eq!(state.saved().as_deref(), Some(STEAMS));

        give_back(&mut root, &state.0).unwrap();
        assert_eq!(look_of(&root), Some(STEAMS));
    }

    #[test]
    fn clearing_with_nothing_saved_removes_the_property() {
        let state = StateDir::new("nothing-saved");
        let mut root = root_with(None);

        take_over(&mut root, &state.0, OURS).unwrap();
        assert_eq!(state.saved(), None);

        give_back(&mut root, &state.0).unwrap();
        assert_eq!(look_of(&root), None);
    }

    #[test]
    fn a_look_changed_by_something_else_meanwhile_is_left_and_the_file_dropped() {
        let state = StateDir::new("changed");
        let mut root = root_with(Some(STEAMS));

        take_over(&mut root, &state.0, OURS).unwrap();
        root.strings.insert(LOOK.into(), OTHER.into());

        give_back(&mut root, &state.0).unwrap();
        assert_eq!(look_of(&root), Some(OTHER));
        assert_eq!(state.saved(), None);
    }

    #[test]
    fn a_satur8_table_is_never_saved_as_foreign() {
        let state = StateDir::new("ours");
        let mut root = root_with(Some(OURS_TOO));

        take_over(&mut root, &state.0, OURS).unwrap();
        assert_eq!(state.saved(), None);
        assert!(!state.0.join(FOREIGN_FILE).exists());
    }
}
