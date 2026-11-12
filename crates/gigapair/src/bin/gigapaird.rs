//! `gigapaird`: the background service.

// No console window for the service in release builds on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() -> anyhow::Result<()> {
    gigapaird::main()
}
