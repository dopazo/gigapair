//! Deleting everything Gigapair keeps about this device, for someone who
//! wants to start over or leave without traces. Uninstalling keeps the data
//! on purpose (updates uninstall too), so this is the explicit way out.
//!
//! Received files are never touched: they belong to the user.

use std::path::{Path, PathBuf};

use crate::api::Request;
use crate::dirs::AppDirs;
use crate::ipc::{Client, IpcError};
use crate::{keys, service};

/// What [`wipe`] couldn't delete.
#[derive(Debug, Default)]
pub struct Leftovers {
    /// Paths still on disk, e.g. a log file another process has open.
    pub paths: Vec<PathBuf>,
    /// The node key couldn't be deleted from the keyring.
    pub key_error: Option<String>,
}

impl Leftovers {
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty() && self.key_error.is_none()
    }
}

/// The whole reset, as the CLI and the app run it: asks a running service
/// to forget every contact and stop ([`Request::Reset`]), removes the start
/// at login, waits for the service to exit and then [`wipe`]s.
pub async fn reset_device(dirs: &AppDirs) -> Result<Leftovers, IpcError> {
    match Client::connect(dirs, "reset").await {
        Ok(mut c) => {
            c.call(Request::Reset).await?;
        }
        Err(IpcError::NotRunning) => {}
        Err(e) => return Err(e),
    }
    // Otherwise the next login starts a service that creates a new identity.
    // Only for the standard folders: a custom root's instance may not be the
    // one installed at login.
    if !dirs.custom_root
        && let Err(e) = service::uninstall()
    {
        tracing::warn!("could not remove the start at login: {e}");
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        match Client::connect(dirs, "reset").await {
            Err(IpcError::NotRunning) => break,
            _ if tokio::time::Instant::now() >= deadline => {
                return Err(IpcError::Service(
                    "the Gigapair service didn't stop, so nothing was deleted; quit Gigapair and try again".into(),
                ));
            }
            _ => tokio::time::sleep(std::time::Duration::from_millis(200)).await,
        }
    }
    // The socket closes first; the process may still be closing its
    // database for a few seconds. Try again until only files that another
    // program may hold for longer (logs) are left.
    loop {
        let left = wipe(dirs);
        let busy = left.paths.iter().any(|p| !p.starts_with(&dirs.logs));
        if !busy || tokio::time::Instant::now() >= deadline {
            return Ok(left);
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

/// Deletes the node key, the settings, the database (contacts, queue,
/// history) and the logs. The service must not be running. Never deletes
/// received files. Best effort: whatever is left is reported.
pub fn wipe(dirs: &AppDirs) -> Leftovers {
    let mut left = Leftovers::default();
    if let Err(e) = keys::delete(dirs) {
        left.key_error = Some(e.to_string());
    }
    for dir in [&dirs.config, &dirs.data, &dirs.logs] {
        remove_tree(dir, &mut left.paths);
    }
    // The standard folders sit inside per-app parents
    // (`%LOCALAPPDATA%\gigapair\Gigapair\data`); drop them once empty. A
    // custom root (`GIGAPAIR_HOME`) is the user's folder and stays.
    if !dirs.custom_root {
        for dir in [&dirs.config, &dirs.data, &dirs.logs] {
            remove_empty_parents(dir, 2);
        }
    }
    left
}

/// Removes `dir`; if that fails (a file in use on Windows), removes what it
/// can and records what is left.
fn remove_tree(dir: &Path, left: &mut Vec<PathBuf>) {
    if !dir.exists() || std::fs::remove_dir_all(dir).is_ok() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        left.push(dir.to_path_buf());
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let removed = if path.is_dir() {
            remove_tree(&path, left);
            !path.exists()
        } else {
            std::fs::remove_file(&path).is_ok()
        };
        if !removed && !path.is_dir() {
            left.push(path);
        }
    }
    let _ = std::fs::remove_dir(dir);
}

fn remove_empty_parents(dir: &Path, levels: usize) {
    let mut current = dir.parent();
    for _ in 0..levels {
        let Some(p) = current else { return };
        if std::fs::remove_dir(p).is_err() {
            return;
        }
        current = p.parent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::KeyStorage;

    #[test]
    fn wipe_removes_data_but_not_received_files() {
        let root = tempfile::tempdir().unwrap();
        let dirs = AppDirs::under(root.path());
        dirs.create_all().unwrap();
        keys::load_or_create(&dirs, KeyStorage::File).unwrap();
        crate::db::Db::open(&dirs.db_file()).unwrap();
        std::fs::write(dirs.config_file(), "device_name = \"x\"").unwrap();
        std::fs::write(dirs.logs.join("gigapaird.log"), "log").unwrap();
        std::fs::create_dir_all(&dirs.downloads).unwrap();
        std::fs::write(dirs.downloads.join("movie.mov"), "data").unwrap();

        let left = wipe(&dirs);
        assert!(left.paths.is_empty(), "{left:?}");
        assert!(!dirs.config.exists());
        assert!(!dirs.data.exists());
        assert!(!dirs.logs.exists());
        assert!(dirs.downloads.join("movie.mov").exists());
        // The custom root itself stays.
        assert!(root.path().exists());
    }
}
