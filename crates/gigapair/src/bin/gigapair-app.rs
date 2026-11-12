//! `gigapair-app`: the desktop app.

// No console window in release builds on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    gigapair_app::main()
}
