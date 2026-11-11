//! `config.toml`: user settings.
//!
//! The database is the source of truth for contacts. `[[contacts]]` entries
//! here are a convenience for servers set up by hand: the service imports
//! (or updates) them into the database at startup.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Where the node's private key is kept (RNF-05).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyStorage {
    /// Use an existing key file if there is one, otherwise the OS keyring,
    /// falling back to a `0600` file when no keyring is available
    /// (headless servers).
    #[default]
    Auto,
    Keyring,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeedContact {
    pub name: String,
    pub node_id: String,
    #[serde(default)]
    pub auto_accept: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Name shown to contacts. Defaults to the host name.
    pub device_name: Option<String>,
    /// Where received files go. Defaults to `Downloads/Gigapair`.
    pub download_dir: Option<PathBuf>,
    /// Keep the computer awake while transfers are active (RF-07).
    pub keep_awake: bool,
    /// Desktop notifications for requests and finished transfers (RF-13).
    pub notifications: bool,
    /// Closing the app window keeps the app running in the system tray
    /// (RF-14). Only the desktop app reads it.
    pub close_to_tray: bool,
    /// Refuse to move data through a relay (fail rather than relay).
    pub direct_only: bool,
    /// Custom relay URL instead of the public n0 relays (RF-18).
    pub relay_url: Option<String>,
    /// Publish this device's relay URL (not its IP addresses) to n0's DNS
    /// so contacts can find it after a network change.
    pub discovery: bool,
    /// Upload limit in KiB/s; none means unlimited (RF-17).
    pub upload_limit_kib: Option<u64>,
    /// How many incoming transfers download at the same time.
    pub max_parallel_downloads: u32,
    pub key_storage: KeyStorage,
    /// Set once the user has seen the explanation of what a relay can see.
    pub relay_notice_shown: bool,
    pub contacts: Vec<SeedContact>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            device_name: None,
            download_dir: None,
            keep_awake: true,
            notifications: true,
            close_to_tray: true,
            direct_only: false,
            relay_url: None,
            discovery: true,
            upload_limit_kib: None,
            max_parallel_downloads: 1,
            key_storage: KeyStorage::Auto,
            relay_notice_shown: false,
            contacts: Vec::new(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path} is not valid: {source}. Fix or delete the file and try again")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("could not write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("unknown setting {0:?}. Run `gigapair config get` to list the settings")]
    UnknownKey(String),
    #[error("invalid value {value:?} for {key}: {hint}")]
    InvalidValue {
        key: String,
        value: String,
        hint: &'static str,
    },
}

/// Settings that `config get/set` can read and change.
pub const KEYS: &[&str] = &[
    "device_name",
    "download_dir",
    "keep_awake",
    "notifications",
    "close_to_tray",
    "direct_only",
    "relay_url",
    "discovery",
    "upload_limit_kib",
    "max_parallel_downloads",
    "key_storage",
];

impl Config {
    /// Loads `path`, or the defaults if it doesn't exist.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|source| ConfigError::Parse {
                path: path.to_path_buf(),
                source,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigError::Read {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// Writes the config atomically (temp file + rename).
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let write_err = |source| ConfigError::Write {
            path: path.to_path_buf(),
            source,
        };
        let text = toml::to_string_pretty(self)
            .map_err(|e| write_err(std::io::Error::other(e.to_string())))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(write_err)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(write_err)?;
        std::fs::rename(&tmp, path).map_err(write_err)
    }

    /// The device name to show contacts.
    pub fn effective_device_name(&self) -> String {
        self.device_name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(host_name)
    }

    pub fn effective_download_dir(&self, default: &Path) -> PathBuf {
        self.download_dir
            .clone()
            .unwrap_or_else(|| default.to_path_buf())
    }

    /// Reads one setting as text. `None` means unset.
    pub fn get(&self, key: &str) -> Result<Option<String>, ConfigError> {
        Ok(match key {
            "device_name" => self.device_name.clone(),
            "download_dir" => self.download_dir.as_ref().map(|p| p.display().to_string()),
            "keep_awake" => Some(self.keep_awake.to_string()),
            "notifications" => Some(self.notifications.to_string()),
            "close_to_tray" => Some(self.close_to_tray.to_string()),
            "direct_only" => Some(self.direct_only.to_string()),
            "relay_url" => self.relay_url.clone(),
            "discovery" => Some(self.discovery.to_string()),
            "upload_limit_kib" => self.upload_limit_kib.map(|v| v.to_string()),
            "max_parallel_downloads" => Some(self.max_parallel_downloads.to_string()),
            "key_storage" => Some(
                match self.key_storage {
                    KeyStorage::Auto => "auto",
                    KeyStorage::Keyring => "keyring",
                    KeyStorage::File => "file",
                }
                .to_owned(),
            ),
            other => return Err(ConfigError::UnknownKey(other.to_owned())),
        })
    }

    /// Changes one setting from text. An empty value unsets optional
    /// settings.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), ConfigError> {
        let value = value.trim();
        let invalid = |hint| ConfigError::InvalidValue {
            key: key.to_owned(),
            value: value.to_owned(),
            hint,
        };
        let opt = |v: &str| (!v.is_empty()).then(|| v.to_owned());
        let boolean = |v: &str| match v.to_ascii_lowercase().as_str() {
            "true" | "on" | "yes" | "1" => Ok(true),
            "false" | "off" | "no" | "0" => Ok(false),
            _ => Err(invalid("use on or off")),
        };
        match key {
            "device_name" => self.device_name = opt(value),
            "download_dir" => self.download_dir = opt(value).map(PathBuf::from),
            "keep_awake" => self.keep_awake = boolean(value)?,
            "notifications" => self.notifications = boolean(value)?,
            "close_to_tray" => self.close_to_tray = boolean(value)?,
            "direct_only" => self.direct_only = boolean(value)?,
            "relay_url" => {
                if let Some(v) = opt(value) {
                    v.parse::<iroh::RelayUrl>()
                        .map_err(|_| invalid("use a full URL such as https://relay.example.com"))?;
                    self.relay_url = Some(v);
                } else {
                    self.relay_url = None;
                }
            }
            "discovery" => self.discovery = boolean(value)?,
            "upload_limit_kib" => {
                self.upload_limit_kib = match opt(value) {
                    None => None,
                    Some(v) => match v.parse::<u64>() {
                        Ok(0) => None,
                        Ok(n) => Some(n),
                        Err(_) => return Err(invalid("use a number of KiB/s, or 0 for no limit")),
                    },
                }
            }
            "max_parallel_downloads" => {
                self.max_parallel_downloads = value
                    .parse::<u32>()
                    .ok()
                    .filter(|n| (1..=8).contains(n))
                    .ok_or_else(|| invalid("use a number from 1 to 8"))?
            }
            "key_storage" => {
                self.key_storage = match value {
                    "auto" => KeyStorage::Auto,
                    "keyring" => KeyStorage::Keyring,
                    "file" => KeyStorage::File,
                    _ => return Err(invalid("use auto, keyring or file")),
                }
            }
            other => return Err(ConfigError::UnknownKey(other.to_owned())),
        }
        Ok(())
    }
}

/// This computer's name, or "Gigapair device" if it can't be read.
pub fn host_name() -> String {
    let from_env = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|s| !s.trim().is_empty());
    if let Some(name) = from_env {
        return name;
    }
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        // SAFETY: the buffer is valid for `buf.len()` bytes.
        let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
        if rc == 0 {
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            let name = String::from_utf8_lossy(&buf[..end]).trim().to_owned();
            if !name.is_empty() {
                return name;
            }
        }
    }
    "Gigapair device".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let c = Config::load(&dir.path().join("config.toml")).unwrap();
        assert_eq!(c, Config::default());
        assert!(c.keep_awake);
    }

    #[test]
    fn parses_server_example() {
        let text = r#"
download_dir = "/srv/gigapair/incoming"
keep_awake = true

[[contacts]]
name = "workstation"
node_id = "abc"
auto_accept = true
"#;
        let c: Config = toml::from_str(text).unwrap();
        assert_eq!(
            c.download_dir,
            Some(PathBuf::from("/srv/gigapair/incoming"))
        );
        assert_eq!(c.contacts.len(), 1);
        assert!(c.contacts[0].auto_accept);
    }

    #[test]
    fn save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/config.toml");
        let mut c = Config::default();
        c.set("direct_only", "on").unwrap();
        c.set("upload_limit_kib", "2048").unwrap();
        c.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), c);
    }

    #[test]
    fn get_set_validation() {
        let mut c = Config::default();
        assert!(matches!(
            c.set("nope", "1"),
            Err(ConfigError::UnknownKey(_))
        ));
        assert!(c.set("keep_awake", "maybe").is_err());
        assert!(c.set("relay_url", "not a url").is_err());
        c.set("relay_url", "https://relay.example.com").unwrap();
        assert!(c.get("relay_url").unwrap().is_some());
        c.set("relay_url", "").unwrap();
        assert_eq!(c.get("relay_url").unwrap(), None);
        for key in KEYS {
            c.get(key).unwrap();
        }
    }
}
