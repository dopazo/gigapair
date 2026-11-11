//! Long path support on Windows.
//!
//! Classic Windows APIs fail on paths longer than 260 characters
//! (`MAX_PATH`), which deep folder trees easily exceed. Prefixing an absolute
//! path with `\\?\` ("extended-length path") lifts the limit to ~32,767
//! characters without depending on the system-wide `LongPathsEnabled`
//! setting.

use std::path::{Path, PathBuf};

/// Converts an absolute Windows path to extended-length form.
///
/// - `C:\a\b` → `\\?\C:\a\b`
/// - `\\server\share\a` → `\\?\UNC\server\share\a`
/// - already prefixed (`\\?\…`) or device paths (`\\.\…`) are unchanged
/// - relative paths are unchanged (the prefix only works on absolute paths)
///
/// Windows does not normalize extended-length paths, so `/` is converted to
/// `\`. The input must not contain `.` or `..` components.
pub fn to_extended_length(path: &str) -> String {
    if path.starts_with(r"\\?\") || path.starts_with(r"\\.\") {
        return path.to_owned();
    }

    let path = path.replace('/', r"\");
    if let Some(unc) = path.strip_prefix(r"\\") {
        format!(r"\\?\UNC\{unc}")
    } else if is_drive_absolute(&path) {
        format!(r"\\?\{path}")
    } else {
        path
    }
}

fn is_drive_absolute(path: &str) -> bool {
    let b = path.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\'
}

/// The form of `path` to hand to file system calls: extended-length on
/// Windows, unchanged elsewhere.
pub fn for_io(path: &Path) -> PathBuf {
    if cfg!(windows) {
        if let Some(s) = path.to_str() {
            return PathBuf::from(to_extended_length(s));
        }
    }
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_paths_are_prefixed() {
        assert_eq!(
            to_extended_length(r"C:\Users\me\file.mp4"),
            r"\\?\C:\Users\me\file.mp4"
        );
        assert_eq!(
            to_extended_length("d:/videos/a.mp4"),
            r"\\?\d:\videos\a.mp4"
        );
    }

    #[test]
    fn unc_paths_use_unc_prefix() {
        assert_eq!(
            to_extended_length(r"\\nas\share\a.mp4"),
            r"\\?\UNC\nas\share\a.mp4"
        );
    }

    #[test]
    fn already_prefixed_and_device_paths_are_unchanged() {
        assert_eq!(to_extended_length(r"\\?\C:\a"), r"\\?\C:\a");
        assert_eq!(
            to_extended_length(r"\\.\pipe\gigapair"),
            r"\\.\pipe\gigapair"
        );
    }

    #[test]
    fn relative_paths_are_unchanged() {
        assert_eq!(to_extended_length(r"talks\a.mp4"), r"talks\a.mp4");
        assert_eq!(to_extended_length("C:relative"), "C:relative");
    }

    #[cfg(windows)]
    #[test]
    fn can_create_files_beyond_max_path() {
        let base = std::env::temp_dir().join(format!("gigapair-long-{}", std::process::id()));
        let mut deep = base.clone();
        for _ in 0..12 {
            deep.push("a".repeat(30));
        }
        assert!(deep.as_os_str().len() > 260);

        std::fs::create_dir_all(for_io(&deep)).unwrap();
        std::fs::write(for_io(&deep.join("file.bin")), b"x").unwrap();
        assert!(for_io(&deep.join("file.bin")).exists());
        std::fs::remove_dir_all(for_io(&base)).unwrap();
    }
}
