//! Where Gigapair keeps its files on each system.
//!
//! Standard locations come from the `directories` crate. Setting
//! `GIGAPAIR_HOME` puts everything under one folder instead, which lets two
//! instances run side by side on one computer (handy for testing).

use std::path::{Path, PathBuf};

/// Environment variable that overrides all locations.
pub const HOME_ENV: &str = "GIGAPAIR_HOME";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppDirs {
    /// Holds `config.toml`.
    pub config: PathBuf,
    /// Holds the database, the IPC socket and (in file mode) the node key.
    pub data: PathBuf,
    pub logs: PathBuf,
    /// Default download folder.
    pub downloads: PathBuf,
    /// True when created from `GIGAPAIR_HOME` or [`AppDirs::under`].
    pub custom_root: bool,
}

#[derive(Debug, thiserror::Error)]
#[error("could not find the home folder; set {HOME_ENV} to choose where Gigapair stores its data")]
pub struct NoHomeDir;

impl AppDirs {
    /// Locations for this user, honoring `GIGAPAIR_HOME`.
    pub fn from_env() -> Result<Self, NoHomeDir> {
        match std::env::var_os(HOME_ENV) {
            Some(root) if !root.is_empty() => Ok(Self::under(Path::new(&root))),
            _ => Self::standard(),
        }
    }

    /// Everything under `root`.
    pub fn under(root: &Path) -> Self {
        Self {
            config: root.join("config"),
            data: root.join("data"),
            logs: root.join("logs"),
            downloads: root.join("downloads"),
            custom_root: true,
        }
    }

    fn standard() -> Result<Self, NoHomeDir> {
        let project =
            directories::ProjectDirs::from("dev", "gigapair", "Gigapair").ok_or(NoHomeDir)?;
        let base = directories::BaseDirs::new().ok_or(NoHomeDir)?;
        let user = directories::UserDirs::new();

        let logs = if cfg!(target_os = "macos") {
            base.home_dir().join("Library/Logs/Gigapair")
        } else if let Some(state) = project.state_dir() {
            state.to_path_buf()
        } else {
            project.data_local_dir().join("logs")
        };

        let downloads = user
            .as_ref()
            .and_then(|u| u.download_dir().map(Path::to_path_buf))
            .unwrap_or_else(|| base.home_dir().join("Downloads"))
            .join("Gigapair");

        Ok(Self {
            config: project.config_dir().to_path_buf(),
            data: project.data_local_dir().to_path_buf(),
            logs,
            downloads,
            custom_root: false,
        })
    }

    pub fn config_file(&self) -> PathBuf {
        self.config.join("config.toml")
    }

    pub fn db_file(&self) -> PathBuf {
        self.data.join("gigapair.db")
    }

    pub fn key_file(&self) -> PathBuf {
        self.data.join("node.key")
    }

    /// Creates the config, data and log folders.
    pub fn create_all(&self) -> std::io::Result<()> {
        for dir in [&self.config, &self.data, &self.logs] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    /// A short, stable tag for this set of folders: tells instances apart in
    /// the IPC socket name and keyring entry.
    pub fn instance_tag(&self) -> String {
        let hash = blake3::hash(self.data.to_string_lossy().as_bytes());
        hash.to_hex()[..12].to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_root() {
        let d = AppDirs::under(Path::new("/x"));
        assert_eq!(d.config_file(), Path::new("/x/config/config.toml"));
        assert_eq!(d.db_file(), Path::new("/x/data/gigapair.db"));
        assert!(d.custom_root);
    }

    #[test]
    fn instance_tags_differ() {
        let a = AppDirs::under(Path::new("/a"));
        let b = AppDirs::under(Path::new("/b"));
        assert_ne!(a.instance_tag(), b.instance_tag());
        assert_eq!(a.instance_tag().len(), 12);
    }
}
