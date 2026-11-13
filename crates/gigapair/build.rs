//! Embeds the app icon in the Windows executables (`assets/gigapair.rc`).
//! `assets/gigapair.ico` is the G mark on its ink tile, drawn with the same
//! geometry as the tray icon (`crates/gigapair-app/src/icon.rs`), with the
//! block filled.

fn main() {
    println!("cargo:rerun-if-changed=assets/gigapair.rc");
    println!("cargo:rerun-if-changed=assets/gigapair.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/gigapair.rc", embed_resource::NONE)
            .manifest_optional()
            .unwrap_or_else(|e| panic!("could not embed the app icon: {e}"));
    }
}
