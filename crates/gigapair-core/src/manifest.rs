//! What gets sent: scanning the chosen files and folders, and hashing them.
//!
//! Every file is split into fixed-size blocks. The sender computes the
//! BLAKE3 hash of each block and of the whole file in one streaming pass.
//! The receiver checks each block as it arrives against the block list,
//! which is itself pinned by a hash in the offer, and re-hashes the whole
//! file at the end against the root hash (RF-06, RNF-06). The root hash is
//! a plain BLAKE3 of the file, so `b3sum` gives the same value.

use std::collections::HashSet;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::paths::unique_name;

/// Size of a verification and resume block.
pub const BLOCK_SIZE: u64 = 4 * 1024 * 1024;

/// Size of one block hash in the block list.
pub const HASH_LEN: usize = 32;

/// Number of blocks in a file of `size` bytes (0 for an empty file).
pub fn block_count(size: u64) -> u64 {
    size.div_ceil(BLOCK_SIZE)
}

/// Length of block `index` in a file of `size` bytes.
pub fn block_len(size: u64, index: u64) -> u64 {
    let start = index * BLOCK_SIZE;
    size.saturating_sub(start).min(BLOCK_SIZE)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedFile {
    /// `/`-separated path relative to the transfer root, starting with the
    /// top-level name.
    pub rel_path: String,
    pub src: PathBuf,
    pub size: u64,
    pub mtime_ns: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedDir {
    pub rel_path: String,
    pub mtime_ns: i64,
}

#[derive(Debug, Default)]
pub struct Scan {
    pub files: Vec<ScannedFile>,
    /// Every folder, including empty ones, so the receiver can recreate
    /// the tree and its modification times.
    pub dirs: Vec<ScannedDir>,
    /// Things that were skipped (symlinks, unreadable entries).
    pub warnings: Vec<String>,
    pub total_size: u64,
    /// Top-level names, in order.
    pub roots: Vec<String>,
    /// Which roots are folders.
    pub root_is_dir: Vec<bool>,
}

impl Scan {
    /// Short description for lists: `talk.mp4`, `talks/`, `talk.mp4 + 2 more`.
    pub fn label(&self) -> String {
        make_label(&self.roots, &self.root_is_dir)
    }
}

pub fn make_label(roots: &[String], is_dir: &[bool]) -> String {
    let Some(first) = roots.first() else {
        return "(empty)".to_owned();
    };
    let mut label = first.clone();
    if is_dir.first().copied().unwrap_or(false) {
        label.push('/');
    }
    if roots.len() > 1 {
        label.push_str(&format!(" + {} more", roots.len() - 1));
    }
    label
}

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("{0} does not exist")]
    NotFound(PathBuf),
    #[error("could not read {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("nothing to send: {0}")]
    Empty(String),
}

pub fn mtime_ns(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

pub fn system_time_from_ns(ns: i64) -> SystemTime {
    UNIX_EPOCH + std::time::Duration::from_nanos(ns.max(0) as u64)
}

/// Lists everything under `sources`. Symlinks are skipped with a warning,
/// never followed. Two sources with the same name get `name (1)`.
pub fn scan(sources: &[PathBuf]) -> Result<Scan, ScanError> {
    let mut scan = Scan::default();
    let mut root_names: HashSet<String> = HashSet::new();

    for src in sources {
        let meta = std::fs::symlink_metadata(src).map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                ScanError::NotFound(src.clone())
            } else {
                ScanError::Io {
                    path: src.clone(),
                    source: e,
                }
            }
        })?;
        if meta.file_type().is_symlink() {
            scan.warnings.push(format!(
                "skipped symlink {} (links are not followed)",
                src.display()
            ));
            continue;
        }
        let Some(name) = src.file_name() else {
            return Err(ScanError::Io {
                path: src.clone(),
                source: io::Error::other("a folder with a name is needed, not a drive root"),
            });
        };
        let name = name_to_string(name, src, &mut scan.warnings);
        let root = unique_name(Path::new(""), &name, |p| {
            root_names.contains(&p.to_string_lossy().to_string())
        })
        .to_string_lossy()
        .to_string();
        root_names.insert(root.clone());

        if meta.is_dir() {
            scan.roots.push(root.clone());
            scan.root_is_dir.push(true);
            scan.dirs.push(ScannedDir {
                rel_path: root.clone(),
                mtime_ns: mtime_ns(&meta),
            });
            walk(src, &root, &mut scan);
        } else if meta.is_file() {
            scan.roots.push(root.clone());
            scan.root_is_dir.push(false);
            scan.total_size += meta.len();
            scan.files.push(ScannedFile {
                rel_path: root,
                src: src.clone(),
                size: meta.len(),
                mtime_ns: mtime_ns(&meta),
            });
        } else {
            scan.warnings
                .push(format!("skipped {} (not a regular file)", src.display()));
        }
    }

    if scan.roots.is_empty() {
        let why = scan
            .warnings
            .first()
            .cloned()
            .unwrap_or_else(|| "no files or folders given".to_owned());
        return Err(ScanError::Empty(why));
    }
    Ok(scan)
}

fn name_to_string(name: &std::ffi::OsStr, path: &Path, warnings: &mut Vec<String>) -> String {
    match name.to_str() {
        Some(s) => s.to_owned(),
        None => {
            warnings.push(format!(
                "{} has a name that is not valid Unicode; it will be sent with replacement characters",
                path.display()
            ));
            name.to_string_lossy().into_owned()
        }
    }
}

fn walk(dir: &Path, rel: &str, scan: &mut Scan) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            scan.warnings
                .push(format!("skipped folder {}: {e}", dir.display()));
            return;
        }
    };
    let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                scan.warnings
                    .push(format!("skipped {}: {e}", path.display()));
                continue;
            }
        };
        let name = name_to_string(&entry.file_name(), &path, &mut scan.warnings);
        let child = format!("{rel}/{name}");
        let ft = meta.file_type();
        if ft.is_symlink() {
            scan.warnings.push(format!(
                "skipped symlink {} (links are not followed)",
                path.display()
            ));
        } else if ft.is_dir() {
            scan.dirs.push(ScannedDir {
                rel_path: child.clone(),
                mtime_ns: mtime_ns(&meta),
            });
            walk(&path, &child, scan);
        } else if ft.is_file() {
            scan.total_size += meta.len();
            scan.files.push(ScannedFile {
                rel_path: child,
                src: path,
                size: meta.len(),
                mtime_ns: mtime_ns(&meta),
            });
        } else {
            scan.warnings
                .push(format!("skipped {} (not a regular file)", path.display()));
        }
    }
}

/// Hashes of one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHashes {
    pub root: blake3::Hash,
    /// Concatenated block hashes, [`HASH_LEN`] bytes each.
    pub blocks: Vec<u8>,
}

impl FileHashes {
    /// Hash of the block list, sent in the offer so the receiver can trust
    /// the list it downloads.
    pub fn blocks_hash(&self) -> blake3::Hash {
        blake3::hash(&self.blocks)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HashError {
    #[error("could not read {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error(
        "{0} changed while it was being read; send it again once it is no longer being written"
    )]
    Changed(PathBuf),
    #[error("cancelled")]
    Cancelled,
}

/// Hash of one block.
pub fn hash_block(data: &[u8]) -> [u8; HASH_LEN] {
    let mut h = blake3::Hasher::new();
    if data.len() >= 128 * 1024 {
        h.update_rayon(data);
    } else {
        h.update(data);
    }
    *h.finalize().as_bytes()
}

/// Hashes a file in one streaming pass: root hash and, if `with_blocks`,
/// the block list. `progress` is called with bytes read so far.
pub fn hash_file(
    path: &Path,
    expected_size: u64,
    with_blocks: bool,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> Result<FileHashes, HashError> {
    let io_err = |source| HashError::Io {
        path: path.to_path_buf(),
        source,
    };
    let mut file = File::open(crate::paths::for_io(path)).map_err(io_err)?;
    let mut root = blake3::Hasher::new();
    let mut blocks = Vec::with_capacity(if with_blocks {
        block_count(expected_size) as usize * HASH_LEN
    } else {
        0
    });
    let mut buf = vec![0u8; BLOCK_SIZE as usize];
    let mut total = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(HashError::Cancelled);
        }
        let n = read_full(&mut file, &mut buf).map_err(io_err)?;
        if n == 0 {
            break;
        }
        let chunk = &buf[..n];
        root.update_rayon(chunk);
        if with_blocks {
            blocks.extend_from_slice(&hash_block(chunk));
        }
        total += n as u64;
        if total > expected_size {
            return Err(HashError::Changed(path.to_path_buf()));
        }
        progress(total);
        if n < buf.len() {
            break;
        }
    }
    if total != expected_size {
        return Err(HashError::Changed(path.to_path_buf()));
    }
    Ok(FileHashes {
        root: root.finalize(),
        blocks,
    })
}

/// Reads until `buf` is full or end of file.
pub fn read_full(r: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_math() {
        assert_eq!(block_count(0), 0);
        assert_eq!(block_count(1), 1);
        assert_eq!(block_count(BLOCK_SIZE), 1);
        assert_eq!(block_count(BLOCK_SIZE + 1), 2);
        assert_eq!(block_len(BLOCK_SIZE + 1, 1), 1);
        assert_eq!(block_len(BLOCK_SIZE + 1, 0), BLOCK_SIZE);
        assert_eq!(block_len(10, 5), 0);
    }

    #[test]
    fn scan_tree() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("talks");
        std::fs::create_dir_all(root.join("day1")).unwrap();
        std::fs::create_dir_all(root.join("empty")).unwrap();
        std::fs::write(root.join("day1/a.mp4"), b"aaaa").unwrap();
        std::fs::write(root.join("b.txt"), b"bb").unwrap();
        let single = dir.path().join("x.bin");
        std::fs::write(&single, b"x").unwrap();

        let s = scan(&[root.clone(), single]).unwrap();
        assert_eq!(s.roots, vec!["talks", "x.bin"]);
        assert_eq!(s.total_size, 7);
        let files: Vec<_> = s.files.iter().map(|f| f.rel_path.as_str()).collect();
        assert_eq!(files, vec!["talks/b.txt", "talks/day1/a.mp4", "x.bin"]);
        let dirs: Vec<_> = s.dirs.iter().map(|d| d.rel_path.as_str()).collect();
        assert_eq!(dirs, vec!["talks", "talks/day1", "talks/empty"]);
        assert_eq!(s.label(), "talks/ + 1 more");
    }

    #[test]
    fn duplicate_root_names_get_counter() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a")).unwrap();
        std::fs::create_dir_all(dir.path().join("b")).unwrap();
        std::fs::write(dir.path().join("a/t.mp4"), b"1").unwrap();
        std::fs::write(dir.path().join("b/t.mp4"), b"2").unwrap();
        let s = scan(&[dir.path().join("a/t.mp4"), dir.path().join("b/t.mp4")]).unwrap();
        assert_eq!(s.roots, vec!["t.mp4", "t (1).mp4"]);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("r");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("real"), b"1").unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("link")).unwrap();
        let s = scan(&[root]).unwrap();
        assert_eq!(s.files.len(), 1);
        assert_eq!(s.warnings.len(), 1);
        assert!(s.warnings[0].contains("symlink"));
    }

    #[test]
    fn missing_source() {
        assert!(matches!(
            scan(&[PathBuf::from("/definitely/not/here")]),
            Err(ScanError::NotFound(_))
        ));
    }

    #[test]
    fn hashes_match_plain_blake3() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("f");
        let data: Vec<u8> = (0..(BLOCK_SIZE as usize + 1000))
            .map(|i| (i % 251) as u8)
            .collect();
        std::fs::write(&f, &data).unwrap();
        let cancel = AtomicBool::new(false);
        let h = hash_file(&f, data.len() as u64, true, &cancel, |_| {}).unwrap();
        assert_eq!(h.root, blake3::hash(&data));
        assert_eq!(h.blocks.len(), 2 * HASH_LEN);
        assert_eq!(
            &h.blocks[..HASH_LEN],
            &hash_block(&data[..BLOCK_SIZE as usize])
        );
        assert_eq!(
            &h.blocks[HASH_LEN..],
            &hash_block(&data[BLOCK_SIZE as usize..])
        );
    }

    #[test]
    fn size_change_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("f");
        std::fs::write(&f, b"12345").unwrap();
        let cancel = AtomicBool::new(false);
        assert!(matches!(
            hash_file(&f, 4, true, &cancel, |_| {}),
            Err(HashError::Changed(_))
        ));
    }

    #[test]
    fn empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("f");
        std::fs::write(&f, b"").unwrap();
        let cancel = AtomicBool::new(false);
        let h = hash_file(&f, 0, true, &cancel, |_| {}).unwrap();
        assert_eq!(h.root, blake3::hash(b""));
        assert!(h.blocks.is_empty());
    }
}
