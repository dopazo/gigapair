//! The node's private key (RNF-05).
//!
//! Kept in the OS keyring (Windows Credential Manager, macOS Keychain,
//! Secret Service on Linux). Headless servers without a keyring use a file
//! readable only by the owner (`0600`). The key is never logged.

use std::io::Write;
use std::path::Path;

use iroh::SecretKey;

use crate::config::KeyStorage;
use crate::dirs::AppDirs;

const KEYRING_SERVICE: &str = "gigapair";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyLocation {
    Keyring,
    File,
}

#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error(
        "could not use the system keyring: {0}. Set key_storage = \"file\" in config.toml on headless machines"
    )]
    Keyring(String),
    #[error("could not read or write the key file {path}: {source}")]
    File {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error(
        "the key file {0} is corrupt; restore it from a backup or delete it to create a new identity (contacts must pair again)"
    )]
    Corrupt(std::path::PathBuf),
}

/// Loads the node key, creating and storing a new one on first run.
pub fn load_or_create(
    dirs: &AppDirs,
    storage: KeyStorage,
) -> Result<(SecretKey, KeyLocation), KeyError> {
    let file = dirs.key_file();
    let account = keyring_account(dirs);
    match storage {
        KeyStorage::File => Ok((file_load_or_create(&file)?, KeyLocation::File)),
        KeyStorage::Keyring => Ok((keyring_load_or_create(&account)?, KeyLocation::Keyring)),
        KeyStorage::Auto => {
            if file.exists() {
                return Ok((file_load_or_create(&file)?, KeyLocation::File));
            }
            match keyring_load_or_create(&account) {
                Ok(key) => Ok((key, KeyLocation::Keyring)),
                Err(e) => {
                    tracing::warn!(
                        "system keyring unavailable ({e}); storing the node key in a 0600 file"
                    );
                    Ok((file_load_or_create(&file)?, KeyLocation::File))
                }
            }
        }
    }
}

fn keyring_account(dirs: &AppDirs) -> String {
    if dirs.custom_root {
        format!("node-key-{}", dirs.instance_tag())
    } else {
        "node-key".to_owned()
    }
}

fn keyring_load_or_create(account: &str) -> Result<SecretKey, KeyError> {
    let err = |e: keyring::Error| KeyError::Keyring(e.to_string());
    let entry = keyring::Entry::new(KEYRING_SERVICE, account).map_err(err)?;
    match entry.get_secret() {
        Ok(bytes) => {
            let arr: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| KeyError::Keyring("stored key has the wrong length".into()))?;
            Ok(SecretKey::from_bytes(&arr))
        }
        Err(keyring::Error::NoEntry) => {
            let key = SecretKey::generate();
            entry.set_secret(&key.to_bytes()).map_err(err)?;
            // Read back: some stores accept writes they can't return.
            let back = entry.get_secret().map_err(err)?;
            if back.as_slice() != key.to_bytes().as_slice() {
                return Err(KeyError::Keyring("key could not be read back".into()));
            }
            Ok(key)
        }
        Err(e) => Err(err(e)),
    }
}

fn file_load_or_create(path: &Path) -> Result<SecretKey, KeyError> {
    let io_err = |source| KeyError::File {
        path: path.to_path_buf(),
        source,
    };
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let bytes = data_encoding::HEXLOWER_PERMISSIVE
                .decode(text.trim().as_bytes())
                .map_err(|_| KeyError::Corrupt(path.to_path_buf()))?;
            let arr: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| KeyError::Corrupt(path.to_path_buf()))?;
            Ok(SecretKey::from_bytes(&arr))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(io_err)?;
            }
            let key = SecretKey::generate();
            let hex = data_encoding::HEXLOWER.encode(&key.to_bytes());
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts.open(path).map_err(io_err)?;
            f.write_all(hex.as_bytes()).map_err(io_err)?;
            f.sync_all().map_err(io_err)?;
            Ok(key)
        }
        Err(e) => Err(io_err(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_key_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = AppDirs::under(dir.path());
        let (a, loc) = load_or_create(&dirs, KeyStorage::File).unwrap();
        assert_eq!(loc, KeyLocation::File);
        let (b, _) = load_or_create(&dirs, KeyStorage::File).unwrap();
        assert_eq!(a.public(), b.public());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dirs.key_file())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn auto_prefers_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = AppDirs::under(dir.path());
        let (a, _) = load_or_create(&dirs, KeyStorage::File).unwrap();
        let (b, loc) = load_or_create(&dirs, KeyStorage::Auto).unwrap();
        assert_eq!(loc, KeyLocation::File);
        assert_eq!(a.public(), b.public());
    }

    #[test]
    fn corrupt_file_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = AppDirs::under(dir.path());
        std::fs::create_dir_all(&dirs.data).unwrap();
        std::fs::write(dirs.key_file(), "zz").unwrap();
        assert!(matches!(
            load_or_create(&dirs, KeyStorage::File),
            Err(KeyError::Corrupt(_))
        ));
    }
}
