//! Windows and macOS tray through `tray-icon`. It must be created and used
//! on the main thread; GPUI's event loop runs there and dispatches the
//! tray's window messages (Windows) or status-item events (macOS).

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use super::{ActionSender, TrayAction, TrayView};
use crate::icon;
use crate::strings as s;

const ICON_SIZE: u32 = 32;

pub struct Tray {
    icon: TrayIcon,
    status: MenuItem,
    view: Option<TrayView>,
}

impl Tray {
    pub fn new(actions: ActionSender) -> Result<Self, String> {
        let status = MenuItem::new(s::TRAY_DISCONNECTED, false, None);
        let open = MenuItem::new(s::TRAY_OPEN, true, None);
        let quit = MenuItem::new(s::TRAY_QUIT, true, None);
        let menu = Menu::new();
        menu.append_items(&[&status, &PredefinedMenuItem::separator(), &open, &quit])
            .map_err(|e| e.to_string())?;

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(cfg!(target_os = "macos"))
            .with_tooltip(s::APP_NAME)
            .with_icon(make_icon(false)?)
            .build()
            .map_err(|e| e.to_string())?;

        // Handlers run on the main thread; they only forward to the shell.
        let (open_id, quit_id) = (open.id().clone(), quit.id().clone());
        let menu_actions = actions.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            if e.id == open_id {
                let _ = menu_actions.send(TrayAction::Open);
            } else if e.id == quit_id {
                let _ = menu_actions.send(TrayAction::Quit);
            }
        }));
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| match e {
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }
            | TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } => {
                let _ = actions.send(TrayAction::Open);
            }
            _ => {}
        }));

        Ok(Self {
            icon,
            status,
            view: None,
        })
    }

    pub fn set(&mut self, view: &TrayView) {
        let busy_changed = self.view.as_ref().is_none_or(|v| v.busy != view.busy);
        self.status.set_text(&view.status);
        let _ = self.icon.set_tooltip(Some(view.tooltip()));
        if busy_changed && let Ok(i) = make_icon(view.busy) {
            let _ = self.icon.set_icon(Some(i));
        }
        self.view = Some(view.clone());
    }
}

fn make_icon(busy: bool) -> Result<Icon, String> {
    Icon::from_rgba(icon::rgba(ICON_SIZE, busy), ICON_SIZE, ICON_SIZE).map_err(|e| e.to_string())
}
