//! Linux tray: a StatusNotifierItem served over D-Bus by `ksni` (async-io
//! flavour, which runs its own executor). Updates are fed from a dedicated
//! thread; GPUI's event loop is not involved.

use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use ksni::TrayMethods;
use ksni::menu::{MenuItem, StandardItem};
use tokio::sync::mpsc;

use super::{ActionSender, TrayAction, TrayView};
use crate::icon;
use crate::strings as s;

pub struct Tray {
    updates: mpsc::UnboundedSender<TrayView>,
}

impl Tray {
    /// Registers the tray. Fails when the desktop shows no tray icons
    /// (e.g. GNOME without the AppIndicator extension).
    pub fn new(actions: ActionSender) -> Result<Self, String> {
        let (updates, mut rx) = mpsc::unbounded_channel::<TrayView>();
        let (ready_tx, ready_rx) = std_mpsc::channel::<Result<(), String>>();
        std::thread::Builder::new()
            .name("gigapair-tray".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.to_string()));
                        return;
                    }
                };
                runtime.block_on(async move {
                    let item = Item {
                        view: super::summarize(false, &[]),
                        actions,
                        icons: [icon_set(false), icon_set(true)],
                    };
                    let handle = match item.spawn().await {
                        Ok(h) => h,
                        Err(e) => {
                            let _ = ready_tx.send(Err(e.to_string()));
                            return;
                        }
                    };
                    let _ = ready_tx.send(Ok(()));
                    while let Some(view) = rx.recv().await {
                        handle.update(|item| item.view = view).await;
                    }
                    handle.shutdown().await;
                });
            })
            .map_err(|e| e.to_string())?;
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self { updates }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("the tray did not start in time".into()),
        }
    }

    pub fn set(&mut self, view: &TrayView) {
        let _ = self.updates.send(view.clone());
    }
}

fn icon_set(busy: bool) -> Vec<ksni::Icon> {
    [22, 32, 48, 64]
        .into_iter()
        .map(|size| ksni::Icon {
            width: size as i32,
            height: size as i32,
            data: icon::to_argb(&icon::rgba(size, busy)),
        })
        .collect()
}

struct Item {
    view: TrayView,
    actions: ActionSender,
    icons: [Vec<ksni::Icon>; 2],
}

impl ksni::Tray for Item {
    fn id(&self) -> String {
        "gigapair".into()
    }

    fn title(&self) -> String {
        s::APP_NAME.into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icons[usize::from(self.view.busy)].clone()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: s::APP_NAME.into(),
            description: self.view.status.clone(),
            ..Default::default()
        }
    }

    /// Left click opens the window.
    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.actions.send(TrayAction::Open);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: self.view.status.clone(),
                enabled: false,
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: s::TRAY_OPEN.into(),
                activate: Box::new(|item: &mut Self| {
                    let _ = item.actions.send(TrayAction::Open);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: s::TRAY_QUIT.into(),
                activate: Box::new(|item: &mut Self| {
                    let _ = item.actions.send(TrayAction::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}
