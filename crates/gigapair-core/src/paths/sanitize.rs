//! Making names received from a peer safe to create on this system.

use std::fmt;
use std::path::PathBuf;

use super::split_extension;

/// Which file system naming rules to apply.
///
/// Usually [`Platform::current`]; tests can target either platform from any
/// host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Unix,
}

impl Platform {
    /// The rules of the system this binary runs on.
    pub const fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }

    fn is_invalid_char(self, c: char) -> bool {
        match self {
            Self::Unix => c == '/' || c == '\0',
            Self::Windows => (c as u32) < 0x20 || r#"<>:"/\|?*"#.contains(c),
        }
    }

    /// Length as the file system counts it: UTF-16 units on Windows (NTFS),
    /// bytes on Unix (ext4, APFS).
    fn char_len(self, c: char) -> usize {
        match self {
            Self::Windows => c.len_utf16(),
            Self::Unix => c.len_utf8(),
        }
    }

    fn len(self, s: &str) -> usize {
        s.chars().map(|c| self.char_len(c)).sum()
    }
}

const REPLACEMENT: char = '_';

/// Maximum length of a single path component on all supported file systems.
const MAX_COMPONENT_LEN: usize = 255;

/// Extensions longer than this are not worth preserving when truncating.
const MAX_KEPT_EXTENSION_LEN: usize = 32;

/// Device names Windows reserves, with or without an extension (`nul.txt`
/// is reserved too).
const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "COM1", "COM2", "COM3", "COM4", "COM5",
    "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8",
    "LPT9",
];

/// Turns one file or folder name into a valid name on `platform`.
///
/// Invalid characters become `_`, Windows reserved names get a `_` prefix
/// and overlong names are truncated, keeping the extension. The result is
/// never empty, `.` or `..`.
///
/// Different inputs can map to the same output (`a:b` and `a_b` on
/// Windows); callers resolve that with [`super::unique_name`].
pub fn sanitize_component(name: &str, platform: Platform) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if platform.is_invalid_char(c) {
                REPLACEMENT
            } else {
                c
            }
        })
        .collect();

    if platform == Platform::Windows {
        // Windows silently drops trailing dots and spaces; do it explicitly
        // so the name we check for collisions is the name that gets created.
        out.truncate(out.trim_end_matches(['.', ' ']).len());

        let stem = out
            .split('.')
            .next()
            .unwrap_or_default()
            .trim_end_matches(' ');
        if WINDOWS_RESERVED
            .iter()
            .any(|r| stem.eq_ignore_ascii_case(r))
        {
            out.insert(0, REPLACEMENT);
        }
    }

    if out.is_empty() || out == "." || out == ".." {
        return REPLACEMENT.to_string();
    }

    truncate(out, platform)
}

fn truncate(name: String, platform: Platform) -> String {
    if platform.len(&name) <= MAX_COMPONENT_LEN {
        return name;
    }

    let (stem, ext) = split_extension(&name);
    let ext = if platform.len(ext) <= MAX_KEPT_EXTENSION_LEN {
        ext
    } else {
        ""
    };
    let budget = MAX_COMPONENT_LEN - platform.len(ext);

    let mut out = String::with_capacity(name.len());
    let mut used = 0;
    for c in stem.chars() {
        let w = platform.char_len(c);
        if used + w > budget {
            break;
        }
        used += w;
        out.push(c);
    }
    out.push_str(ext);

    if platform == Platform::Windows {
        out.truncate(out.trim_end_matches(['.', ' ']).len());
    }
    out
}

/// Why a path received from a peer was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    Empty,
    Absolute(String),
    Traversal(String),
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "the sender provided an empty file path"),
            Self::Absolute(p) => {
                write!(
                    f,
                    "the sender provided an absolute path ({p}); only relative paths are accepted"
                )
            }
            Self::Traversal(p) => {
                write!(
                    f,
                    "the sender provided a path that leaves the destination folder ({p})"
                )
            }
        }
    }
}

impl std::error::Error for PathError {}

/// Converts a path received from a peer into a relative path that is safe
/// to join onto the destination folder.
///
/// On the wire, paths are relative and always `/`-separated
/// (`talks/day 1/keynote.mp4`). Absolute paths and `..` are rejected, so a
/// malicious peer can't write outside the destination folder. Empty and `.`
/// components are ignored and every other component is sanitized with
/// [`sanitize_component`].
pub fn safe_relative_path(wire: &str, platform: Platform) -> Result<PathBuf, PathError> {
    if wire.starts_with('/') {
        return Err(PathError::Absolute(wire.to_owned()));
    }

    let mut out = PathBuf::new();
    for component in wire.split('/') {
        match component {
            "" | "." => {}
            ".." => return Err(PathError::Traversal(wire.to_owned())),
            name => out.push(sanitize_component(name, platform)),
        }
    }

    if out.as_os_str().is_empty() {
        return Err(PathError::Empty);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use Platform::{Unix, Windows};

    #[test]
    fn valid_names_are_unchanged() {
        for name in [
            "keynote.mp4",
            "day 1",
            ".hidden",
            "año 2026 (final).mov",
            "a.b.c",
        ] {
            assert_eq!(sanitize_component(name, Windows), name);
            assert_eq!(sanitize_component(name, Unix), name);
        }
    }

    #[test]
    fn windows_invalid_chars_are_replaced() {
        assert_eq!(
            sanitize_component("10:30 talk.mp4", Windows),
            "10_30 talk.mp4"
        );
        assert_eq!(
            sanitize_component(r#"a<b>c"d|e?f*g\h"#, Windows),
            "a_b_c_d_e_f_g_h"
        );
        assert_eq!(sanitize_component("tab\there", Windows), "tab_here");
    }

    #[test]
    fn unix_allows_windows_invalid_chars() {
        assert_eq!(sanitize_component("10:30 talk.mp4", Unix), "10:30 talk.mp4");
        assert_eq!(sanitize_component(r"a\b", Unix), r"a\b");
        assert_eq!(sanitize_component("nul\0byte", Unix), "nul_byte");
    }

    #[test]
    fn windows_trailing_dots_and_spaces_are_removed() {
        assert_eq!(sanitize_component("draft. ", Windows), "draft");
        assert_eq!(sanitize_component("draft.", Unix), "draft.");
        assert_eq!(sanitize_component("...", Windows), "_");
    }

    #[test]
    fn windows_reserved_names_are_prefixed() {
        assert_eq!(sanitize_component("CON", Windows), "_CON");
        assert_eq!(sanitize_component("nul.txt", Windows), "_nul.txt");
        assert_eq!(sanitize_component("Com1.tar.gz", Windows), "_Com1.tar.gz");
        assert_eq!(sanitize_component("console.txt", Windows), "console.txt");
        assert_eq!(sanitize_component("COM10", Windows), "COM10");
        assert_eq!(sanitize_component("CON", Unix), "CON");
    }

    #[test]
    fn never_empty_or_dot() {
        for p in [Windows, Unix] {
            assert_eq!(sanitize_component("", p), "_");
            assert_eq!(sanitize_component(".", p), "_");
            assert_eq!(sanitize_component("..", p), "_");
        }
    }

    #[test]
    fn long_names_are_truncated_keeping_extension() {
        let name = format!("{}.mp4", "a".repeat(300));
        for p in [Windows, Unix] {
            let out = sanitize_component(&name, p);
            assert_eq!(out.len(), MAX_COMPONENT_LEN);
            assert!(out.ends_with(".mp4"));
        }
    }

    #[test]
    fn truncation_counts_bytes_on_unix_and_utf16_on_windows() {
        // 'ñ' is 2 bytes in UTF-8 but 1 unit in UTF-16.
        let name = "ñ".repeat(200);
        assert_eq!(sanitize_component(&name, Windows), name);
        let unix = sanitize_component(&name, Unix);
        assert_eq!(unix.len(), 254);
        assert_eq!(unix.chars().count(), 127);
    }

    #[test]
    fn long_extensions_are_dropped_when_truncating() {
        let name = format!("{}.{}", "a".repeat(250), "x".repeat(40));
        let out = sanitize_component(&name, Unix);
        assert_eq!(out, "a".repeat(250));
    }

    #[test]
    fn relative_paths_are_sanitized_per_component() {
        let path = safe_relative_path("talks/10:30/keynote.mp4", Windows).unwrap();
        assert_eq!(path, PathBuf::from_iter(["talks", "10_30", "keynote.mp4"]));
    }

    #[test]
    fn empty_and_dot_components_are_ignored() {
        let path = safe_relative_path("./talks//keynote.mp4/", Unix).unwrap();
        assert_eq!(path, PathBuf::from_iter(["talks", "keynote.mp4"]));
    }

    #[test]
    fn absolute_paths_are_rejected() {
        assert!(matches!(
            safe_relative_path("/etc/passwd", Unix),
            Err(PathError::Absolute(_))
        ));
    }

    #[test]
    fn traversal_is_rejected() {
        for wire in ["../outside", "talks/../../outside", ".."] {
            assert!(matches!(
                safe_relative_path(wire, Unix),
                Err(PathError::Traversal(_))
            ));
        }
    }

    #[test]
    fn backslash_traversal_cannot_escape_on_windows() {
        // On Windows `\` is a separator, so it must not survive sanitizing.
        let path = safe_relative_path(r"..\..\outside", Windows).unwrap();
        assert_eq!(path, PathBuf::from(".._.._outside"));
    }

    #[test]
    fn drive_paths_cannot_escape_on_windows() {
        let path = safe_relative_path(r"C:\Windows\evil.dll", Windows).unwrap();
        assert_eq!(path, PathBuf::from("C__Windows_evil.dll"));
    }

    #[test]
    fn empty_paths_are_rejected() {
        for wire in ["", "/", ".", "./", "//"] {
            assert!(
                safe_relative_path(wire, Unix).is_err(),
                "{wire:?} should be rejected"
            );
        }
    }
}
