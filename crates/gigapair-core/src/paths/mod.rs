//! File name and path handling for received transfers.
//!
//! Everything a peer sends (file and folder names) is untrusted input. This
//! module turns it into paths that are safe and valid on this system, picks
//! non-clobbering final names, and lays out the in-progress area.

mod collision;
mod fsops;
mod long_path;
mod partial;
mod sanitize;

pub use collision::{unique_destination, unique_name};
pub use fsops::{
    available_space, create_unique_dir, ensure_partial_dir, move_into_unique, rename_noclobber,
    set_mtime,
};
pub use long_path::{for_io, to_extended_length};
pub use partial::{PARTIAL_DIR_NAME, partial_dir, transfer_partial_dir};
pub use sanitize::{PathError, Platform, safe_relative_path, sanitize_component};

/// Splits `name` into stem and extension (the extension keeps its dot).
///
/// A leading dot is not an extension: `.bashrc` has no extension. Only the
/// last extension is split: `archive.tar.gz` gives `("archive.tar", ".gz")`.
pub(crate) fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::split_extension;

    #[test]
    fn splits_last_extension() {
        assert_eq!(split_extension("talk.mp4"), ("talk", ".mp4"));
        assert_eq!(split_extension("archive.tar.gz"), ("archive.tar", ".gz"));
    }

    #[test]
    fn no_extension() {
        assert_eq!(split_extension("README"), ("README", ""));
        assert_eq!(split_extension(".bashrc"), (".bashrc", ""));
    }
}
