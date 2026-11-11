//! Picking a final name that never overwrites an existing file.

use std::path::{Path, PathBuf};

use super::split_extension;

/// Returns `dir/name`, or `dir/name (N).ext` with the lowest free `N` if
/// something already exists there.
///
/// Names that already carry a counter continue it: if `talk (1).mp4` is
/// taken, the next candidate is `talk (2).mp4`, not `talk (1) (1).mp4`.
///
/// `exists` is injected so the logic is testable without touching disk;
/// use [`unique_destination`] for the real file system.
///
/// This only picks a name. Between this check and the final rename another
/// process could create the same file, so the rename itself must refuse to
/// replace an existing file.
pub fn unique_name(dir: &Path, name: &str, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let candidate = dir.join(name);
    if !exists(&candidate) {
        return candidate;
    }

    let (stem, ext) = split_extension(name);
    let (base, start) = match parse_counter(stem) {
        Some((base, n)) => (base, n + 1),
        None => (stem, 1),
    };

    let mut n = start;
    loop {
        let candidate = dir.join(format!("{base} ({n}){ext}"));
        if !exists(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// [`unique_name`] against the real file system.
///
/// Uses `symlink_metadata` so a broken symlink still counts as taken
/// (`Path::exists` follows the link and would report it as free).
pub fn unique_destination(dir: &Path, name: &str) -> PathBuf {
    unique_name(dir, name, |p| p.symlink_metadata().is_ok())
}

/// Splits `"talk (3)"` into `("talk", 3)`.
fn parse_counter(stem: &str) -> Option<(&str, u64)> {
    let inner = stem.strip_suffix(')')?;
    let open = inner.rfind(" (")?;
    let digits = &inner[open + 2..];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n = digits.parse().ok()?;
    Some((&inner[..open], n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn taken(names: &[&str]) -> impl Fn(&Path) -> bool {
        let set: HashSet<PathBuf> = names.iter().map(|n| Path::new("dest").join(n)).collect();
        move |p| set.contains(p)
    }

    fn pick(name: &str, existing: &[&str]) -> PathBuf {
        unique_name(Path::new("dest"), name, taken(existing))
    }

    #[test]
    fn free_name_is_kept() {
        assert_eq!(pick("talk.mp4", &[]), Path::new("dest/talk.mp4"));
    }

    #[test]
    fn taken_name_gets_counter_before_extension() {
        assert_eq!(
            pick("talk.mp4", &["talk.mp4"]),
            Path::new("dest/talk (1).mp4")
        );
    }

    #[test]
    fn counter_skips_taken_numbers() {
        let existing = ["talk.mp4", "talk (1).mp4", "talk (2).mp4"];
        assert_eq!(pick("talk.mp4", &existing), Path::new("dest/talk (3).mp4"));
    }

    #[test]
    fn existing_counter_is_continued() {
        assert_eq!(
            pick("talk (1).mp4", &["talk (1).mp4"]),
            Path::new("dest/talk (2).mp4")
        );
    }

    #[test]
    fn folders_and_dotfiles_get_counter_at_end() {
        assert_eq!(pick("talks", &["talks"]), Path::new("dest/talks (1)"));
        assert_eq!(pick(".env", &[".env"]), Path::new("dest/.env (1)"));
    }

    #[test]
    fn non_counter_parentheses_are_kept() {
        assert_eq!(
            pick("talk (final).mp4", &["talk (final).mp4"]),
            Path::new("dest/talk (final) (1).mp4")
        );
    }

    #[test]
    fn parse_counter_cases() {
        assert_eq!(parse_counter("talk (3)"), Some(("talk", 3)));
        assert_eq!(parse_counter("talk (final)"), None);
        assert_eq!(parse_counter("talk ()"), None);
        assert_eq!(parse_counter("talk(3)"), None);
        assert_eq!(parse_counter("talk"), None);
    }

    #[test]
    fn real_file_system_detects_existing_file() {
        let dir = std::env::temp_dir().join(format!("gigapair-collision-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("talk.mp4"), b"x").unwrap();

        let picked = unique_destination(&dir, "talk.mp4");
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(picked, dir.join("talk (1).mp4"));
    }
}
