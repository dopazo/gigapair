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
    ///
    /// A custom root is hashed in a normalized form, so the same folder
    /// written two ways (`C:\gp` and `c:/gp/`, a relative path and its
    /// absolute form) is one instance: an app and a service started with
    /// either spelling find each other. The standard folders come from the
    /// system, always written the same way, and keep their tag as it was.
    pub fn instance_tag(&self) -> String {
        if self.custom_root {
            tag(&normalized(&self.data))
        } else {
            self.legacy_instance_tag()
        }
    }

    /// The tag before custom roots were normalized: the path hashed as
    /// written. Kept to find keyring entries saved under it.
    pub(crate) fn legacy_instance_tag(&self) -> String {
        tag(&self.data.to_string_lossy())
    }
}

fn tag(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex()[..12].to_string()
}

/// `path` made absolute, with `.` and `..` resolved, no trailing separator
/// and one separator style. On Windows, where paths ignore case, also
/// lowercased, with `\?\` prefixes reduced to their usual form. Works on
/// the text only: the folder doesn't have to exist.
fn normalized(path: &Path) -> String {
    use std::path::{Component, Prefix};

    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let sep = std::path::MAIN_SEPARATOR_STR;
    let mut prefix = String::new();
    let mut rooted = false;
    let mut parts: Vec<String> = Vec::new();
    for c in abs.components() {
        match c {
            Component::Prefix(p) => {
                prefix = match p.kind() {
                    Prefix::Disk(d) | Prefix::VerbatimDisk(d) => format!("{}:", d as char),
                    Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
                        format!(
                            r"\\{}\{}",
                            server.to_string_lossy(),
                            share.to_string_lossy()
                        )
                    }
                    _ => p.as_os_str().to_string_lossy().into_owned(),
                }
            }
            Component::RootDir => rooted = true,
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop();
            }
            Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
        }
    }
    let mut out = prefix;
    if rooted {
        out.push_str(sep);
    }
    out.push_str(&parts.join(sep));
    if cfg!(windows) {
        out = out.to_lowercase();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_tags_differ() {
        let a = AppDirs::under(Path::new("/a"));
        let b = AppDirs::under(Path::new("/b"));
        assert_ne!(a.instance_tag(), b.instance_tag());
        assert_eq!(a.instance_tag().len(), 12);
    }

    #[test]
    fn one_folder_written_two_ways_is_one_instance() {
        let tag = |p: &str| AppDirs::under(Path::new(p)).instance_tag();
        let base = tag("/srv/gp");
        for same in ["/srv/gp/", "/srv/./gp", "/srv/other/../gp", "/srv//gp"] {
            assert_eq!(tag(same), base, "{same}");
        }
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(
            tag("rel/gp"),
            AppDirs::under(&cwd.join("rel/gp")).instance_tag()
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_spellings_are_one_instance() {
        let tag = |p: &str| AppDirs::under(Path::new(p)).instance_tag();
        let base = tag(r"C:\Temp\gp");
        for same in [
            "C:/Temp/gp",
            r"c:\temp\GP\",
            r"\\?\C:\Temp\gp",
            r"C:\Temp\.\gp",
        ] {
            assert_eq!(tag(same), base, "{same}");
        }
        assert_eq!(tag(r"\\server\share\gp"), tag(r"\\?\UNC\server\share\gp"));
        assert_ne!(tag(r"D:\Temp\gp"), base);
    }

    #[test]
    fn standard_folders_keep_their_tag() {
        if let Ok(dirs) = AppDirs::standard() {
            assert_eq!(dirs.instance_tag(), dirs.legacy_instance_tag());
        }
    }
}
