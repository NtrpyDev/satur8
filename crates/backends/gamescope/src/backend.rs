//! Steam Game Mode backend: saturation through the running gamescope's look
//! table, which Steam's own vibrancy, white point and night mode run on top of.
//!
//! It holds no X connection between calls. gamescope can restart or the
//! session can switch modes at any time, and a backend, once selected, is
//! never re-detected, so each apply or reset opens a short connection and
//! re-probes when the remembered display is no longer gamescope's.

use std::fs;
use std::path::{Path, PathBuf};

use satur8_core::{Backend, BackendError, CostNote, Output, Saturation};

use crate::look;
use crate::session::{self, X11Root};

/// The gamescope Game Mode backend.
pub struct GamescopeBackend {
    display: String,
    dir: PathBuf,
    /// A look something else had set before Satur8 replaced it, put back on
    /// reset. Kept in memory only.
    foreign: Option<String>,
}

impl GamescopeBackend {
    /// A backend for the running gamescope session, if there is one.
    pub fn detect() -> Option<GamescopeBackend> {
        let (display, _root) = session::find_session()?;
        Some(GamescopeBackend {
            display,
            dir: look_dir()?,
            foreign: None,
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
        let replaced =
            session::apply_look(&mut root, &value).map_err(|e| apply_err(format!("{e:#}")))?;
        if replaced.is_some() {
            self.foreign = replaced;
        }
        prune_tables(&self.dir, &path);
        Ok(())
    }

    fn reset(&mut self, _output: &Output) -> Result<(), BackendError> {
        let mut root = self.root()?;
        session::clear_look(&mut root, self.foreign.as_deref())
            .map_err(|e| apply_err(format!("{e:#}")))?;
        self.foreign = None;
        Ok(())
    }
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
