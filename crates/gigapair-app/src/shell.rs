//! Everything around the window: the one connection to the service, the
//! tray icon, and opening, closing and reopening the window. The app keeps
//! running in the tray when the window closes (unless the user turned that
//! off, or there is no tray), and quits from the tray's menu. Quitting the
//! app never stops the service, so transfers keep going.

use std::collections::HashMap;

use gigapair_core::api::{Event, Request, Response};
use gigapair_core::dirs::AppDirs;
use gigapair_core::types::TransferInfo;
use gpui_kit::component::Theme;
use gpui_kit::{
    AnyWindowHandle, App, AppContext, Bounds, Global, QuitMode, TitlebarOptions, WeakEntity,
    WindowBounds, WindowOptions, px, size,
};
use tokio::sync::mpsc;

use crate::app::GigapairApp;
use crate::backend::{Backend, BackendMsg};
use crate::single::Primary;
use crate::strings as s;
use crate::tray::{Tray, TrayAction, TrayView, summarize};

pub struct Shell {
    backend: Backend,
    connected: bool,
    /// The `close_to_tray` setting; on until the service says otherwise.
    close_to_tray: bool,
    tray: Option<Tray>,
    shown: Option<TrayView>,
    window: Option<(AnyWindowHandle, WeakEntity<GigapairApp>)>,
    /// Unfinished transfers, for the tray's summary.
    transfers: HashMap<String, TransferInfo>,
}

impl Global for Shell {}

impl Shell {
    pub fn has_tray(&self) -> bool {
        self.tray.is_some()
    }
}

/// Sets up the service connection, the tray and, unless `background`, the
/// window.
pub fn start(dirs: AppDirs, single: Primary, background: bool, cx: &mut App) {
    // The app decides when to quit, not the last window closing.
    cx.set_quit_mode(QuitMode::Explicit);

    let (actions, mut action_rx) = mpsc::unbounded_channel::<TrayAction>();
    if let Err(e) = single.listen(actions.clone()) {
        eprintln!("warning: {e}");
    }
    let tray = match Tray::new(actions) {
        Ok(t) => Some(t),
        Err(e) => {
            eprintln!("note: no tray icon ({e}); closing the window quits the app");
            None
        }
    };
    let (backend, mut backend_rx) = Backend::start(dirs);
    cx.set_global(Shell {
        backend,
        connected: false,
        close_to_tray: true,
        tray,
        shown: None,
        window: None,
        transfers: HashMap::new(),
    });
    refresh_tray(cx);

    cx.spawn(async move |cx| {
        while let Some(msg) = backend_rx.recv().await {
            cx.update(|cx| on_backend(msg, cx));
        }
    })
    .detach();

    cx.spawn(async move |cx| {
        while let Some(action) = action_rx.recv().await {
            cx.update(|cx| match action {
                TrayAction::Open => show_window(cx),
                TrayAction::Quit => cx.quit(),
            });
        }
    })
    .detach();

    cx.on_window_closed(|cx, _| {
        if !cx.windows().is_empty() {
            return;
        }
        let shell = cx.global_mut::<Shell>();
        shell.window = None;
        if !(shell.tray.is_some() && shell.close_to_tray) {
            cx.quit();
        }
    })
    .detach();

    if !background || !cx.global::<Shell>().has_tray() {
        show_window(cx);
    }
}

/// Brings the window to the front, opening it if it was closed.
pub fn show_window(cx: &mut App) {
    let Some(shell) = cx.try_global::<Shell>() else {
        return;
    };
    let (backend, connected, existing) = (
        shell.backend.clone(),
        shell.connected,
        shell.window.as_ref().map(|(h, _)| *h),
    );
    if let Some(handle) = existing
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        cx.activate(true);
        return;
    }

    let bounds = Bounds::centered(None, size(px(1040.), px(700.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(s::APP_NAME.into()),
            ..Default::default()
        }),
        app_id: Some("dev.gigapair.Gigapair".into()),
        ..Default::default()
    };
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        Theme::sync_system_appearance(Some(window), cx);
        cx.new(|cx| GigapairApp::new(backend, window, cx))
    });
    match opened {
        Ok((handle, app)) => {
            cx.global_mut::<Shell>().window = Some((handle, app.downgrade()));
            if connected {
                forward(BackendMsg::Connected, cx);
            }
            cx.activate(true);
        }
        Err(e) => {
            eprintln!("error: could not open the window: {e}");
            if !cx.global::<Shell>().has_tray() {
                cx.quit();
            }
        }
    }
}

/// The window changed the `close_to_tray` setting.
pub fn set_close_to_tray(on: bool, cx: &mut App) {
    cx.global_mut::<Shell>().close_to_tray = on;
}

fn on_backend(msg: BackendMsg, cx: &mut App) {
    let shell = cx.global_mut::<Shell>();
    match &msg {
        BackendMsg::Connected => {
            shell.connected = true;
        }
        BackendMsg::Disconnected(_) => {
            shell.connected = false;
            shell.transfers.clear();
        }
        BackendMsg::Event(ev) => {
            if let Event::Transfer(t) | Event::IncomingRequest(t) = &**ev {
                if t.status.is_terminal() {
                    shell.transfers.remove(&t.id);
                } else {
                    shell.transfers.insert(t.id.clone(), t.clone());
                }
            }
        }
    }
    if matches!(msg, BackendMsg::Connected) {
        load_state(cx);
    }
    refresh_tray(cx);
    forward(msg, cx);
}

/// Hands a service message to the window, if it is open.
fn forward(msg: BackendMsg, cx: &mut App) {
    let Some((handle, app)) = cx.global::<Shell>().window.clone() else {
        return;
    };
    let Some(app) = app.upgrade() else { return };
    let _ = handle.update(cx, |_, window, cx| {
        app.update(cx, |app, cx| app.on_backend(msg, window, cx));
    });
}

/// After (re)connecting: unfinished transfers and the close setting.
fn load_state(cx: &mut App) {
    let backend = cx.global::<Shell>().backend.clone();
    let transfers = backend.call(Request::Transfers);
    let config = backend.call(Request::ConfigGet {
        key: Some("close_to_tray".into()),
    });
    cx.spawn(async move |cx| {
        if let Ok(Ok(Response::Transfers(list))) = transfers.await {
            cx.update(|cx| {
                cx.global_mut::<Shell>().transfers =
                    list.into_iter().map(|t| (t.id.clone(), t)).collect();
                refresh_tray(cx);
            });
        }
        if let Ok(Ok(Response::Config(entries))) = config.await {
            let on = entries
                .iter()
                .find(|e| e.key == "close_to_tray")
                .and_then(|e| e.value.as_deref())
                .is_none_or(|v| v == "true");
            cx.update(|cx| set_close_to_tray(on, cx));
        }
    })
    .detach();
}

fn refresh_tray(cx: &mut App) {
    let shell = cx.global_mut::<Shell>();
    let transfers: Vec<TransferInfo> = shell.transfers.values().cloned().collect();
    let view = summarize(shell.connected, &transfers);
    if shell.shown.as_ref() == Some(&view) {
        return;
    }
    if let Some(tray) = shell.tray.as_mut() {
        tray.set(&view);
    }
    shell.shown = Some(view);
}
