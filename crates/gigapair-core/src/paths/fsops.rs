//! File system operations the receive path depends on: a rename that never
//! replaces an existing file, hiding the in-progress folder on Windows,
//! free space, and modification times.

use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::{for_io, partial_dir, unique_destination};

/// Renames `from` to `to`, failing with [`io::ErrorKind::AlreadyExists`] if
/// `to` exists. Both paths must be on the same file system.
///
/// `std::fs::rename` silently replaces the target on every OS, which would
/// break the "never overwrite" rule if another process created the file
/// between picking a name and renaming.
pub fn rename_noclobber(from: &Path, to: &Path) -> io::Result<()> {
    imp::rename_noclobber(&for_io(from), &for_io(to))
}

/// Moves `from` into `dir` under `name`, or `name (N).ext` if taken, and
/// returns the final path. Retries when another process wins the race for
/// a name.
pub fn move_into_unique(from: &Path, dir: &Path, name: &str) -> io::Result<PathBuf> {
    // Bounded so a pathological directory can't spin forever.
    for _ in 0..10_000 {
        let target = unique_destination(dir, name);
        match rename_noclobber(from, &target) {
            Ok(()) => return Ok(target),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other(format!(
        "could not find a free name for {name:?} in {}",
        dir.display()
    )))
}

/// Creates a new directory `dir/name`, or `dir/name (N)` if taken, and
/// returns it. `create_dir` fails if the directory exists, so this never
/// merges into an existing folder.
pub fn create_unique_dir(dir: &Path, name: &str) -> io::Result<PathBuf> {
    for _ in 0..10_000 {
        let target = unique_destination(dir, name);
        match std::fs::create_dir(for_io(&target)) {
            Ok(()) => return Ok(target),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other(format!(
        "could not find a free name for {name:?} in {}",
        dir.display()
    )))
}

/// Creates `dest_dir/.gigapair-partial` (and `dest_dir`) if needed and
/// returns it. On Windows the folder gets the hidden attribute, since the
/// leading dot doesn't hide it there.
pub fn ensure_partial_dir(dest_dir: &Path) -> io::Result<PathBuf> {
    let dir = partial_dir(dest_dir);
    std::fs::create_dir_all(for_io(&dir))?;
    imp::set_hidden(&for_io(&dir))?;
    Ok(dir)
}

/// Bytes available to this user on the file system holding `path`.
///
/// `path` may not exist yet: the nearest existing ancestor is used.
pub fn available_space(path: &Path) -> io::Result<u64> {
    let mut probe = path;
    loop {
        if probe.exists() {
            return imp::available_space(&for_io(probe));
        }
        match probe.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => probe = parent,
            _ => return imp::available_space(Path::new(".")),
        }
    }
}

/// Sets the modification time of a file or directory.
pub fn set_mtime(path: &Path, mtime: SystemTime) -> io::Result<()> {
    let ft = filetime::FileTime::from_system_time(mtime);
    filetime::set_file_mtime(for_io(path), ft)
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    pub fn rename_noclobber(from: &Path, to: &Path) -> io::Result<()> {
        let from_c = CString::new(from.as_os_str().as_bytes())?;
        let to_c = CString::new(to.as_os_str().as_bytes())?;
        // SAFETY: both pointers are valid NUL-terminated strings for the
        // duration of the call.
        let rc = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                from_c.as_ptr(),
                libc::AT_FDCWD,
                to_c.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if rc == 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        match err.raw_os_error() {
            // Old kernels or file systems without RENAME_NOREPLACE.
            Some(libc::EINVAL) | Some(libc::ENOSYS) | Some(libc::EOPNOTSUPP) => {
                super::link_then_unlink(from, to)
            }
            _ => Err(err),
        }
    }

    pub fn set_hidden(_path: &Path) -> io::Result<()> {
        Ok(())
    }

    pub fn available_space(path: &Path) -> io::Result<u64> {
        super::statvfs_available(path)
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    pub fn rename_noclobber(from: &Path, to: &Path) -> io::Result<()> {
        let from_c = CString::new(from.as_os_str().as_bytes())?;
        let to_c = CString::new(to.as_os_str().as_bytes())?;
        // SAFETY: both pointers are valid NUL-terminated strings for the
        // duration of the call.
        let rc = unsafe { libc::renamex_np(from_c.as_ptr(), to_c.as_ptr(), libc::RENAME_EXCL) };
        if rc == 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::ENOTSUP) | Some(libc::EINVAL) => super::link_then_unlink(from, to),
            _ => Err(err),
        }
    }

    pub fn set_hidden(_path: &Path) -> io::Result<()> {
        Ok(())
    }

    pub fn available_space(path: &Path) -> io::Result<u64> {
        super::statvfs_available(path)
    }
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
mod imp {
    use super::*;

    pub fn rename_noclobber(from: &Path, to: &Path) -> io::Result<()> {
        super::link_then_unlink(from, to)
    }

    pub fn set_hidden(_path: &Path) -> io::Result<()> {
        Ok(())
    }

    pub fn available_space(path: &Path) -> io::Result<u64> {
        super::statvfs_available(path)
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_HIDDEN, GetDiskFreeSpaceExW, GetFileAttributesW, INVALID_FILE_ATTRIBUTES,
        MOVEFILE_WRITE_THROUGH, MoveFileExW, SetFileAttributesW,
    };

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    pub fn rename_noclobber(from: &Path, to: &Path) -> io::Result<()> {
        let (from_w, to_w) = (wide(from), wide(to));
        // Without MOVEFILE_REPLACE_EXISTING, MoveFileExW fails if the target
        // exists. Same-volume moves are a rename (no copy flag given).
        // SAFETY: both buffers are NUL-terminated UTF-16 and outlive the call.
        let ok = unsafe { MoveFileExW(from_w.as_ptr(), to_w.as_ptr(), MOVEFILE_WRITE_THROUGH) };
        if ok != 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        match err.raw_os_error().map(|c| c as u32) {
            Some(ERROR_ALREADY_EXISTS) | Some(ERROR_FILE_EXISTS) => Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} already exists", to.display()),
            )),
            _ => Err(err),
        }
    }

    pub fn set_hidden(path: &Path) -> io::Result<()> {
        let w = wide(path);
        // SAFETY: `w` is NUL-terminated UTF-16 and outlives both calls.
        unsafe {
            let attrs = GetFileAttributesW(w.as_ptr());
            if attrs == INVALID_FILE_ATTRIBUTES {
                return Err(io::Error::last_os_error());
            }
            if attrs & FILE_ATTRIBUTE_HIDDEN == 0
                && SetFileAttributesW(w.as_ptr(), attrs | FILE_ATTRIBUTE_HIDDEN) == 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    pub fn available_space(path: &Path) -> io::Result<u64> {
        let w = wide(path);
        let mut free: u64 = 0;
        // SAFETY: `w` is NUL-terminated; the out pointer is valid; the other
        // out parameters are optional and passed as null.
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                w.as_ptr(),
                &mut free,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(free)
    }
}

/// No-clobber rename for file systems without a native flag: a hard link
/// fails if the target exists, then the source name is removed.
#[cfg(unix)]
fn link_then_unlink(from: &Path, to: &Path) -> io::Result<()> {
    match std::fs::hard_link(from, to) {
        Ok(()) => std::fs::remove_file(from),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(e),
        // Directories can't be hard-linked, and some file systems (FAT,
        // exFAT) don't support links: fall back to a checked rename. This
        // has a small race window, but those file systems offer nothing
        // better.
        Err(_) => {
            if to.symlink_metadata().is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("{} already exists", to.display()),
                ));
            }
            std::fs::rename(from, to)
        }
    }
}

#[cfg(unix)]
fn statvfs_available(path: &Path) -> io::Result<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c = CString::new(path.as_os_str().as_bytes())?;
    // SAFETY: zeroed statvfs is a valid out buffer; `c` is NUL-terminated.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c.as_ptr(), &mut st) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    #[allow(clippy::unnecessary_cast)]
    Ok(st.f_bavail as u64 * st.f_frsize as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_refuses_to_replace() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::write(&a, b"new").unwrap();
        std::fs::write(&b, b"old").unwrap();

        let err = rename_noclobber(&a, &b).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&b).unwrap(), b"old");
        assert!(a.exists());
    }

    #[test]
    fn rename_to_free_name() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::write(&a, b"data").unwrap();
        rename_noclobber(&a, &b).unwrap();
        assert!(!a.exists());
        assert_eq!(std::fs::read(&b).unwrap(), b"data");
    }

    #[test]
    fn move_into_unique_adds_counter() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("talk.mp4"), b"old").unwrap();
        let src = dir.path().join("incoming");
        std::fs::write(&src, b"new").unwrap();

        let placed = move_into_unique(&src, dir.path(), "talk.mp4").unwrap();
        assert_eq!(placed, dir.path().join("talk (1).mp4"));
        assert_eq!(std::fs::read(dir.path().join("talk.mp4")).unwrap(), b"old");
        assert_eq!(std::fs::read(placed).unwrap(), b"new");
    }

    #[test]
    fn unique_dir_never_merges() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("talks")).unwrap();
        let made = create_unique_dir(dir.path(), "talks").unwrap();
        assert_eq!(made, dir.path().join("talks (1)"));
    }

    #[test]
    fn partial_dir_is_created() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("incoming");
        let p = ensure_partial_dir(&dest).unwrap();
        assert!(p.is_dir());
        // Idempotent.
        ensure_partial_dir(&dest).unwrap();
    }

    #[test]
    fn free_space_of_missing_path_uses_ancestor() {
        let dir = tempfile::tempdir().unwrap();
        let space = available_space(&dir.path().join("not/yet/here")).unwrap();
        assert!(space > 0);
    }

    #[test]
    fn mtime_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("f");
        std::fs::write(&f, b"x").unwrap();
        let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        set_mtime(&f, t).unwrap();
        assert_eq!(std::fs::metadata(&f).unwrap().modified().unwrap(), t);
    }
}
