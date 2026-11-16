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
        "could not use the system keyring: {0}. If it is locked or not running yet, unlock it and start Gigapair again"
    )]
    Keyring(String),
    #[error(
        "this device's key was not found, although Gigapair has been used here before ({0}). If the system keyring is locked or not running yet, unlock it and start Gigapair again. To start over with a new identity (every contact must pair again), delete {1}"
    )]
    Missing(String, std::path::PathBuf),
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
///
/// A new key is created only when this device has no identity yet (no
/// database). Afterwards a key that can't be found is an error, never a
/// silent new identity: that would orphan every paired contact, e.g. when
/// the service starts before the keyring is unlocked. Switching
/// `key_storage` between `file` and `keyring` carries the key over.
pub fn load_or_create(
    dirs: &AppDirs,
    storage: KeyStorage,
) -> Result<(SecretKey, KeyLocation), KeyError> {
    let file = dirs.key_file();
    let account = keyring_account(dirs);
    let used_before = dirs.db_file().exists();
    let missing = |place: &str| KeyError::Missing(place.to_owned(), dirs.db_file());
    match storage {
        KeyStorage::File => {
            if let Some(key) = file_read(&file)? {
                return Ok((key, KeyLocation::File));
            }
            if used_before {
                // Moving from the keyring to a file.
                return match keyring_read(&account) {
                    Ok(Some(key)) => {
                        file_write(&file, &key)?;
                        Ok((key, KeyLocation::File))
                    }
                    _ => Err(missing("not in the key file nor the keyring")),
                };
            }
            Ok((file_create(&file)?, KeyLocation::File))
        }
        KeyStorage::Keyring => {
            if let Some(key) = keyring_read(&account)? {
                return Ok((key, KeyLocation::Keyring));
            }
            if let Some(key) = file_read(&file)? {
                // Moving from a file to the keyring.
                keyring_write(&account, &key)?;
                return Ok((key, KeyLocation::Keyring));
            }
            if used_before {
                return Err(missing("not in the keyring nor the key file"));
            }
            Ok((keyring_create(&account)?, KeyLocation::Keyring))
        }
        KeyStorage::Auto => {
            if let Some(key) = file_read(&file)? {
                return Ok((key, KeyLocation::File));
            }
            match keyring_read(&account) {
                Ok(Some(key)) => Ok((key, KeyLocation::Keyring)),
                Ok(None) if used_before => Err(missing("not in the keyring nor the key file")),
                Err(e) if used_before => Err(e),
                Ok(None) => match keyring_create(&account) {
                    Ok(key) => Ok((key, KeyLocation::Keyring)),
                    Err(e) => file_fallback(&file, e),
                },
                Err(e) => file_fallback(&file, e),
            }
        }
    }
}

/// First run without a usable keyring (headless servers).
fn file_fallback(file: &Path, e: KeyError) -> Result<(SecretKey, KeyLocation), KeyError> {
    tracing::warn!("system keyring unavailable ({e}); storing the node key in a 0600 file");
    Ok((file_create(file)?, KeyLocation::File))
}

fn keyring_account(dirs: &AppDirs) -> String {
    if dirs.custom_root {
        format!("node-key-{}", dirs.instance_tag())
    } else {
        "node-key".to_owned()
    }
}

fn keyring_err(e: keyring::Error) -> KeyError {
    KeyError::Keyring(e.to_string())
}

fn keyring_entry(account: &str) -> Result<keyring::Entry, KeyError> {
    keyring::Entry::new(KEYRING_SERVICE, account).map_err(keyring_err)
}

/// The key stored in the keyring, or `None` if there is no entry.
fn keyring_read(account: &str) -> Result<Option<SecretKey>, KeyError> {
    match keyring_entry(account)?.get_secret() {
        Ok(bytes) => {
            let arr: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| KeyError::Keyring("stored key has the wrong length".into()))?;
            Ok(Some(SecretKey::from_bytes(&arr)))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(keyring_err(e)),
    }
}

fn keyring_write(account: &str, key: &SecretKey) -> Result<(), KeyError> {
    let entry = keyring_entry(account)?;
    entry.set_secret(&key.to_bytes()).map_err(keyring_err)?;
    // Read back: some stores accept writes they can't return.
    let back = entry.get_secret().map_err(keyring_err)?;
    if back.as_slice() != key.to_bytes().as_slice() {
        return Err(KeyError::Keyring("key could not be read back".into()));
    }
    Ok(())
}

fn keyring_create(account: &str) -> Result<SecretKey, KeyError> {
    let key = SecretKey::generate();
    keyring_write(account, &key)?;
    Ok(key)
}

/// The key stored in `path`, or `None` if the file doesn't exist.
fn file_read(path: &Path) -> Result<Option<SecretKey>, KeyError> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let bytes = data_encoding::HEXLOWER_PERMISSIVE
                .decode(text.trim().as_bytes())
                .map_err(|_| KeyError::Corrupt(path.to_path_buf()))?;
            let arr: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| KeyError::Corrupt(path.to_path_buf()))?;
            Ok(Some(SecretKey::from_bytes(&arr)))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(KeyError::File {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Writes `key` to a new `0600` file; fails if the file exists.
fn file_write(path: &Path, key: &SecretKey) -> Result<(), KeyError> {
    let io_err = |source| KeyError::File {
        path: path.to_path_buf(),
        source,
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io_err)?;
    }
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
    f.sync_all().map_err(io_err)
}

fn file_create(path: &Path) -> Result<SecretKey, KeyError> {
    let key = SecretKey::generate();
    file_write(path, &key)?;
    Ok(key)
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
    fn no_new_identity_once_used() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = AppDirs::under(dir.path());
        load_or_create(&dirs, KeyStorage::File).unwrap();
        crate::db::Db::open(&dirs.db_file()).unwrap();
        // The key is lost, but the device has data: refuse instead of
        // creating a new identity.
        std::fs::remove_file(dirs.key_file()).unwrap();
        assert!(matches!(
            load_or_create(&dirs, KeyStorage::File),
            Err(KeyError::Missing(..))
        ));
        assert!(!dirs.key_file().exists());
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
