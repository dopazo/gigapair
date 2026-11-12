//! Gigapair desktop app (GPUI). A thin client of the `gigapaird` service:
//! it starts the service if needed, and closing the window never stops a
//! transfer. With a tray icon available, the app keeps running in the tray
//! when the window closes.
//!
//! Options: `--background` starts in the tray without opening the window.

// No console window in release builds on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod backend;
mod icon;
mod qr;
mod shell;
mod single;
mod strings;
mod tray;

use gigapair_core::dirs::AppDirs;
use gpui_kit::App;

fn main() {
    let background = std::env::args().skip(1).any(|a| a == "--background");
    let dirs = match AppDirs::from_env() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    let primary = match single::claim(&dirs) {
        single::Instance::Primary(p) => p,
        // The running app shows its window.
        single::Instance::Secondary => return,
    };

    let app = gpui_kit::application().with_assets(gpui_kit::assets::Assets);
    // macOS: clicking the Dock icon while only the tray is left.
    app.on_reopen(shell::show_window);
    app.run(move |cx: &mut App| {
        gpui_kit::init(cx);
        shell::start(dirs, primary, background, cx);
    });
}
