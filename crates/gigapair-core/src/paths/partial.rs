//! Layout of the in-progress area.
//!
//! Incoming data is written to `.gigapair-partial/<transfer id>/` inside the
//! destination folder. Being on the same file system as the final location
//! lets the finished file be moved into place with a rename instead of a
//! copy, so a 200 GB file never needs 400 GB. Until then, nothing appears
//! under its final name, so nobody opens an incomplete file.

use std::path::{Path, PathBuf};

/// Name of the in-progress folder inside each destination folder.
///
/// The leading dot hides it on macOS and Linux. Windows ignores the dot, so
/// the folder also needs the hidden attribute when it is created.
pub const PARTIAL_DIR_NAME: &str = ".gigapair-partial";

/// `dest_dir/.gigapair-partial`.
pub fn partial_dir(dest_dir: &Path) -> PathBuf {
    dest_dir.join(PARTIAL_DIR_NAME)
}

/// `dest_dir/.gigapair-partial/<transfer_id>`.
///
/// `transfer_id` is generated locally (never received from a peer) and must
/// be a single, plain path component.
pub fn transfer_partial_dir(dest_dir: &Path, transfer_id: &str) -> PathBuf {
    debug_assert!(
        !transfer_id.is_empty()
            && transfer_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-'),
        "transfer id must be a plain path component: {transfer_id:?}"
    );
    partial_dir(dest_dir).join(transfer_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout() {
        let dest = Path::new("downloads");
        assert_eq!(partial_dir(dest), Path::new("downloads/.gigapair-partial"));
        assert_eq!(
            transfer_partial_dir(dest, "3f9a-01"),
            Path::new("downloads/.gigapair-partial/3f9a-01")
        );
    }

    #[test]
    #[should_panic(expected = "plain path component")]
    #[cfg(debug_assertions)]
    fn rejects_traversal_in_transfer_id() {
        transfer_partial_dir(Path::new("downloads"), "../x");
    }
}
