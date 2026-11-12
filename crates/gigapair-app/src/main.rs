//! Gigapair desktop app (GPUI). A thin client of the `gigapaird` service:
//! it starts the service if needed, and closing the window never stops a
//! transfer. With a tray icon available, the app keeps running in the tray
//! when the window closes.
//!
//! Options: `--background` starts in the tray without opening the window;
//! `--verbose` also logs to the terminal (always on in debug builds). The
//! log file is `gigapair-app.YYYY-MM-DD.log` in the logs folder.

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
    let args: Vec<String> = std::env::args().skip(1).collect();
    let background = args.iter().any(|a| a == "--background");
    let verbose = cfg!(debug_assertions) || args.iter().any(|a| a == "--verbose");
    let dirs = match AppDirs::from_env() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = dirs.create_all() {
        eprintln!("warning: could not create {}: {e}", dirs.logs.display());
    }
    // Flushes the log file when main returns.
    let _log_guard = gigapair_core::logging::init(&dirs, "gigapair-app", verbose);
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        os = std::env::consts::OS,
        background,
        logs = %dirs.logs.display(),
        "Gigapair app starting"
    );

    let primary = match single::claim(&dirs) {
        single::Instance::Primary(p) => p,
        single::Instance::Secondary => {
            tracing::info!("another Gigapair app is running; asked it to show its window");
            return;
        }
    };

    let app = gpui_kit::application().with_assets(gpui_kit::assets::Assets);
    // macOS: clicking the Dock icon while only the tray is left.
    app.on_reopen(shell::show_window);
    app.run(move |cx: &mut App| {
        gpui_kit::init(cx);
        shell::start(dirs, primary, background, cx);
    });
    tracing::info!("Gigapair app closed");
}
