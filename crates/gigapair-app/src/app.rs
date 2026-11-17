//! The main window: contacts on the left; on the right, the selected
//! contact's drop zone and the transfer queue, or pairing, history or
//! settings. Everything goes through the service (no business logic here).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gigapair_core::api::{Event, RELAY_NOTICE, Request, Response, StatusInfo, same_network_hint};
use gigapair_core::dirs::AppDirs;
use gigapair_core::types::{
    ConnInfo, ConnKind, Contact, Direction, PendingPair, TransferDetails, TransferInfo,
    TransferStatus, format_bytes, format_duration, now_secs,
};
use gigapair_core::update::UpdateInfo;
use gpui_kit::base::{SelectableText, TextSelection};
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme, Disableable, Sizable, Theme, ThemeMode, WindowExt, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, AppContext, ClipboardItem, Context, Div, Entity, ExternalPaths, FontFeatures,
    FontWeight, Hsla, InteractiveElement, IntoElement, Keystroke, MouseButton, ParentElement,
    PathPromptOptions, Render, SharedString, Stateful, StatefulInteractiveElement, Styled,
    Subscription, Svg, Window, div, px, svg,
};

use crate::backend::{Backend, BackendMsg};
use crate::qr;
use crate::shell;
use crate::strings as s;
use crate::style::{self, Palette};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Page {
    Contact(String),
    AddContact,
    History,
    Settings,
}

pub struct GigapairApp {
    backend: Backend,
    connected: bool,
    last_error: Option<String>,
    status: Option<StatusInfo>,
    contacts: Vec<Contact>,
    transfers: Vec<TransferInfo>,
    history: Vec<TransferInfo>,
    config: HashMap<String, Option<String>>,
    pending_pairs: Vec<PendingPair>,
    page: Page,
    pair_code: Option<(String, i64)>,
    relay_banner: Option<String>,
    /// The "What is a relay?" panel in the settings is open.
    relay_info_open: bool,
    remove_armed: bool,
    /// "Delete this device's data" was clicked once; the next click deletes.
    reset_armed: bool,
    resetting: bool,
    /// The contact page shows the rename field.
    renaming: bool,
    start_at_login: Option<bool>,
    /// Where the service and the app write their log files.
    logs_dir: Option<PathBuf>,
    /// Settings → Check for updates.
    update: UpdateCheck,
    code_input: Entity<InputState>,
    pair_name_inputs: HashMap<String, Entity<InputState>>,
    rename_input: Entity<InputState>,
    device_input: Entity<InputState>,
    relay_input: Entity<InputState>,
    limit_input: Entity<InputState>,
    /// "Clear history" was clicked once; the next click clears it.
    clear_armed: bool,
    history_search: Entity<InputState>,
    history_dir: Option<Direction>,
    history_outcome: Outcome,
    /// Finished transfers loaded; "Show older" raises it.
    history_limit: u32,
    /// The service may hold older finished transfers than were loaded.
    history_more: bool,
    /// History rows showing their details, with the details once loaded.
    expanded: HashMap<String, Option<TransferDetails>>,
    _subscriptions: Vec<Subscription>,
}

/// The history's "how it ended" filter.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    All,
    Completed,
    /// Failed, cancelled or rejected.
    Problems,
}

/// Finished transfers loaded at first, and added by each "Show older".
const HISTORY_PAGE: u32 = 200;

/// Files listed in a history row's details (the service sends more).
const DETAILS_FILES_SHOWN: usize = 20;

type Done = Box<dyn FnOnce(&mut GigapairApp, Response, &mut Window, &mut Context<GigapairApp>)>;

/// Where "Check for updates" stands. Only checked when clicked.
enum UpdateCheck {
    NotYet,
    Checking,
    Done(UpdateInfo),
    /// A message ready to show.
    Failed(String),
}

impl GigapairApp {
    /// Service messages arrive through [`Self::on_backend`], called by the
    /// shell, which owns the one connection to the service.
    pub fn new(backend: Backend, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Countdown for the pairing code and coarse refresh of speeds.
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();

        cx.observe_window_appearance(window, |app, window, cx| {
            // Only a "system" choice follows the operating system.
            if !matches!(app.config_str("theme").as_str(), "light" | "dark") {
                Theme::sync_system_appearance(Some(window), cx);
            }
        })
        .detach();

        let input = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };

        let code_input = input(s::CODE_PLACEHOLDER, window, cx);
        let rename_input = input(s::NAME_PLACEHOLDER, window, cx);
        let device_input = input(s::SETTINGS_DEVICE, window, cx);
        let relay_input = input("https://relay.example.com", window, cx);
        let limit_input = input("0", window, cx);
        let history_search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(s::SEARCH_HISTORY)
                .clean_on_escape()
        });

        // Enter in a field does what its button does.
        let on_enter = |input: &Entity<InputState>,
                        window: &mut Window,
                        cx: &mut Context<Self>,
                        f: fn(&mut Self, &mut Window, &mut Context<Self>)| {
            cx.subscribe_in(input, window, move |app, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    f(app, window, cx)
                }
            })
        };
        let weak = cx.entity().downgrade();
        let subscriptions = vec![
            on_enter(&code_input, window, cx, Self::join_code),
            on_enter(&rename_input, window, cx, Self::save_rename),
            on_enter(&device_input, window, cx, |app, window, cx| {
                app.save_input("device_name", app.device_input.clone(), window, cx)
            }),
            on_enter(&relay_input, window, cx, |app, window, cx| {
                app.save_input("relay_url", app.relay_input.clone(), window, cx)
            }),
            on_enter(&limit_input, window, cx, |app, window, cx| {
                app.save_input("upload_limit_kib", app.limit_input.clone(), window, cx)
            }),
            cx.subscribe_in(&history_search, window, |_, _, ev: &InputEvent, _, cx| {
                if let InputEvent::Change = ev {
                    cx.notify()
                }
            }),
            // Before any element sees the keys, so they work wherever the
            // focus is (an input included).
            cx.intercept_keystrokes(move |event, window, cx| {
                let handled = weak
                    .update(cx, |app, cx| app.on_shortcut(&event.keystroke, window, cx))
                    .unwrap_or(false);
                if handled {
                    cx.stop_propagation();
                }
            }),
        ];

        Self {
            backend,
            connected: false,
            last_error: None,
            status: None,
            contacts: Vec::new(),
            transfers: Vec::new(),
            history: Vec::new(),
            config: HashMap::new(),
            pending_pairs: Vec::new(),
            page: Page::AddContact,
            pair_code: None,
            relay_banner: None,
            relay_info_open: false,
            remove_armed: false,
            reset_armed: false,
            resetting: false,
            renaming: false,
            start_at_login: gigapair_core::service::status().ok().map(|s| s.installed),
            logs_dir: AppDirs::from_env().ok().map(|d| d.logs),
            update: UpdateCheck::NotYet,
            code_input,
            pair_name_inputs: HashMap::new(),
            rename_input,
            device_input,
            relay_input,
            limit_input,
            clear_armed: false,
            history_search,
            history_dir: None,
            history_outcome: Outcome::All,
            history_limit: HISTORY_PAGE,
            history_more: false,
            expanded: HashMap::new(),
            _subscriptions: subscriptions,
        }
    }

    // ---------- service plumbing ----------

    fn call(
        &mut self,
        req: Request,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: impl FnOnce(&mut Self, Response, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let rx = self.backend.call(req);
        let done: Done = Box::new(done);
        cx.spawn_in(window, async move |this, cx| {
            let result = rx
                .await
                .unwrap_or_else(|_| Err("the service connection was lost".into()));
            let _ = this.update_in(cx, |app, window, cx| match result {
                Ok(resp) => done(app, resp, window, cx),
                Err(msg) => app.notify_error(msg, window, cx),
            });
        })
        .detach();
    }

    /// Asks the service to check GitHub for a newer release. The answer
    /// (or the error) is shown under the button, not as a notification.
    fn check_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.update = UpdateCheck::Checking;
        cx.notify();
        let rx = self.backend.call(Request::CheckUpdate);
        cx.spawn_in(window, async move |this, cx| {
            let result = rx
                .await
                .unwrap_or_else(|_| Err("the service connection was lost".into()));
            let _ = this.update(cx, |app, cx| {
                app.update = match result {
                    Ok(Response::Update(info)) => UpdateCheck::Done(info),
                    Ok(other) => UpdateCheck::Failed(format!("unexpected reply: {other:?}")),
                    Err(msg) => {
                        tracing::warn!("update check failed: {msg}");
                        UpdateCheck::Failed(msg)
                    }
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Fire-and-forget request; errors are shown.
    fn act(&mut self, req: Request, window: &mut Window, cx: &mut Context<Self>) {
        self.call(req, window, cx, |_, _, _, _| {});
    }

    fn notify_error(&mut self, msg: String, window: &mut Window, cx: &mut Context<Self>) {
        tracing::warn!("shown to the user: {msg}");
        window.push_notification(Notification::error(msg), cx);
        cx.notify();
    }

    fn refresh_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load_status(window, cx);
        self.load_contacts(window, cx);
        self.call(Request::Transfers, window, cx, |app, r, _, cx| {
            if let Response::Transfers(list) = r {
                app.transfers = list;
                cx.notify();
            }
        });
        self.load_history(window, cx);
        self.load_config(window, cx);
        self.call(Request::PendingPairs, window, cx, |app, r, window, cx| {
            if let Response::PendingPairs(list) = r {
                for p in &list {
                    app.ensure_name_input(p, window, cx);
                }
                if list.iter().any(|p| p.incoming) {
                    app.page = Page::AddContact;
                }
                app.pending_pairs = list;
                cx.notify();
            }
        });
    }

    fn load_status(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.call(Request::Status, window, cx, |app, r, window, cx| {
            if let Response::Status(st) = r {
                if app.status.is_none() {
                    app.device_input
                        .update(cx, |i, cx| i.set_value(st.device_name.clone(), window, cx));
                }
                app.status = Some(st);
                cx.notify();
            }
        });
    }

    fn load_contacts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.call(Request::Contacts, window, cx, |app, r, _, cx| {
            if let Response::Contacts(list) = r {
                if let Page::Contact(id) = &app.page
                    && !list.iter().any(|c| &c.node_id == id)
                {
                    app.page = Page::AddContact;
                }
                if app.page == Page::AddContact
                    && app.pending_pairs.is_empty()
                    && app.pair_code.is_none()
                    && let Some(first) = list.first()
                {
                    app.page = Page::Contact(first.node_id.clone());
                }
                app.contacts = list;
                cx.notify();
            }
        });
    }

    fn load_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.call(
            Request::History {
                limit: self.history_limit,
            },
            window,
            cx,
            |app, r, _, cx| {
                if let Response::Transfers(list) = r {
                    app.history_more = list.len() as u32 >= app.history_limit;
                    app.history = list;
                    cx.notify();
                }
            },
        );
    }

    fn load_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.call(
            Request::ConfigGet { key: None },
            window,
            cx,
            |app, r, window, cx| {
                if let Response::Config(entries) = r {
                    app.config = entries.into_iter().map(|e| (e.key, e.value)).collect();
                    let close_to_tray = app.config_bool("close_to_tray");
                    shell::set_close_to_tray(close_to_tray, cx);
                    apply_theme(&app.config_str("theme"), window, cx);
                    let relay = app.config_str("relay_url");
                    let limit = app.config_str("upload_limit_kib");
                    app.relay_input
                        .update(cx, |i, cx| i.set_value(relay, window, cx));
                    app.limit_input.update(cx, |i, cx| {
                        i.set_value(
                            if limit.is_empty() { "0".into() } else { limit },
                            window,
                            cx,
                        )
                    });
                    cx.notify();
                }
            },
        );
    }

    fn config_bool(&self, key: &str) -> bool {
        self.config
            .get(key)
            .and_then(|v| v.as_deref())
            .is_some_and(|v| v == "true")
    }

    fn config_str(&self, key: &str) -> String {
        self.config
            .get(key)
            .and_then(|v| v.clone())
            .unwrap_or_default()
    }

    fn set_config(
        &mut self,
        key: &str,
        value: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.call(
            Request::ConfigSet {
                key: key.to_owned(),
                value,
            },
            window,
            cx,
            |app, _, window, cx| {
                app.load_config(window, cx);
                app.load_status(window, cx);
            },
        );
    }

    pub fn on_backend(&mut self, msg: BackendMsg, window: &mut Window, cx: &mut Context<Self>) {
        match msg {
            BackendMsg::Connected => {
                self.connected = true;
                self.last_error = None;
                self.refresh_all(window, cx);
            }
            BackendMsg::Disconnected(e) => {
                self.connected = false;
                self.last_error = Some(e);
            }
            BackendMsg::Event(ev) => self.on_event(*ev, window, cx),
        }
        cx.notify();
    }

    fn on_event(&mut self, ev: Event, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            Event::Transfer(t) => self.upsert_transfer(t),
            Event::IncomingRequest(t) => {
                window.push_notification(
                    Notification::info(s::incoming_request(&t.peer_name, &t.label, t.total_bytes)),
                    cx,
                );
                self.upsert_transfer(t);
            }
            Event::PairRequest(p) => {
                window.push_notification(Notification::info(s::pair_request(&p.remote_name)), cx);
                self.ensure_name_input(&p, window, cx);
                self.pending_pairs.retain(|x| x.id != p.id);
                self.pending_pairs.push(p);
                self.page = Page::AddContact;
            }
            Event::PairCompleted(c) => {
                window.push_notification(Notification::success(s::paired(&c.name)), cx);
                self.pending_pairs.retain(|p| p.node_id != c.node_id);
                self.pair_code = None;
                self.page = Page::Contact(c.node_id.clone());
                self.load_contacts(window, cx);
            }
            Event::ContactsChanged => self.load_contacts(window, cx),
            Event::RelayFirstUse { relay } => {
                self.relay_banner = Some(relay);
            }
            Event::KeepAwake { .. } => self.load_status(window, cx),
            Event::Notice { message } => {
                window.push_notification(Notification::info(message), cx);
            }
            // The shell quits the app.
            Event::Reset => {}
        }
        cx.notify();
    }

    fn upsert_transfer(&mut self, t: TransferInfo) {
        if t.status.is_terminal() {
            self.transfers.retain(|x| x.id != t.id);
            self.history.retain(|x| x.id != t.id);
            self.history.insert(0, t);
        } else if let Some(x) = self.transfers.iter_mut().find(|x| x.id == t.id) {
            *x = t;
        } else {
            self.transfers.push(t);
        }
    }

    fn ensure_name_input(&mut self, p: &PendingPair, window: &mut Window, cx: &mut Context<Self>) {
        if !self.pair_name_inputs.contains_key(&p.id) {
            let name = p.remote_name.clone();
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(s::NAME_PLACEHOLDER)
                    .default_value(name)
            });
            let id = p.id.clone();
            let sub = cx.subscribe_in(
                &input,
                window,
                move |app, _, ev: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = ev {
                        app.confirm_pair(id.clone(), window, cx)
                    }
                },
            );
            self._subscriptions.push(sub);
            self.pair_name_inputs.insert(p.id.clone(), input);
        }
    }

    // ---------- actions ----------

    fn send_paths(
        &mut self,
        contact: String,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        let paths = paths.iter().map(|p| p.display().to_string()).collect();
        self.call(
            Request::Send { contact, paths },
            window,
            cx,
            |app, r, window, cx| {
                if let Response::Sent { transfer, warnings } = r {
                    if !warnings.is_empty() {
                        window.push_notification(
                            Notification::warning(s::sent_warnings(warnings.len())),
                            cx,
                        );
                    }
                    app.upsert_transfer(transfer);
                    cx.notify();
                }
            },
        );
    }

    fn pick_and_send(
        &mut self,
        contact: String,
        directories: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: !directories,
            directories,
            multiple: true,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let _ = this.update_in(cx, |app, window, cx| {
                    app.send_paths(contact, paths, window, cx)
                });
            }
        })
        .detach();
    }

    fn accept_to(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = rx.await
                && let Some(dir) = paths.pop()
            {
                let _ = this.update_in(cx, |app, window, cx| {
                    app.act(
                        Request::Accept {
                            id,
                            dest_dir: Some(dir.display().to_string()),
                        },
                        window,
                        cx,
                    )
                });
            }
        })
        .detach();
    }

    /// Switches page, dropping any half-done "click again" or rename.
    fn go(&mut self, page: Page, cx: &mut Context<Self>) {
        self.page = page;
        self.disarm();
        self.renaming = false;
        cx.notify();
    }

    /// Cancels every "click again to …" button. Returns whether one was
    /// armed.
    fn disarm(&mut self) -> bool {
        let was = self.remove_armed || self.reset_armed || self.clear_armed;
        self.remove_armed = false;
        self.reset_armed = false;
        self.clear_armed = false;
        was
    }

    /// The window's keyboard shortcuts (listed by `s::shortcuts`). Returns
    /// whether the key was used.
    fn on_shortcut(&mut self, k: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let m = &k.modifiers;
        if k.key == "escape" && !m.modified() {
            if self.renaming {
                self.renaming = false;
                cx.notify();
                return true;
            }
            if self.disarm() {
                cx.notify();
                return true;
            }
            return false;
        }
        if !m.secondary(cx) || m.alt {
            return false;
        }
        let contact = match &self.page {
            Page::Contact(id) => Some(id.clone()),
            _ => None,
        };
        match (k.key.as_str(), m.shift) {
            ("o", shift) => {
                let Some(id) = contact else { return false };
                self.pick_and_send(id, shift, window, cx);
            }
            ("n", false) => self.go(Page::AddContact, cx),
            (",", false) => self.go(Page::Settings, cx),
            ("f", false) => {
                self.go(Page::History, cx);
                self.history_search.update(cx, |i, cx| i.focus(window, cx));
            }
            (d, false) if d.len() == 1 && ("1"..="9").contains(&d) => {
                let n = d.parse::<usize>().unwrap_or(1) - 1;
                let Some(c) = self.contacts.get(n) else {
                    return false;
                };
                let page = Page::Contact(c.node_id.clone());
                self.go(page, cx);
            }
            _ => return false,
        }
        true
    }

    /// "Connect" on the Add contact page (or Enter in the code field).
    fn join_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let code = self.code_input.read(cx).value().to_string();
        if code.trim().is_empty() {
            return;
        }
        self.call(
            Request::PairJoin { code },
            window,
            cx,
            |app, r, window, cx| {
                if let Response::PendingPair(p) = r {
                    app.ensure_name_input(&p, window, cx);
                    app.pending_pairs.retain(|x| x.id != p.id);
                    app.pending_pairs.push(p);
                    app.code_input
                        .update(cx, |i, cx| i.set_value("", window, cx));
                    cx.notify();
                }
            },
        );
    }

    /// "Words match, pair" (or Enter in the name field).
    fn confirm_pair(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let name = self
            .pair_name_inputs
            .get(&id)
            .map(|i| i.read(cx).value().to_string())
            .unwrap_or_default();
        self.call(
            Request::PairConfirm {
                id: id.clone(),
                name,
            },
            window,
            cx,
            move |app, r, window, cx| {
                app.pending_pairs.retain(|p| p.id != id);
                if let Response::Contact(c) = r {
                    app.pair_code = None;
                    app.page = Page::Contact(c.node_id);
                    app.load_contacts(window, cx);
                }
                cx.notify();
            },
        );
    }

    /// Saves the open contact's new name (button or Enter).
    fn save_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Page::Contact(id) = self.page.clone() else {
            return;
        };
        let name = self.rename_input.read(cx).value().trim().to_string();
        self.renaming = false;
        cx.notify();
        if name.is_empty() {
            return;
        }
        self.call(
            Request::ContactRename {
                contact: id,
                name: name.clone(),
            },
            window,
            cx,
            move |_, _, window, cx| {
                window.push_notification(Notification::success(s::renamed(&name)), cx)
            },
        );
    }

    /// Saves a setting typed in a field (button or Enter) and says so.
    fn save_input(
        &mut self,
        key: &'static str,
        input: Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = input.read(cx).value().to_string();
        self.call(
            Request::ConfigSet {
                key: key.to_owned(),
                value,
            },
            window,
            cx,
            |app, _, window, cx| {
                window.push_notification(Notification::success(s::SAVED), cx);
                app.load_config(window, cx);
                app.load_status(window, cx);
            },
        );
    }

    /// The finished transfers that pass the history's search and filters.
    fn filtered_history(&self, cx: &Context<Self>) -> Vec<TransferInfo> {
        let query = self.history_search.read(cx).value().trim().to_lowercase();
        self.history
            .iter()
            .filter(|t| self.history_dir.is_none_or(|d| t.direction == d))
            .filter(|t| match self.history_outcome {
                Outcome::All => true,
                Outcome::Completed => t.status == TransferStatus::Completed,
                Outcome::Problems => t.status != TransferStatus::Completed,
            })
            .filter(|t| {
                query.is_empty()
                    || t.label.to_lowercase().contains(&query)
                    || t.peer_name.to_lowercase().contains(&query)
            })
            .cloned()
            .collect()
    }

    fn clear_filters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.history_dir = None;
        self.history_outcome = Outcome::All;
        self.history_search
            .update(cx, |i, cx| i.set_value("", window, cx));
        cx.notify();
    }

    /// Opens or closes a history row's details, loading them the first
    /// time.
    fn toggle_details(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.expanded.remove(&id).is_some() {
            cx.notify();
            return;
        }
        self.expanded.insert(id.clone(), None);
        cx.notify();
        let rx = self
            .backend
            .call(Request::TransferDetails { id: id.clone() });
        cx.spawn_in(window, async move |this, cx| {
            let result = rx
                .await
                .unwrap_or_else(|_| Err("the service connection was lost".into()));
            let _ = this.update_in(cx, |app, window, cx| {
                match result {
                    Ok(Response::Details(d)) => {
                        if let Some(slot) = app.expanded.get_mut(&id) {
                            *slot = Some(d);
                        }
                    }
                    // Closed again rather than stuck on "Loading…".
                    other => {
                        app.expanded.remove(&id);
                        let msg = match other {
                            Err(msg) => msg,
                            Ok(r) => format!("unexpected reply: {r:?}"),
                        };
                        app.notify_error(msg, window, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn remove_from_history(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.call(
            Request::HistoryRemove { id: id.clone() },
            window,
            cx,
            move |app, _, window, cx| {
                app.history.retain(|t| t.id != id);
                app.expanded.remove(&id);
                window.push_notification(Notification::success(s::REMOVED_FROM_HISTORY), cx);
                cx.notify();
            },
        );
    }

    /// Shows the transfer's files in the system file manager, selected:
    /// what was received, or what was sent from.
    fn show_in_folder(&mut self, t: &TransferInfo, window: &mut Window, cx: &mut Context<Self>) {
        let fallback = t
            .dest_dir
            .clone()
            .filter(|_| t.direction == Direction::Receive);
        if let Some(Some(d)) = self.expanded.get(&t.id) {
            let locations = d.locations.clone();
            reveal(&locations, fallback.as_deref(), window, cx);
            return;
        }
        self.call(
            Request::TransferDetails { id: t.id.clone() },
            window,
            cx,
            move |_, r, window, cx| {
                if let Response::Details(d) = r {
                    reveal(&d.locations, fallback.as_deref(), window, cx);
                }
            },
        );
    }

    fn choose_download_dir(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = rx.await
                && let Some(dir) = paths.pop()
            {
                let _ = this.update_in(cx, |app, window, cx| {
                    app.set_config("download_dir", dir.display().to_string(), window, cx)
                });
            }
        })
        .detach();
    }

    fn toggle_start_at_login(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        let result = if on {
            match gigapair_core::ipc::sibling_exe("gigapaird") {
                Some(exe) => AppDirs::from_env()
                    .map_err(|e| e.to_string())
                    .and_then(|d| {
                        gigapair_core::service::install(&exe, &d.logs).map_err(|e| e.to_string())
                    }),
                None => Err("gigapaird was not found next to the app".into()),
            }
        } else {
            gigapair_core::service::uninstall().map_err(|e| e.to_string())
        };
        match result {
            Ok(_) => self.start_at_login = Some(on),
            Err(e) => self.notify_error(e, window, cx),
        }
        cx.notify();
    }

    // ---------- rendering ----------
    //
    // Three zones, three forms: a dashed target you drop onto, an inverted
    // band that asks you something, and ruled rows you only watch. Signal
    // yellow marks verified data and the primary action, nothing else.

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::get(cx);
        let mut col = v_flex()
            .w(px(210.))
            .h_full()
            .flex_shrink_0()
            .px(px(8.))
            .py(px(12.))
            .gap(px(1.))
            .border_r_1()
            .border_color(p.line)
            .child(
                h_flex()
                    .gap(px(8.))
                    .px(px(6.))
                    .pt(px(2.))
                    .pb(px(12.))
                    .child(mark(28., &p))
                    .child(
                        v_flex()
                            .min_w_0()
                            .child(
                                div()
                                    .font_family(style::FONT_WIDE)
                                    .text_size(px(13.))
                                    .line_height(px(16.))
                                    .child(text(s::APP_NAME)),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(p.ink_3)
                                    .truncate()
                                    .child(
                                        self.status
                                            .as_ref()
                                            .map(|st| st.device_name.clone())
                                            .unwrap_or_default(),
                                    ),
                            ),
                    ),
            )
            .child(small_label(s::CONTACTS, &p).mx(px(6.)).my(px(4.)));

        if self.contacts.is_empty() {
            col = col.child(
                div()
                    .px(px(6.))
                    .py(px(4.))
                    .text_size(px(12.))
                    .text_color(p.ink_3)
                    .child(text(s::NO_CONTACTS)),
            );
        }
        for c in &self.contacts {
            let selected = self.page == Page::Contact(c.node_id.clone());
            let id = c.node_id.clone();
            let drop_id = c.node_id.clone();
            let pending = self
                .transfers
                .iter()
                .filter(|t| t.peer_id == c.node_id && t.status == TransferStatus::PendingAccept)
                .count();
            col = col.child(
                nav_row(
                    SharedString::from(format!("contact-{}", c.node_id)),
                    selected,
                    &p,
                )
                .on_click(cx.listener(move |app, _, _, cx| app.go(Page::Contact(id.clone()), cx)))
                // Files dropped on a contact go to that contact.
                .drag_over::<ExternalPaths>(|style, _, _, cx| {
                    let p = Palette::get(cx);
                    style.bg(p.signal.opacity(0.18)).border_color(p.ink)
                })
                .on_drop(cx.listener(move |app, paths: &ExternalPaths, window, cx| {
                    app.go(Page::Contact(drop_id.clone()), cx);
                    app.send_paths(drop_id.clone(), paths.paths().to_vec(), window, cx)
                }))
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(6.))
                        .items_baseline()
                        .child(div().truncate().child(c.name.clone()))
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(11.5))
                                .text_color(p.ink_3)
                                .child(if c.online {
                                    s::ONLINE_SHORT
                                } else {
                                    s::OFFLINE_SHORT
                                }),
                        ),
                )
                .when(pending > 0, |d| {
                    d.child(
                        div()
                            .flex_shrink_0()
                            .min_w(px(18.))
                            .h(px(18.))
                            .px(px(5.))
                            .rounded(px(5.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(p.signal)
                            .text_color(p.signal_ink)
                            .text_size(px(11.))
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .child(pending.to_string()),
                    )
                }),
            );
        }

        let nav = |id: &'static str,
                   icon: &'static str,
                   label: &'static str,
                   page: Page,
                   app: &Self,
                   cx: &mut Context<Self>| {
            // A button, so it takes keyboard focus like the rest.
            let active = app.page == page;
            let color = if active { p.ink } else { p.ink_2 };
            Button::new(id)
                .child(
                    h_flex()
                        .w_full()
                        .gap(px(8.))
                        .child(glyph(icon, 14., color))
                        .child(label),
                )
                .w_full()
                .custom(
                    ButtonCustomVariant::new(cx)
                        .color(if active {
                            p.surface
                        } else {
                            gpui_kit::transparent_black()
                        })
                        .foreground(color)
                        .hover(p.surface)
                        .active(p.surface)
                        .shadow(false),
                )
                .when(active, |b| b.border_1().border_color(p.line))
                .on_click(cx.listener(move |app, _, _, cx| app.go(page.clone(), cx)))
        };

        col = col
            .child(nav(
                "nav-add",
                style::icon::NAV_ADD,
                s::ADD_CONTACT,
                Page::AddContact,
                self,
                cx,
            ))
            .child(div().flex_1())
            .child(nav(
                "nav-history",
                style::icon::NAV_HISTORY,
                s::HISTORY,
                Page::History,
                self,
                cx,
            ))
            .child(nav(
                "nav-settings",
                style::icon::NAV_SETTINGS,
                s::SETTINGS,
                Page::Settings,
                self,
                cx,
            ));
        if self.status.as_ref().is_some_and(|st| st.keeping_awake) {
            col = col.child(
                h_flex()
                    .gap(px(6.))
                    .mt(px(6.))
                    .pt(px(8.))
                    .px(px(6.))
                    .border_t_1()
                    .border_color(p.line)
                    .text_size(px(11.5))
                    .text_color(p.ink_2)
                    .child(glyph(style::icon::AWAKE, 13., p.ink_2))
                    .child(text(s::KEEPING_AWAKE)),
            );
        }
        col
    }

    /// The actions a transfer offers, in every list it appears in.
    fn transfer_actions(&self, t: &TransferInfo, cx: &mut Context<Self>) -> Div {
        let id = t.id.clone();
        let mut buttons = h_flex().gap(px(2.)).justify_end();
        if t.status == TransferStatus::PendingAccept {
            return buttons;
        }
        if !t.status.is_terminal() {
            let (a, b) = (id.clone(), id.clone());
            let paused = t.status == TransferStatus::Paused;
            return buttons
                .child(
                    Button::new(SharedString::from(format!("pause-{id}")))
                        .label(if paused { s::RESUME } else { s::PAUSE })
                        .on_click(cx.listener(move |app, _, window, cx| {
                            let req = if paused {
                                Request::Resume { id: a.clone() }
                            } else {
                                Request::Pause { id: a.clone() }
                            };
                            app.act(req, window, cx)
                        })),
                )
                .child(
                    Button::new(SharedString::from(format!("cancel-{id}")))
                        .label(s::CANCEL)
                        .ghost()
                        .on_click(cx.listener(move |app, _, window, cx| {
                            app.act(Request::Cancel { id: b.clone() }, window, cx)
                        })),
                );
        }
        // A receiver that got nothing has nothing to show.
        if t.direction == Direction::Send || t.done_bytes > 0 {
            let shown = t.clone();
            buttons = buttons.child(
                Button::new(SharedString::from(format!("show-{id}")))
                    .label(s::SHOW_IN_FOLDER)
                    .on_click(cx.listener(move |app, _, window, cx| {
                        app.show_in_folder(&shown, window, cx)
                    })),
            );
        }
        let retryable = match t.direction {
            Direction::Send => t.status != TransferStatus::Completed,
            Direction::Receive => t.status == TransferStatus::Failed,
        };
        if retryable {
            let a = id.clone();
            buttons = buttons.child(
                Button::new(SharedString::from(format!("retry-{id}")))
                    .label(s::RETRY)
                    .on_click(cx.listener(move |app, _, window, cx| {
                        app.act(Request::Retry { id: a.clone() }, window, cx)
                    })),
            );
        }
        buttons
    }

    /// One unfinished transfer: name and connection, the strip of verified
    /// blocks, percentage, amount, speed and time left, actions.
    fn render_transfer(&self, t: &TransferInfo, cx: &mut Context<Self>) -> AnyElement {
        let p = Palette::get(cx);
        let status = t.status;
        let waiting = status == TransferStatus::WaitingAccept;
        let mode = match status {
            TransferStatus::Transferring => Some(StripMode::Live),
            TransferStatus::Paused | TransferStatus::Queued => Some(StripMode::Still),
            TransferStatus::Preparing | TransferStatus::Verifying => Some(StripMode::Hashing),
            _ => None,
        };
        let percent = t.percent();

        let rate =
            if status == TransferStatus::Transferring && t.speed_bps > 0 {
                v_flex()
                    .items_end()
                    .text_size(px(12.))
                    .line_height(px(15.))
                    .child(
                        div()
                            .text_color(p.ink)
                            .font_weight(FontWeight::SEMIBOLD)
                            .font_features(tabular())
                            .child(selectable(
                                format!("speed-{}", t.id),
                                format!("{}/s", format_bytes(t.speed_bps)),
                            )),
                    )
                    .when_some(t.eta_secs, |d, eta| {
                        d.child(div().text_color(p.ink_2).font_features(tabular()).child(
                            selectable(
                                format!("eta-{}", t.id),
                                s::time_left(&format_duration(eta)),
                            ),
                        ))
                    })
            } else {
                v_flex().items_end().child(
                    div()
                        .text_size(px(12.))
                        .text_color(p.ink_2)
                        .text_right()
                        .child(selectable(format!("status-{}", t.id), status.label())),
                )
            };

        let (big, small) = if mode.is_some() && status != TransferStatus::Preparing {
            amount_parts(Some(t.done_bytes), t.total_bytes)
        } else {
            amount_parts(None, t.total_bytes)
        };

        let row = wrapping_row(
            v_flex()
                .gap(px(2.))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if waiting { p.ink_2 } else { p.ink })
                        .child(selectable(format!("label-{}", t.id), t.label.clone())),
                )
                .child(
                    h_flex()
                        .gap(px(8.))
                        .min_w_0()
                        .flex_wrap()
                        .text_size(px(12.))
                        .text_color(p.ink_3)
                        .child(div().flex_shrink_0().child(s::files(t.file_count)))
                        .children(t.conn.as_ref().map(|c| conn_tag(&t.id, c, &p))),
                )
                .children(
                    t.conn
                        .as_ref()
                        .filter(|c| c.kind == ConnKind::Relay && c.same_network)
                        .map(|_| {
                            div()
                                .text_size(px(12.))
                                .text_color(p.ink_2)
                                .child(selectable(
                                    format!("hint-{}", t.id),
                                    same_network_hint(&t.peer_name),
                                ))
                        }),
                ),
            [
                div()
                    .w(px(150.))
                    .flex_shrink_0()
                    .when_some(mode, |d, mode| d.child(strip(percent, mode, &p))),
                div()
                    .w(px(34.))
                    .flex_shrink_0()
                    .text_right()
                    .text_size(px(12.))
                    .text_color(p.ink_2)
                    .font_features(tabular())
                    .when(mode.is_some(), |d| {
                        d.child(selectable(
                            format!("percent-{}", t.id),
                            format!("{}%", percent.floor()),
                        ))
                    }),
                div()
                    .w(px(112.))
                    .flex_shrink_0()
                    .child(amount(&t.id, big, small, &p)),
                div().w(px(74.)).flex_shrink_0().child(rate),
                div()
                    .w(px(132.))
                    .flex_shrink_0()
                    .child(self.transfer_actions(t, cx)),
            ],
        );

        v_flex()
            .py(px(8.))
            .px(px(2.))
            .gap(px(4.))
            .border_t_1()
            .border_color(p.line)
            .child(row)
            .when_some(t.note.clone(), |d, note| {
                d.child(div().text_size(px(12.)).text_color(p.ink_2).child(note))
            })
            .when_some(t.error.clone(), |d, err| {
                d.child(div().text_size(px(12.)).text_color(p.danger).child(err))
            })
            .into_any_element()
    }

    /// An incoming request, on the inverted band: the one thing on the page
    /// that waits for you.
    fn render_request(&self, t: &TransferInfo, cx: &mut Context<Self>) -> AnyElement {
        let p = Palette::get(cx);
        let (a, b, c) = (t.id.clone(), t.id.clone(), t.id.clone());
        let id = &t.id;
        let on_band = |color: Hsla| {
            ButtonCustomVariant::new(cx)
                .color(gpui_kit::transparent_black())
                .foreground(color)
                .hover(p.bg.opacity(0.12))
                .active(p.bg.opacity(0.2))
                .shadow(false)
        };
        let (reject_style, plain_style) = (on_band(p.danger_on_band), on_band(p.bg));
        // "Felipe PC wants to send movie.mov 50 GB, 1 file", flowing onto
        // more lines as needed; a long name wraps instead of being cut.
        let sentence = h_flex()
            .flex_wrap()
            .gap_x(px(4.))
            .child(
                div()
                    .min_w_0()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(selectable(format!("from-{}", t.id), t.peer_name.clone())),
            )
            .child(selectable(format!("wants-{}", t.id), s::WANTS_TO_SEND))
            .child(
                div()
                    .min_w_0()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(selectable(format!("label-{}", t.id), t.label.clone())),
            )
            .child(
                div()
                    .opacity(0.7)
                    .font_features(tabular())
                    .child(selectable(
                        format!("size-{}", t.id),
                        format!(
                            "{}, {}",
                            format_bytes(t.total_bytes),
                            s::files(t.file_count)
                        ),
                    )),
            );
        let buttons = h_flex()
            .flex_wrap()
            .gap(px(12.))
            .gap_y(px(6.))
            .child(
                Button::new(SharedString::from(format!("reject-{id}")))
                    .label(s::REJECT)
                    .custom(reject_style)
                    .on_click(cx.listener(move |app, _, window, cx| {
                        app.act(Request::Reject { id: c.clone() }, window, cx)
                    })),
            )
            .child(
                Button::new(SharedString::from(format!("accept-to-{id}")))
                    .label(s::ACCEPT_TO)
                    .custom(plain_style)
                    .border_1()
                    .border_color(p.bg.opacity(0.35))
                    .on_click(
                        cx.listener(move |app, _, window, cx| app.accept_to(b.clone(), window, cx)),
                    ),
            )
            .child(
                Button::new(SharedString::from(format!("accept-{id}")))
                    .label(s::ACCEPT)
                    .primary()
                    .on_click(cx.listener(move |app, _, window, cx| {
                        app.act(
                            Request::Accept {
                                id: a.clone(),
                                dest_dir: None,
                            },
                            window,
                            cx,
                        )
                    })),
            );
        h_flex()
            .gap(px(12.))
            .pl(px(12.))
            .pr(px(8.))
            .py(px(8.))
            .rounded(px(8.))
            .bg(p.ink)
            .text_color(p.bg)
            .child(dir_glyph(Direction::Receive, 16., p.bg, &p))
            .child(wrapping_row(sentence, [buttons]))
            .into_any_element()
    }

    /// Requests on bands, then unfinished transfers grouped by direction and
    /// contact; the open contact's groups come first.
    fn render_queue(&self, open: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::get(cx);
        let mut col = v_flex().gap(px(10.));

        let requests: Vec<_> = self
            .transfers
            .iter()
            .filter(|t| t.status == TransferStatus::PendingAccept)
            .cloned()
            .collect();
        if !requests.is_empty() {
            col = col.child(small_label(s::INCOMING, &p).mt(px(8.)));
            for t in &requests {
                col = col.child(self.render_request(t, cx));
            }
        }

        let mut groups: Vec<((bool, Direction, String), Vec<TransferInfo>)> = Vec::new();
        for t in self
            .transfers
            .iter()
            .filter(|t| t.status != TransferStatus::PendingAccept)
        {
            let key = (t.peer_id != open, t.direction, t.peer_id.clone());
            match groups.iter_mut().find(|(k, _)| *k == key) {
                Some((_, list)) => list.push(t.clone()),
                None => groups.push((key, vec![t.clone()])),
            }
        }
        groups.sort_by(|((o1, d1, _), l1), ((o2, d2, _), l2)| {
            (o1, *d1 == Direction::Receive, &l1[0].peer_name).cmp(&(
                o2,
                *d2 == Direction::Receive,
                &l2[0].peer_name,
            ))
        });

        if requests.is_empty() && groups.is_empty() {
            let name = self
                .contacts
                .iter()
                .find(|c| c.node_id == open)
                .map(|c| c.name.clone())
                .unwrap_or_default();
            col = col.child(
                div()
                    .mt(px(8.))
                    .text_size(px(12.))
                    .text_color(p.ink_3)
                    .child(text(s::no_transfers_hint(&name))),
            );
        }
        for ((_, direction, _), list) in groups {
            let peer = list[0].peer_name.clone();
            let title = match direction {
                Direction::Send => s::sending_to(&peer),
                Direction::Receive => s::receiving_from(&peer),
            };
            let mut rows = v_flex().border_b_1().border_color(p.line);
            for t in &list {
                rows = rows.child(self.render_transfer(t, cx));
            }
            col = col
                .child(
                    h_flex()
                        .mt(px(10.))
                        .gap(px(7.))
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(dir_glyph(direction, 15., p.ink, &p))
                        .child(text(title))
                        .child(
                            div()
                                .text_color(p.ink_3)
                                .font_weight(FontWeight::NORMAL)
                                .child(text(list.len().to_string())),
                        ),
                )
                .child(rows);
        }
        col
    }

    fn render_contact_page(&mut self, node_id: &str, cx: &mut Context<Self>) -> AnyElement {
        let Some(c) = self.contacts.iter().find(|c| c.node_id == node_id).cloned() else {
            return div().child(text(s::SELECT_CONTACT)).into_any_element();
        };
        let p = Palette::get(cx);
        let (id_files, id_folder) = (c.node_id.clone(), c.node_id.clone());
        let id_trust = c.node_id.clone();
        let id_remove = c.node_id.clone();
        let removed_name = c.name.clone();
        let current_name = c.name.clone();
        let armed = self.remove_armed;

        let remove = Button::new("remove-contact")
            .label(if armed { s::REMOVE_CONFIRM } else { s::REMOVE })
            .custom(
                ButtonCustomVariant::new(cx)
                    .color(gpui_kit::transparent_black())
                    .foreground(p.danger)
                    .hover(p.danger.opacity(0.1))
                    .active(p.danger.opacity(0.18))
                    .shadow(false),
            )
            .on_click(cx.listener(move |app, _, window, cx| {
                if app.remove_armed {
                    app.remove_armed = false;
                    let name = removed_name.clone();
                    app.call(
                        Request::ContactRemove {
                            contact: id_remove.clone(),
                        },
                        window,
                        cx,
                        move |_, _, window, cx| {
                            window.push_notification(
                                Notification::success(s::contact_removed(&name)),
                                cx,
                            )
                        },
                    );
                } else {
                    app.remove_armed = true;
                }
                cx.notify();
            }));

        let rename: AnyElement = if self.renaming {
            h_flex()
                .gap(px(6.))
                .child(div().w(px(170.)).child(Input::new(&self.rename_input)))
                .child(
                    Button::new("rename-save")
                        .label(s::SAVE)
                        .on_click(cx.listener(|app, _, window, cx| app.save_rename(window, cx))),
                )
                .child(
                    Button::new("rename-cancel")
                        .label(s::CANCEL)
                        .ghost()
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.renaming = false;
                            cx.notify();
                        })),
                )
                .into_any_element()
        } else {
            Button::new("rename")
                .label(s::RENAME)
                .on_click(cx.listener(move |app, _, window, cx| {
                    let name = current_name.clone();
                    app.rename_input.update(cx, |i, cx| {
                        i.set_value(name, window, cx);
                        i.focus(window, cx);
                    });
                    app.renaming = true;
                    cx.notify();
                }))
                .into_any_element()
        };

        let drop_zone = h_flex()
            .id("drop-zone")
            .gap(px(14.))
            .px(px(12.))
            .py(px(10.))
            .rounded(px(8.))
            .border(px(1.5))
            .border_dashed()
            .border_color(p.line)
            .bg(p.surface)
            // The drop itself is handled by the whole page (`render`).
            .drag_over::<ExternalPaths>(|style, _, _, cx| {
                let p = Palette::get(cx);
                style.bg(p.signal.opacity(0.1)).border_color(p.ink)
            })
            .child(
                h_flex()
                    .gap(px(3.))
                    .flex_shrink_0()
                    .children((0..6).map(|_| glyph(style::icon::CELL, 12., p.ink_3))),
            )
            .child(wrapping_row(
                v_flex()
                    .gap(px(2.))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(selectable("drop-hint", s::drop_hint(&c.name))),
                    )
                    .when(!c.online, |d| {
                        d.child(
                            div()
                                .text_size(px(12.))
                                .text_color(p.ink_2)
                                .child(selectable("offline-hint", s::offline_hint(&c.name))),
                        )
                    }),
                [h_flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .child(
                        Button::new("choose-files")
                            .label(s::CHOOSE_FILES)
                            .tooltip(s::key("O"))
                            .on_click(cx.listener(move |app, _, window, cx| {
                                app.pick_and_send(id_files.clone(), false, window, cx)
                            })),
                    )
                    .child(
                        Button::new("choose-folder")
                            .label(s::CHOOSE_FOLDER)
                            .tooltip(s::key_shift("O"))
                            .on_click(cx.listener(move |app, _, window, cx| {
                                app.pick_and_send(id_folder.clone(), true, window, cx)
                            })),
                    )],
            ));

        // Top-aligned: a wrapped id would otherwise put "Offline" beside
        // its last line.
        let meta = h_flex()
            .gap(px(10.))
            .min_w_0()
            .items_start()
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(12.))
                    .text_color(p.ink_2)
                    .child(text(if c.online { s::ONLINE } else { s::OFFLINE })),
            )
            .child(
                div()
                    .min_w_0()
                    .text_size(px(11.5))
                    .text_color(p.ink_3)
                    .child(selectable("contact-node-id", c.node_id.clone())),
            );

        v_flex()
            .gap(px(10.))
            .child(page_head(
                &s::send_to(&c.name),
                Some(meta.into_any_element()),
                Some(remove.into_any_element()),
                &p,
            ))
            .child(wrapping_row(
                div().child(
                    Switch::new("auto-accept")
                        .checked(c.auto_accept)
                        .small()
                        .label(s::AUTO_ACCEPT_LABEL)
                        .on_click(cx.listener(move |app, on: &bool, window, cx| {
                            app.act(
                                Request::ContactTrust {
                                    contact: id_trust.clone(),
                                    auto_accept: *on,
                                },
                                window,
                                cx,
                            )
                        })),
                ),
                [div().child(rename)],
            ))
            .child(drop_zone)
            .child(self.render_queue(node_id, cx))
            .child(self.render_recent(&c, cx))
            .into_any_element()
    }

    /// The last few finished transfers with this contact.
    fn render_recent(&self, c: &Contact, cx: &mut Context<Self>) -> AnyElement {
        const RECENT: usize = 5;
        let recent: Vec<TransferInfo> = self
            .history
            .iter()
            .filter(|t| t.peer_id == c.node_id)
            .take(RECENT)
            .cloned()
            .collect();
        if recent.is_empty() {
            return div().into_any_element();
        }
        let p = Palette::get(cx);
        let name = c.name.clone();
        let mut rows = v_flex().border_b_1().border_color(p.line);
        for t in &recent {
            rows = rows.child(self.render_history_row(t, cx));
        }
        v_flex()
            .mt(px(10.))
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(small_label(s::RECENT, &p))
                    .child(div().flex_1())
                    .child(
                        Button::new("see-all")
                            .label(s::SEE_ALL)
                            .ghost()
                            .small()
                            .on_click(cx.listener(move |app, _, window, cx| {
                                app.clear_filters(window, cx);
                                let name = name.clone();
                                app.history_search
                                    .update(cx, |i, cx| i.set_value(name, window, cx));
                                app.go(Page::History, cx);
                            })),
                    ),
            )
            .child(rows)
            .into_any_element()
    }

    fn render_pair_confirm(&self, pp: &PendingPair, cx: &mut Context<Self>) -> AnyElement {
        let p = Palette::get(cx);
        let (id_ok, id_no) = (pp.id.clone(), pp.id.clone());
        let input = self.pair_name_inputs.get(&pp.id).cloned();
        let mut card = v_flex()
            .gap(px(10.))
            .p(px(14.))
            .rounded(px(8.))
            .bg(p.ink)
            .text_color(p.bg)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(text(s::CONFIRM_TITLE)),
            )
            .child(div().text_size(px(12.)).opacity(0.75).child(format!(
                "{}: {}",
                pp.remote_name,
                s::CONFIRM_TEXT
            )))
            .child(
                div()
                    .font_family(style::FONT_WIDE)
                    .text_size(px(22.))
                    .child(selectable(
                        format!("fingerprint-{}", pp.id),
                        pp.fingerprint.to_uppercase(),
                    )),
            );
        if let Some(input) = &input {
            card = card.child(
                div()
                    .w(px(280.))
                    .rounded(px(6.))
                    .bg(p.surface)
                    .text_color(p.ink)
                    .child(Input::new(input)),
            );
        }
        card.child(
            h_flex()
                .gap(px(6.))
                .child(
                    Button::new(SharedString::from(format!("pair-ok-{}", pp.id)))
                        .label(s::WORDS_MATCH)
                        .primary()
                        .on_click(cx.listener(move |app, _, window, cx| {
                            app.confirm_pair(id_ok.clone(), window, cx)
                        })),
                )
                .child(
                    Button::new(SharedString::from(format!("pair-no-{}", pp.id)))
                        .label(s::REJECT)
                        .custom(
                            ButtonCustomVariant::new(cx)
                                .color(gpui_kit::transparent_black())
                                .foreground(p.danger_on_band)
                                .hover(p.bg.opacity(0.12))
                                .active(p.bg.opacity(0.2))
                                .shadow(false),
                        )
                        .on_click(cx.listener(move |app, _, window, cx| {
                            let id = id_no.clone();
                            app.pending_pairs.retain(|p| p.id != id);
                            app.act(Request::PairReject { id }, window, cx);
                            cx.notify();
                        })),
                ),
        )
        .into_any_element()
    }

    fn render_add_contact(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = Palette::get(cx);
        let mut col =
            v_flex()
                .gap(px(14.))
                .max_w(px(640.))
                .child(page_head(s::ADD_CONTACT, None, None, &p));
        for pp in self.pending_pairs.clone() {
            col = col.child(self.render_pair_confirm(&pp, cx));
        }

        // Share our code.
        let mut share = section(s::SHARE_CODE_TITLE, s::SHARE_CODE_TEXT, &p);
        let create = Button::new("create-code")
            .label(if self.pair_code.is_some() {
                s::NEW_CODE
            } else {
                s::CREATE_CODE
            })
            .primary()
            .on_click(cx.listener(|app, _, window, cx| {
                app.call(Request::PairCreate, window, cx, |app, r, _, cx| {
                    if let Response::PairCode { code, expires_at } = r {
                        app.pair_code = Some((code, expires_at));
                        cx.notify();
                    }
                })
            }));
        share = share.child(h_flex().child(create));
        if let Some((code, expires_at)) = self.pair_code.clone() {
            let left = expires_at - now_secs();
            if left <= 0 {
                self.pair_code = None;
            } else {
                let copy_code = code.clone();
                share = share.child(
                    h_flex()
                        .gap(px(14.))
                        .items_start()
                        .children(qr::qr(&code))
                        .child(
                            v_flex()
                                .gap(px(8.))
                                .flex_1()
                                .min_w_0()
                                .child(
                                    // Groups separated by spaces so the code
                                    // wraps; it is parsed either way.
                                    div()
                                        .p(px(8.))
                                        .rounded(px(6.))
                                        .bg(p.sunken)
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .text_size(px(12.))
                                        .child(text(code.replace('-', " "))),
                                )
                                .child(
                                    h_flex()
                                        .gap(px(8.))
                                        .child(Button::new("copy-code").label(s::COPY).on_click(
                                            cx.listener(move |_, _, window, cx| {
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    copy_code.clone(),
                                                ));
                                                window.push_notification(
                                                    Notification::success(s::COPIED),
                                                    cx,
                                                );
                                            }),
                                        ))
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .text_color(p.ink_3)
                                                .font_features(tabular())
                                                .child(text(s::expires_in(left))),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(p.ink_2)
                                        .child(text(s::WAITING_OTHER)),
                                ),
                        ),
                );
            }
        }
        col = col.child(share);

        // Use the other device's code.
        col = col.child(
            section(s::USE_CODE_TITLE, s::USE_CODE_TEXT, &p).child(
                h_flex()
                    .gap(px(6.))
                    .child(div().flex_1().child(Input::new(&self.code_input)))
                    .child(
                        Button::new("join")
                            .label(s::CONNECT)
                            .on_click(cx.listener(|app, _, window, cx| app.join_code(window, cx))),
                    ),
            ),
        );
        col.into_any_element()
    }

    /// One finished transfer: direction, name and contact, size, how it
    /// ended, when, and what you can still do with it.
    fn render_history_row(&self, t: &TransferInfo, cx: &mut Context<Self>) -> AnyElement {
        let p = Palette::get(cx);
        let percent = t.percent().floor();
        let (headline, detail, color) = match t.status {
            TransferStatus::Completed => (s::COMPLETED.to_owned(), None, p.ink),
            TransferStatus::Failed => (s::failed_at(percent), t.error.clone(), p.danger),
            TransferStatus::Rejected => (
                match t.direction {
                    Direction::Send => s::rejected_by(&t.peer_name),
                    Direction::Receive => s::REJECTED_BY_YOU.to_owned(),
                },
                t.error.clone(),
                p.ink_3,
            ),
            TransferStatus::Cancelled => (s::cancelled_at(percent), t.error.clone(), p.ink_3),
            other => (other.label().to_owned(), t.error.clone(), p.ink_2),
        };
        // "Rejected by Ana" is both the outcome and the stored reason.
        let detail = detail.filter(|d| *d != headline);
        let (big, small) = amount_parts(None, t.total_bytes);
        let when = local_time(t.finished_at.unwrap_or(t.created_at))
            .map(|d| d.format("%H:%M").to_string())
            .unwrap_or_default();
        let who = match t.direction {
            Direction::Send => s::sent_to(&t.peer_name, t.file_count),
            Direction::Receive => s::received_from(&t.peer_name, t.file_count),
        };
        let open = self.expanded.contains_key(&t.id);
        let id = t.id.clone();

        let name = v_flex()
            .gap(px(2.))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(selectable(format!("label-{}", t.id), t.label.clone())),
            )
            .child(
                h_flex()
                    .gap(px(8.))
                    .min_w_0()
                    .flex_wrap()
                    .text_size(px(12.))
                    .text_color(p.ink_3)
                    .child(
                        div()
                            .flex_shrink_0()
                            .child(selectable(format!("who-{}", t.id), who)),
                    )
                    .children(t.conn.as_ref().map(|c| conn_tag(&t.id, c, &p))),
            );
        let columns = [
            div()
                .w(px(96.))
                .flex_shrink_0()
                .child(amount(&t.id, big, small, &p)),
            v_flex()
                .w(px(220.))
                .flex_shrink_0()
                .text_size(px(12.))
                .line_height(px(16.))
                .child(
                    div()
                        .text_color(color)
                        .child(selectable(format!("outcome-{}", t.id), headline)),
                )
                .when_some(detail, |d, detail| {
                    d.child(
                        div()
                            .text_color(p.ink_2)
                            .line_clamp(2)
                            .child(selectable(format!("detail-{}", t.id), detail)),
                    )
                }),
            div()
                .w(px(40.))
                .flex_shrink_0()
                .text_right()
                .text_size(px(12.))
                .text_color(p.ink_3)
                .font_features(tabular())
                .child(selectable(format!("when-{}", t.id), when)),
            h_flex()
                .min_w(px(96.))
                .flex_shrink_0()
                .gap(px(2.))
                .justify_end()
                .child(self.transfer_actions(t, cx))
                .child(
                    Button::new(SharedString::from(format!("details-{}", t.id)))
                        .label(if open { s::HIDE_DETAILS } else { s::DETAILS })
                        .ghost()
                        .on_click(cx.listener(move |app, _, window, cx| {
                            app.toggle_details(id.clone(), window, cx)
                        })),
                ),
        ];

        v_flex()
            .border_t_1()
            .border_color(p.line)
            .child(
                h_flex()
                    .gap(px(14.))
                    .py(px(8.))
                    .px(px(2.))
                    .child(dir_glyph(t.direction, 16., p.ink, &p))
                    .child(wrapping_row(name, columns)),
            )
            .when(open, |d| {
                d.child(self.render_details(t, self.expanded.get(&t.id).cloned().flatten(), cx))
            })
            .into_any_element()
    }

    /// Under a history row: the whole error, when, where the files are,
    /// the files, and removing the row.
    fn render_details(
        &self,
        t: &TransferInfo,
        details: Option<TransferDetails>,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = Palette::get(cx);
        let col = v_flex()
            .gap(px(8.))
            .pl(px(32.))
            .pr(px(2.))
            .pb(px(12.))
            .text_size(px(12.))
            .text_color(p.ink_2);
        let Some(d) = details else {
            return col.child(text(s::LOADING));
        };
        let heading = |label: String| {
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(p.ink)
                .child(selectable(format!("{label}-{}", t.id), label))
        };
        let id = t.id.clone();

        let timing = match (local_time(t.created_at), t.finished_at.and_then(local_time)) {
            (Some(start), Some(end)) => {
                let same_day = start.date_naive() == end.date_naive();
                let took =
                    format_duration(end.signed_duration_since(start).num_seconds().max(0) as u64);
                Some(s::timing(
                    &start.format("%-d %b %H:%M").to_string(),
                    &end.format(if same_day { "%H:%M" } else { "%-d %b %H:%M" })
                        .to_string(),
                    &took,
                ))
            }
            _ => None,
        };

        let mut files = v_flex().gap(px(2.));
        for (i, f) in d.files.iter().take(DETAILS_FILES_SHOWN).enumerate() {
            files = files.child(
                h_flex()
                    .gap(px(8.))
                    .items_start()
                    .child(
                        div()
                            .w(px(10.))
                            .flex_shrink_0()
                            .text_color(if f.done { p.ink } else { p.ink_3 })
                            .child(if f.done { "✓" } else { "·" }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(selectable(format!("file-{id}-{i}"), f.path.clone())),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(p.ink_3)
                            .font_features(tabular())
                            .child(selectable(format!("fsize-{id}-{i}"), format_bytes(f.size))),
                    ),
            );
        }
        let more = d.files.len().saturating_sub(DETAILS_FILES_SHOWN) as u64 + d.more_files;

        let has_location = !d.locations.is_empty();
        let (show, remove) = (t.clone(), t.id.clone());
        // A rejection's reason is already the row's outcome.
        let error = t
            .error
            .clone()
            .filter(|_| t.status != TransferStatus::Rejected);
        col.when_some(error, |c, e| {
            c.child(
                div()
                    .text_color(p.danger)
                    .child(selectable(format!("error-full-{id}"), e)),
            )
        })
        .when_some(timing, |c, timing| {
            c.child(selectable(format!("timing-{id}"), timing))
        })
        .child(
            v_flex()
                .gap(px(2.))
                .child(heading(s::ON_THIS_DEVICE.to_owned()))
                .when(!has_location, |c| c.child(text(s::NOT_ON_DEVICE)))
                .children(d.locations.iter().enumerate().map(|(i, l)| {
                    div().child(selectable(format!("location-{id}-{i}"), l.clone()))
                })),
        )
        .child(
            v_flex()
                .gap(px(2.))
                .child(heading(s::file_count_title(t.file_count)))
                .child(files)
                .when(more > 0, |c| c.child(text(s::more_files(more)))),
        )
        .child(
            h_flex()
                .gap(px(6.))
                .when(has_location, |c| {
                    c.child(
                        Button::new(SharedString::from(format!("details-show-{id}")))
                            .label(s::SHOW_IN_FOLDER)
                            .small()
                            .on_click(cx.listener(move |app, _, window, cx| {
                                app.show_in_folder(&show, window, cx)
                            })),
                    )
                })
                .child(
                    Button::new(SharedString::from(format!("details-remove-{id}")))
                        .label(s::REMOVE_FROM_HISTORY)
                        .small()
                        .custom(danger_ghost(&p, cx))
                        .on_click(cx.listener(move |app, _, window, cx| {
                            app.remove_from_history(remove.clone(), window, cx)
                        })),
                ),
        )
    }

    fn render_history(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = Palette::get(cx);
        let armed = self.clear_armed;
        let clear = Button::new("clear-history")
            .label(if armed {
                s::CLEAR_HISTORY_CONFIRM
            } else {
                s::CLEAR_HISTORY
            })
            .when(armed, |b| b.custom(danger_ghost(&p, cx)))
            .when(!armed, |b| b.ghost())
            .disabled(self.history.is_empty())
            .on_click(cx.listener(|app, _, window, cx| {
                if !app.clear_armed {
                    app.clear_armed = true;
                    cx.notify();
                    return;
                }
                app.clear_armed = false;
                app.call(Request::ClearHistory, window, cx, |app, _, window, cx| {
                    app.history.clear();
                    app.expanded.clear();
                    window.push_notification(Notification::success(s::HISTORY_CLEARED), cx);
                    cx.notify();
                })
            }));
        let meta = div()
            .text_size(px(12.))
            .text_color(p.ink_2)
            .child(text(s::HISTORY_META));
        let mut col = v_flex().gap(px(10.)).child(page_head(
            s::HISTORY,
            Some(meta.into_any_element()),
            Some(clear.into_any_element()),
            &p,
        ));
        if self.history.is_empty() {
            return col
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(p.ink_3)
                        .child(text(s::NO_HISTORY)),
                )
                .into_any_element();
        }

        let chip = |id: &'static str,
                    label: &'static str,
                    selected: bool,
                    set: fn(&mut Self),
                    cx: &mut Context<Self>| {
            Button::new(id)
                .label(label)
                .small()
                .when(selected, |b| b.primary())
                .when(!selected, |b| b.ghost())
                .on_click(cx.listener(move |app, _, _, cx| {
                    set(app);
                    cx.notify();
                }))
        };
        let (dir, outcome) = (self.history_dir, self.history_outcome);
        col = col.child(
            h_flex()
                .flex_wrap()
                .gap_x(px(14.))
                .gap_y(px(6.))
                .child(
                    div().w(px(260.)).child(
                        Input::new(&self.history_search)
                            .small()
                            .cleanable(true)
                            .prefix(glyph(style::icon::SEARCH, 13., p.ink_3)),
                    ),
                )
                .child(
                    h_flex()
                        .gap(px(2.))
                        .child(chip(
                            "dir-all",
                            s::FILTER_ALL,
                            dir.is_none(),
                            |a| a.history_dir = None,
                            cx,
                        ))
                        .child(chip(
                            "dir-sent",
                            s::FILTER_SENT,
                            dir == Some(Direction::Send),
                            |a| a.history_dir = Some(Direction::Send),
                            cx,
                        ))
                        .child(chip(
                            "dir-received",
                            s::FILTER_RECEIVED,
                            dir == Some(Direction::Receive),
                            |a| a.history_dir = Some(Direction::Receive),
                            cx,
                        )),
                )
                .child(
                    h_flex()
                        .gap(px(2.))
                        .child(chip(
                            "outcome-all",
                            s::FILTER_ALL,
                            outcome == Outcome::All,
                            |a| a.history_outcome = Outcome::All,
                            cx,
                        ))
                        .child(chip(
                            "outcome-completed",
                            s::FILTER_COMPLETED,
                            outcome == Outcome::Completed,
                            |a| a.history_outcome = Outcome::Completed,
                            cx,
                        ))
                        .child(chip(
                            "outcome-problems",
                            s::FILTER_PROBLEMS,
                            outcome == Outcome::Problems,
                            |a| a.history_outcome = Outcome::Problems,
                            cx,
                        )),
                ),
        );

        let list = self.filtered_history(cx);
        if list.is_empty() {
            col = col.child(
                h_flex()
                    .gap(px(10.))
                    .mt(px(6.))
                    .text_size(px(12.))
                    .text_color(p.ink_3)
                    .child(text(s::NO_MATCHES))
                    .child(
                        Button::new("clear-filters")
                            .label(s::CLEAR_FILTERS)
                            .small()
                            .on_click(
                                cx.listener(|app, _, window, cx| app.clear_filters(window, cx)),
                            ),
                    ),
            );
        }

        let today = chrono::Local::now().date_naive();
        let mut day: Option<chrono::NaiveDate> = None;
        let mut rows: Option<Div> = None;
        for t in list {
            let date = local_time(t.finished_at.unwrap_or(t.created_at)).map(|d| d.date_naive());
            if rows.is_none() || date != day {
                if let Some(r) = rows.take() {
                    col = col.child(r);
                }
                day = date;
                col = col.child(
                    div()
                        .mt(px(10.))
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(text(date.map(|d| day_label(d, today)).unwrap_or_default())),
                );
                rows = Some(v_flex().border_b_1().border_color(p.line));
            }
            let row = self.render_history_row(&t, cx);
            rows = rows.map(|r| r.child(row));
        }
        if let Some(r) = rows {
            col = col.child(r);
        }
        // The service may hold older ones than were loaded.
        if self.history_more {
            col = col.child(
                h_flex().child(
                    Button::new("history-more")
                        .label(s::SHOW_MORE)
                        .ghost()
                        .on_click(cx.listener(|app, _, window, cx| {
                            app.history_limit += HISTORY_PAGE;
                            app.load_history(window, cx);
                        })),
                ),
            );
        }
        col.into_any_element()
    }

    /// Deletes this device's data after a second click, then quits.
    fn reset_button(&mut self, cx: &mut Context<Self>) -> Button {
        let p = Palette::get(cx);
        let label = if self.resetting {
            s::RESETTING
        } else if self.reset_armed {
            s::RESET_CONFIRM
        } else {
            s::RESET
        };
        Button::new("reset-device")
            .label(label)
            .disabled(self.resetting)
            .custom(
                ButtonCustomVariant::new(cx)
                    .color(gpui_kit::transparent_black())
                    .foreground(p.danger)
                    .hover(p.danger.opacity(0.1))
                    .active(p.danger.opacity(0.18))
                    .shadow(false),
            )
            .on_click(cx.listener(|app, _, window, cx| {
                if !app.reset_armed {
                    app.reset_armed = true;
                    cx.notify();
                    return;
                }
                app.reset_armed = false;
                app.resetting = true;
                tracing::info!("deleting this device's data from the app");
                cx.notify();
                app.call(Request::Reset, window, cx, |_, _, _, cx| {
                    tracing::info!("this device's data was deleted; quitting");
                    cx.quit();
                });
            }))
    }

    fn render_settings(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = Palette::get(cx);
        let switch = |id: &'static str,
                      label: &'static str,
                      key: &'static str,
                      app: &Self,
                      cx: &mut Context<Self>| {
            Switch::new(id)
                .checked(app.config_bool(key))
                .small()
                .label(label)
                .on_click(cx.listener(move |app, on: &bool, window, cx| {
                    app.set_config(key, on.to_string(), window, cx)
                }))
        };
        let label = |text: &'static str| {
            div()
                .mt(px(6.))
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(self::text(text))
        };
        let note = |text: &'static str| {
            div()
                .text_size(px(12.))
                .text_color(p.ink_3)
                .child(self::text(text))
        };
        let download = self
            .status
            .as_ref()
            .map(|st| st.download_dir.clone())
            .unwrap_or_default();
        let node = self
            .status
            .as_ref()
            .map(|st| st.node_id.clone())
            .unwrap_or_default();
        let logs = self.logs_dir.clone();

        v_flex()
            .gap(px(8.))
            .max_w(px(640.))
            .child(page_head(s::SETTINGS, None, None, &p))
            .child(label(s::SETTINGS_DOWNLOADS))
            .child(
                h_flex()
                    .gap(px(6.))
                    .child(div().flex_1().min_w_0().child(text(download)))
                    .child(Button::new("change-download").label(s::CHANGE).on_click(
                        cx.listener(|app, _, window, cx| app.choose_download_dir(window, cx)),
                    )),
            )
            .child(label(s::SETTINGS_DEVICE))
            .child(
                h_flex()
                    .gap(px(6.))
                    .child(div().flex_1().child(Input::new(&self.device_input)))
                    .child(
                        Button::new("save-device")
                            .label(s::SAVE)
                            .on_click(cx.listener(|app, _, window, cx| {
                                app.save_input("device_name", app.device_input.clone(), window, cx)
                            })),
                    ),
            )
            .child(label(s::SETTINGS_APPEARANCE))
            .child(
                h_flex().gap(px(6.)).children(
                    [
                        ("theme-system", s::THEME_SYSTEM, "system"),
                        ("theme-light", s::THEME_LIGHT, "light"),
                        ("theme-dark", s::THEME_DARK, "dark"),
                    ]
                    .map(|(id, text, value)| {
                        let current = self.config_str("theme");
                        let selected =
                            current == value || (value == "system" && current.is_empty());
                        Button::new(id)
                            .label(text)
                            .when(selected, |b| b.primary())
                            .on_click(cx.listener(move |app, _, window, cx| {
                                app.set_config("theme", value.to_owned(), window, cx)
                            }))
                    }),
                ),
            )
            .child(label(s::SETTINGS_BEHAVIOUR))
            .child(switch(
                "sw-awake",
                s::SETTINGS_KEEP_AWAKE,
                "keep_awake",
                self,
                cx,
            ))
            .child(switch(
                "sw-notify",
                s::SETTINGS_NOTIFICATIONS,
                "notifications",
                self,
                cx,
            ))
            .when(cx.global::<shell::Shell>().has_tray(), |el| {
                el.child(switch(
                    "sw-tray",
                    s::SETTINGS_CLOSE_TO_TRAY,
                    "close_to_tray",
                    self,
                    cx,
                ))
            })
            .child(
                Switch::new("sw-login")
                    .checked(self.start_at_login.unwrap_or(false))
                    .disabled(self.start_at_login.is_none())
                    .small()
                    .label(s::SETTINGS_START_LOGIN)
                    .on_click(cx.listener(|app, on: &bool, window, cx| {
                        app.toggle_start_at_login(*on, window, cx)
                    })),
            )
            .child(label(s::SETTINGS_NETWORK))
            .child(switch(
                "sw-direct",
                s::SETTINGS_DIRECT_ONLY,
                "direct_only",
                self,
                cx,
            ))
            .child(note(s::SETTINGS_DIRECT_ONLY_NOTE))
            .child(
                h_flex()
                    .id("relay-info-toggle")
                    .gap(px(6.))
                    .cursor_pointer()
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.ink_2)
                    .child(if self.relay_info_open { "▾" } else { "▸" })
                    .child(s::RELAY_INFO_TITLE)
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.relay_info_open = !app.relay_info_open;
                        cx.notify();
                    })),
            )
            .when(self.relay_info_open, |el| {
                el.child(
                    v_flex()
                        .gap(px(6.))
                        .p(px(10.))
                        .rounded(px(8.))
                        .bg(p.surface)
                        .border_1()
                        .border_color(p.line)
                        .text_size(px(12.))
                        .text_color(p.ink_2)
                        .children(s::RELAY_INFO.iter().map(|para| div().child(text(*para)))),
                )
            })
            .child(switch(
                "sw-discovery",
                s::SETTINGS_DISCOVERY,
                "discovery",
                self,
                cx,
            ))
            .child(label(s::SETTINGS_RELAY))
            .child(
                h_flex()
                    .gap(px(6.))
                    .child(div().flex_1().child(Input::new(&self.relay_input)))
                    .child(
                        Button::new("save-relay")
                            .label(s::SAVE)
                            .on_click(cx.listener(|app, _, window, cx| {
                                app.save_input("relay_url", app.relay_input.clone(), window, cx)
                            })),
                    ),
            )
            .child(label(s::SETTINGS_LIMIT))
            .child(
                h_flex()
                    .gap(px(6.))
                    .child(div().w(px(160.)).child(Input::new(&self.limit_input)))
                    .child(
                        Button::new("save-limit")
                            .label(s::SAVE)
                            .on_click(cx.listener(|app, _, window, cx| {
                                app.save_input(
                                    "upload_limit_kib",
                                    app.limit_input.clone(),
                                    window,
                                    cx,
                                )
                            })),
                    ),
            )
            .child(note(s::RESTART_NOTE))
            .child(label(s::SETTINGS_SHORTCUTS))
            .child(
                v_flex().gap(px(3.)).text_size(px(12.)).children(
                    s::shortcuts()
                        .into_iter()
                        .enumerate()
                        .map(|(i, (keys, what))| {
                            h_flex()
                                .gap(px(10.))
                                .items_start()
                                .child(
                                    div()
                                        .w(px(130.))
                                        .flex_shrink_0()
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(selectable(format!("shortcut-{i}"), keys)),
                                )
                                .child(div().min_w_0().text_color(p.ink_2).child(text(what)))
                        }),
                ),
            )
            .child(label(s::SETTINGS_UPDATES))
            .child(self.render_update_check(cx))
            .child(note(s::UPDATES_NOTE))
            .child(label(s::SETTINGS_LOGS))
            .child(
                h_flex()
                    .gap(px(6.))
                    .child(
                        div().flex_1().min_w_0().child(text(
                            logs.as_ref()
                                .map(|p| p.display().to_string())
                                .unwrap_or_default(),
                        )),
                    )
                    .child(
                        Button::new("open-logs")
                            .label(s::OPEN)
                            .disabled(logs.is_none())
                            .on_click(move |_, _, cx| {
                                if let Some(p) = &logs {
                                    tracing::info!("opening the logs folder");
                                    cx.open_with_system(p);
                                }
                            }),
                    ),
            )
            .child(note(s::LOGS_NOTE))
            .child(label(s::SETTINGS_NODE))
            .child(
                div()
                    .text_size(px(11.5))
                    .text_color(p.ink_3)
                    .child(text(node)),
            )
            .child(label(s::SETTINGS_RESET))
            .child(note(s::RESET_NOTE))
            .child(h_flex().child(self.reset_button(cx)))
            .into_any_element()
    }

    /// This version, the button, and what the last check found.
    fn render_update_check(&self, cx: &mut Context<Self>) -> Div {
        let p = Palette::get(cx);
        let checking = matches!(self.update, UpdateCheck::Checking);
        let result: Option<AnyElement> = match &self.update {
            UpdateCheck::NotYet | UpdateCheck::Checking => None,
            UpdateCheck::Done(info) => Some(match &info.newer {
                None => div()
                    .text_size(px(12.))
                    .text_color(p.ink_2)
                    .child(text(s::UP_TO_DATE))
                    .into_any_element(),
                Some(r) => {
                    let url = r.url.clone();
                    h_flex()
                        .flex_wrap()
                        .gap(px(8.))
                        .child(
                            div()
                                .min_w_0()
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(text(s::update_available(&r.version, r.prerelease))),
                        )
                        .child(
                            Button::new("open-release")
                                .label(s::OPEN_RELEASE)
                                .primary()
                                .on_click(move |_, _, cx| {
                                    tracing::info!("opening the release page");
                                    cx.open_url(&url);
                                }),
                        )
                        .into_any_element()
                }
            }),
            UpdateCheck::Failed(msg) => Some(
                div()
                    .text_size(px(12.))
                    .text_color(p.danger)
                    .child(text(msg.clone()))
                    .into_any_element(),
            ),
        };
        v_flex()
            .gap(px(6.))
            .child(
                h_flex()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_features(tabular())
                            .child(text(s::version_line(gigapair_core::update::CURRENT))),
                    )
                    .child(
                        Button::new("check-updates")
                            .label(s::CHECK_UPDATES)
                            .loading(checking)
                            .disabled(checking)
                            .on_click(
                                cx.listener(|app, _, window, cx| app.check_updates(window, cx)),
                            ),
                    ),
            )
            .children(result)
    }
}

impl Render for GigapairApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::get(cx);
        let page = match self.page.clone() {
            Page::Contact(id) => self.render_contact_page(&id, cx),
            Page::AddContact => self.render_add_contact(cx),
            Page::History => self.render_history(cx),
            Page::Settings => self.render_settings(cx),
        };

        let mut main = v_flex()
            .id("main")
            .flex_1()
            .min_w_0()
            .h_full()
            .px(px(18.))
            .py(px(14.))
            .gap(px(10.))
            .overflow_y_scroll();
        if !self.connected {
            main = main.child(
                h_flex()
                    .px(px(12.))
                    .py(px(8.))
                    .rounded(px(8.))
                    .bg(p.sunken)
                    .text_size(px(12.))
                    .text_color(p.ink_2)
                    .child(text(match &self.last_error {
                        Some(e) => format!("{} ({e})", s::CONNECTING_SERVICE),
                        None => s::CONNECTING_SERVICE.to_owned(),
                    })),
            );
        }
        if let Some(relay) = self.relay_banner.clone() {
            main = main.child(
                v_flex()
                    .gap(px(6.))
                    .px(px(12.))
                    .py(px(10.))
                    .rounded(px(8.))
                    .bg(p.surface)
                    .border_1()
                    .border_color(p.line)
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(glyph(style::icon::LINK_RELAY, 18., p.ink))
                            .child(text(format!("{} ({relay})", s::RELAY_SLOWER))),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(p.ink_2)
                            .child(text(RELAY_NOTICE)),
                    )
                    .child(
                        h_flex().child(Button::new("dismiss-relay").label(s::DISMISS).on_click(
                            cx.listener(|app, _, window, cx| {
                                app.relay_banner = None;
                                app.act(Request::AckRelayNotice, window, cx);
                                cx.notify();
                            }),
                        )),
                    ),
            );
        }
        if let Page::Contact(id) = self.page.clone() {
            main = main.on_drop(cx.listener(move |app, paths: &ExternalPaths, window, cx| {
                app.send_paths(id.clone(), paths.paths().to_vec(), window, cx)
            }));
        }
        main = main.child(div().w_full().max_w(px(1100.)).child(page));

        h_flex()
            .id("window-root")
            // A plain click anywhere clears the text selection. gpui-base
            // only does this while its mouse-up handler keeps seeing the
            // release, which inputs and buttons stop; after clicking one,
            // a selection would otherwise stay until the next drag.
            // A click that no focusable element took (empty space, text)
            // moves the keyboard focus away, so a switch or input clicked
            // earlier stops looking selected. Focusable elements focus
            // themselves first and mark the event (prevent_default).
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                if !window.default_prevented() {
                    window.blur(cx);
                }
            })
            .capture_any_mouse_down(|event, window, cx| {
                if event.button == MouseButton::Left
                    && event.click_count == 1
                    && !event.modifiers.shift
                {
                    TextSelection::clear(window, cx);
                }
            })
            .size_full()
            .bg(p.bg)
            .text_color(p.ink)
            .text_size(px(13.))
            .child(self.render_sidebar(cx))
            .child(main)
            // Right click: Copy, for the text selected when the menu opens
            // (the click on the item itself clears the selection).
            .context_menu(|menu, window, cx| {
                let selected = TextSelection::selected_text(window, cx).trim().to_owned();
                let item = PopupMenuItem::new(s::COPY).disabled(selected.is_empty());
                menu.item(item.on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(selected.clone()));
                }))
            })
    }
}

/// Ctrl+C (Cmd+C on macOS) copies the text selected in the window,
/// whichever element has the keyboard focus. gpui-base binds the shortcut
/// on its root, but a focused input earlier in the dispatch path takes the
/// copy itself (even with nothing selected in it), so the root never saw
/// it. Intercepting runs before any of that; with no window selection the
/// keystroke goes on as usual, so copying inside an input still works.
pub fn install_copy_shortcut(cx: &mut gpui_kit::App) {
    cx.intercept_keystrokes(|event, window, cx| {
        let k = &event.keystroke;
        let m = &k.modifiers;
        if k.key != "c" || !m.secondary(cx) || m.shift || m.alt {
            return;
        }
        let selected = TextSelection::selected_text(window, cx);
        let selected = selected.trim();
        if selected.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(selected.to_owned()));
        cx.stop_propagation();
    })
    .detach();
}

/// Applies the `theme` setting: `light` or `dark` force that scheme,
/// anything else follows the operating system.
fn apply_theme(choice: &str, window: &mut Window, cx: &mut gpui_kit::App) {
    match choice {
        "light" => Theme::change(ThemeMode::Light, Some(window), cx),
        "dark" => Theme::change(ThemeMode::Dark, Some(window), cx),
        _ => Theme::sync_system_appearance(Some(window), cx),
    }
}

// ---------- building blocks ----------

/// Figures that keep their width while they count.
fn tabular() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".into(), 1)]))
}

/// A one-colour glyph from `style::icon`.
fn glyph(path: &'static str, size: f32, color: Hsla) -> Svg {
    svg()
        .path(path)
        .size(px(size))
        .flex_shrink_0()
        .text_color(color)
}

/// The mark: an ink G whose bar ends in a yellow block.
fn mark(size: f32, p: &Palette) -> Div {
    div()
        .relative()
        .size(px(size))
        .flex_shrink_0()
        .child(glyph(style::icon::MARK_G, size, p.ink).absolute())
        .child(glyph(style::icon::MARK_BLOCK, size, p.signal).absolute())
}

/// Up (sent: away from you) or down (received: brought to you), over a
/// yellow block.
fn dir_glyph(direction: Direction, size: f32, ink: Hsla, p: &Palette) -> Div {
    let arrow = match direction {
        Direction::Send => style::icon::DIR_UP,
        Direction::Receive => style::icon::DIR_DOWN,
    };
    div()
        .relative()
        .size(px(size))
        .flex_shrink_0()
        .child(glyph(style::icon::DIR_BLOCK, size, p.signal).absolute())
        .child(glyph(arrow, size, ink).absolute())
}

/// A text button in the danger colour (removing, deleting).
fn danger_ghost(p: &Palette, cx: &mut gpui_kit::App) -> ButtonCustomVariant {
    ButtonCustomVariant::new(cx)
        .color(gpui_kit::transparent_black())
        .foreground(p.danger)
        .hover(p.danger.opacity(0.1))
        .active(p.danger.opacity(0.18))
        .shadow(false)
}

fn small_label(text: &'static str, p: &Palette) -> Div {
    div()
        .text_size(px(11.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(p.ink_3)
        .child(self::text(text))
}

/// A clickable row in the sidebar.
fn nav_row(id: SharedString, selected: bool, p: &Palette) -> Stateful<Div> {
    let (surface, line) = (p.surface, p.line);
    h_flex()
        .id(id)
        .gap(px(8.))
        .px(px(6.))
        .py(px(5.))
        .rounded(px(6.))
        .cursor_pointer()
        .border_1()
        .border_color(gpui_kit::transparent_black())
        .when(selected, |d| d.bg(surface).border_color(line))
        .hover(move |d| d.bg(surface))
}

/// Page title, a line under it, and an action on the right.
fn page_head(
    title: &str,
    meta: Option<AnyElement>,
    action: Option<AnyElement>,
    p: &Palette,
) -> impl IntoElement {
    h_flex()
        .gap(px(12.))
        .items_start()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(1.))
                .child(
                    div()
                        .font_family(style::FONT_SEMIWIDE)
                        .text_size(px(18.))
                        .line_height(px(22.))
                        .text_color(p.ink)
                        .child(selectable("page-title", title.to_owned())),
                )
                .children(meta),
        )
        .children(action)
}

/// A titled block on a raised surface (pairing).
fn section(title: &'static str, text: &'static str, p: &Palette) -> Div {
    v_flex()
        .gap(px(8.))
        .p(px(14.))
        .rounded(px(8.))
        .bg(p.surface)
        .border_1()
        .border_color(p.line)
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(self::text(title)),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(p.ink_2)
                .child(self::text(text)),
        )
}

/// "Direct" or "Via relay use1-1", with the full relay host on hover.
fn conn_tag(id: &str, c: &ConnInfo, p: &Palette) -> AnyElement {
    let (icon, label, host) = match c.kind {
        ConnKind::Direct => (style::icon::LINK_DIRECT, s::DIRECT.to_owned(), None),
        ConnKind::Relay => (
            style::icon::LINK_RELAY,
            s::via_relay(c.relay.as_deref().map(relay_short_name)),
            c.relay.clone(),
        ),
    };
    let tag = h_flex()
        .id(SharedString::from(format!("conn-{id}")))
        .flex_shrink_0()
        .gap(px(5.))
        .px(px(7.))
        .py(px(1.))
        .rounded(px(5.))
        .bg(p.sunken)
        .text_size(px(11.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(p.ink_2)
        .child(svg().path(icon).w(px(18.)).h(px(7.)).text_color(p.ink_2))
        .child(selectable(format!("conn-label-{id}"), label));
    match host {
        Some(host) => tag
            .tooltip(move |window, cx| Tooltip::new(host.clone()).build(window, cx))
            .into_any_element(),
        None => tag.into_any_element(),
    }
}

/// `use1-1` from `https://use1-1.relay.n0.iroh.link./`.
fn relay_short_name(host: &str) -> &str {
    let host = host.split("://").last().unwrap_or(host);
    host.split(['.', '/', ':']).next().unwrap_or(host)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StripMode {
    /// Blocks are arriving: the next one is in flight.
    Live,
    /// Paused or waiting for the contact: what's verified stays.
    Still,
    /// Reading the files to hash them, not moving data.
    Hashing,
}

/// The progress strip: 40 cells, filled as blocks are verified.
fn strip(percent: f64, mode: StripMode, p: &Palette) -> Div {
    const CELLS: usize = 40;
    let verified = ((percent / 100.0) * CELLS as f64).floor() as usize;
    h_flex()
        .w_full()
        .h(px(7.))
        .gap(px(2.))
        .children((0..CELLS).map(|i| {
            let color = if i < verified {
                match mode {
                    StripMode::Live => p.signal,
                    StripMode::Still => p.signal.opacity(0.45),
                    StripMode::Hashing => p.ink_3,
                }
            } else if i == verified && mode == StripMode::Live {
                p.cell_flight
            } else {
                p.cell_empty
            };
            div().flex_1().h_full().rounded(px(1.5)).bg(color)
        }))
}

/// Text that can be selected with the mouse and copied, identified by
/// where it is written and what it says. For text repeated per row (one
/// call site, several equal texts) use [`selectable`] with the row's id.
#[track_caller]
fn text(content: impl Into<SharedString>) -> SelectableText {
    let content = content.into();
    let at = std::panic::Location::caller();
    selectable(
        format!("text-{}:{}-{content}", at.line(), at.column()),
        content,
    )
}

/// Text the user can select with the mouse and copy (Ctrl+C, Cmd+C).
///
/// Let it wrap rather than cutting it: gpui-base can't select text cut
/// with `.truncate()`, and `.line_clamp(1)` cuts it without an ellipsis.
fn selectable(id: impl Into<SharedString>, text: impl Into<SharedString>) -> SelectableText {
    SelectableText::new(id.into(), text)
}

/// A row of wrapping text (a transfer's name, the drop hint) followed by
/// fixed-width columns (amount, status, buttons). When the text would get
/// less than [`NAME_MIN_W`], the columns move to a line of their own below
/// it, and they wrap among themselves if even that line is too narrow.
/// Without this a narrow window squeezes the text to one letter per line.
fn wrapping_row(name: Div, columns: impl IntoIterator<Item = Div>) -> Div {
    h_flex()
        .flex_1()
        .min_w_0()
        .flex_wrap()
        .gap_x(px(14.))
        .gap_y(px(6.))
        .child(
            name.flex_grow_1()
                .flex_shrink_1()
                .flex_basis(px(NAME_MIN_W))
                .min_w_0(),
        )
        .child(h_flex().flex_wrap().gap(px(14.)).children(columns))
}

/// The narrowest a transfer's name gets before the columns beside it wrap.
const NAME_MIN_W: f32 = 180.;

/// An amount set wide, with its unit (and the total) small beside it.
fn amount(id: &str, big: String, small: String, p: &Palette) -> Div {
    h_flex()
        .justify_end()
        .items_baseline()
        .gap(px(4.))
        .whitespace_nowrap()
        .child(
            div()
                .font_family(style::FONT_WIDE)
                .text_size(px(15.))
                .text_color(p.ink)
                .font_features(tabular())
                .child(selectable(format!("amount-{id}"), big)),
        )
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(p.ink_3)
                .font_features(tabular())
                .child(selectable(format!("unit-{id}"), small)),
        )
}

/// `("1.1", "of 3.2 GB")` for progress, `("3.2", "GB")` for a size: both
/// numbers in the total's unit.
fn amount_parts(done: Option<u64>, total: u64) -> (String, String) {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut scale = 1.0;
    let mut unit = 0;
    while total as f64 / scale >= 1000.0 && unit < UNITS.len() - 1 {
        scale *= 1000.0;
        unit += 1;
    }
    let fmt = |n: u64| {
        if unit == 0 {
            n.to_string()
        } else {
            format!("{:.1}", n as f64 / scale)
        }
    };
    match done {
        Some(done) => (fmt(done), s::of_total(&fmt(total), UNITS[unit])),
        None => (fmt(total), UNITS[unit].to_owned()),
    }
}

/// Selects the first location that still exists in the file manager, or
/// opens `fallback` (the destination folder); says so if neither is there.
fn reveal(
    locations: &[String],
    fallback: Option<&str>,
    window: &mut Window,
    cx: &mut gpui_kit::App,
) {
    let existing = |p: &str| {
        // Explorer can't select a `\\?\` path.
        let p = PathBuf::from(p.strip_prefix(r"\\?\").unwrap_or(p));
        p.exists().then_some(p)
    };
    if let Some(path) = locations.iter().find_map(|l| existing(l)) {
        tracing::info!("showing a transfer's files in the file manager");
        cx.reveal_path(&path);
    } else if let Some(dir) = fallback.and_then(existing) {
        cx.open_with_system(&dir);
    } else {
        window.push_notification(Notification::warning(s::NOT_FOUND), cx);
    }
}

fn local_time(secs: i64) -> Option<chrono::DateTime<chrono::Local>> {
    use chrono::TimeZone;
    chrono::Local.timestamp_opt(secs, 0).single()
}

fn day_label(day: chrono::NaiveDate, today: chrono::NaiveDate) -> String {
    use chrono::Datelike;
    if day == today {
        s::TODAY.to_owned()
    } else if today.pred_opt() == Some(day) {
        s::YESTERDAY.to_owned()
    } else if day.year() == today.year() {
        day.format("%-d %B").to_string()
    } else {
        day.format("%-d %B %Y").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_use_the_total_unit() {
        assert_eq!(
            amount_parts(Some(1_100_000_000), 3_200_000_000),
            ("1.1".into(), "of 3.2 GB".into())
        );
        assert_eq!(
            amount_parts(Some(900_000), 3_200_000_000),
            ("0.0".into(), "of 3.2 GB".into())
        );
        assert_eq!(
            amount_parts(None, 860_000_000),
            ("860.0".into(), "MB".into())
        );
        assert_eq!(amount_parts(None, 512), ("512".into(), "B".into()));
    }

    #[test]
    fn relay_names() {
        assert_eq!(
            relay_short_name("https://use1-1.relay.n0.iroh-canary.iroh.link./"),
            "use1-1"
        );
        assert_eq!(relay_short_name("euc1-1.relay.n0.iroh.link"), "euc1-1");
    }

    #[test]
    fn day_labels() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 11, 12).unwrap();
        assert_eq!(day_label(today, today), s::TODAY);
        assert_eq!(day_label(today.pred_opt().unwrap(), today), s::YESTERDAY);
        let earlier = chrono::NaiveDate::from_ymd_opt(2026, 11, 10).unwrap();
        assert_eq!(day_label(earlier, today), "10 November");
        let last_year = chrono::NaiveDate::from_ymd_opt(2025, 3, 4).unwrap();
        assert_eq!(day_label(last_year, today), "4 March 2025");
    }
}
