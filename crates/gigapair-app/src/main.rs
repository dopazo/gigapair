//! Gigapair desktop app (GPUI). A thin client of the `gigapaird` service:
//! it starts the service if needed, and closing the window never stops a
//! transfer.

// No console window in release builds on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod backend;
mod qr;
mod strings;

use gigapair_core::dirs::AppDirs;
use gpui_kit::component::Theme;
use gpui_kit::{App, AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};

fn main() {
    let dirs = match AppDirs::from_env() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx: &mut App| {
            gpui_kit::init(cx);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let bounds = Bounds::centered(None, size(px(1040.), px(700.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(strings::APP_NAME.into()),
                    ..Default::default()
                }),
                app_id: Some("dev.gigapair.Gigapair".into()),
                ..Default::default()
            };
            let opened = gpui_kit::open_window(options, cx, |window, cx| {
                Theme::sync_system_appearance(Some(window), cx);
                cx.new(|cx| app::GigapairApp::new(dirs.clone(), window, cx))
            });
            if let Err(e) = opened {
                eprintln!("error: could not open the window: {e}");
                cx.quit();
            }
            cx.activate(true);
        });
}
