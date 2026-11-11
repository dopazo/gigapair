//! The main window: contacts on the left; on the right, the selected
//! contact's drop zone and the transfer queue, or pairing, history or
//! settings. Everything goes through the service (no business logic here).

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use gigapair_core::api::{Event, RELAY_NOTICE, Request, Response, StatusInfo};
use gigapair_core::dirs::AppDirs;
use gigapair_core::types::{
    ConnKind, Contact, Direction, PendingPair, TransferInfo, TransferStatus, format_bytes,
    format_duration, now_secs,
};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme, Disableable, Sizable, Theme, WindowExt, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, AppContext, ClipboardItem, Context, Entity, ExternalPaths, FontWeight,
    InteractiveElement, IntoElement, ParentElement, PathPromptOptions, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::backend::{Backend, BackendMsg};
use crate::qr;
use crate::shell;
use crate::strings as s;

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
    remove_armed: bool,
    start_at_login: Option<bool>,
    code_input: Entity<InputState>,
    pair_name_inputs: HashMap<String, Entity<InputState>>,
    rename_input: Entity<InputState>,
    device_input: Entity<InputState>,
    relay_input: Entity<InputState>,
    limit_input: Entity<InputState>,
}

type Done = Box<dyn FnOnce(&mut GigapairApp, Response, &mut Window, &mut Context<GigapairApp>)>;

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

        cx.observe_window_appearance(window, |_, window, cx| {
            Theme::sync_system_appearance(Some(window), cx);
        })
        .detach();

        let input = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };

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
            remove_armed: false,
            start_at_login: gigapair_core::service::status().ok().map(|s| s.installed),
            code_input: input(s::CODE_PLACEHOLDER, window, cx),
            pair_name_inputs: HashMap::new(),
            rename_input: input(s::NAME_PLACEHOLDER, window, cx),
            device_input: input(s::SETTINGS_DEVICE, window, cx),
            relay_input: input("https://relay.example.com", window, cx),
            limit_input: input("0", window, cx),
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

    /// Fire-and-forget request; errors are shown.
    fn act(&mut self, req: Request, window: &mut Window, cx: &mut Context<Self>) {
        self.call(req, window, cx, |_, _, _, _| {});
    }

    fn notify_error(&mut self, msg: String, window: &mut Window, cx: &mut Context<Self>) {
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
            Request::History { limit: 200 },
            window,
            cx,
            |app, r, _, cx| {
                if let Response::Transfers(list) = r {
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

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mut list = v_flex().gap_1().flex_1();
        if self.contacts.is_empty() {
            list = list.child(
                div()
                    .px_2()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(s::NO_CONTACTS),
            );
        }
        for c in &self.contacts {
            let selected = self.page == Page::Contact(c.node_id.clone());
            let id = c.node_id.clone();
            let pending = self
                .transfers
                .iter()
                .filter(|t| t.peer_id == c.node_id && t.status == TransferStatus::PendingAccept)
                .count();
            list = list.child(
                h_flex()
                    .id(SharedString::from(format!("contact-{}", c.node_id)))
                    .gap_2()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .when(selected, |d| d.bg(theme.sidebar_accent))
                    .hover(|d| d.bg(theme.sidebar_accent))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.page = Page::Contact(id.clone());
                        app.remove_armed = false;
                        cx.notify();
                    }))
                    .child(div().size(px(8.)).rounded_full().bg(if c.online {
                        theme.success
                    } else {
                        theme.muted_foreground
                    }))
                    .child(div().flex_1().text_sm().truncate().child(c.name.clone()))
                    .when(pending > 0, |d| {
                        d.child(Tag::warning().small().child(pending.to_string()))
                    }),
            );
        }

        let nav = |id: &'static str, label: &'static str, page: Page, cx: &mut Context<Self>| {
            let active = self.page == page;
            Button::new(id)
                .label(label)
                .ghost()
                .small()
                .w_full()
                .toggled(active)
                .on_click(cx.listener(move |app, _, _, cx| {
                    app.page = page.clone();
                    cx.notify();
                }))
        };

        v_flex()
            .w(px(240.))
            .h_full()
            .flex_shrink_0()
            .p_3()
            .gap_3()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(
                v_flex()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(s::APP_NAME),
                    )
                    .child(
                        div().text_xs().text_color(theme.muted_foreground).child(
                            self.status
                                .as_ref()
                                .map(|st| st.device_name.clone())
                                .unwrap_or_default(),
                        ),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground)
                    .child(s::CONTACTS),
            )
            .child(list)
            .child(
                Button::new("add-contact")
                    .label(s::ADD_CONTACT)
                    .primary()
                    .small()
                    .w_full()
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.page = Page::AddContact;
                        cx.notify();
                    })),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(nav("nav-history", s::HISTORY, Page::History, cx))
                    .child(nav("nav-settings", s::SETTINGS, Page::Settings, cx)),
            )
            .when(
                self.status.as_ref().is_some_and(|st| st.keeping_awake),
                |d| d.child(Tag::success().small().child(s::KEEPING_AWAKE)),
            )
    }

    fn render_transfer(&self, t: &TransferInfo, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let id = t.id.clone();
        let arrow = match t.direction {
            Direction::Send => format!("→ {}", t.peer_name),
            Direction::Receive => format!("← {}", t.peer_name),
        };
        let mut details = vec![
            format!(
                "{} / {}",
                format_bytes(t.done_bytes),
                format_bytes(t.total_bytes)
            ),
            t.status.label().to_owned(),
        ];
        if t.speed_bps > 0 {
            details.push(format!("{}/s", format_bytes(t.speed_bps)));
        }
        if let Some(eta) = t.eta_secs {
            details.push(format!("ETA {}", format_duration(eta)));
        }

        let conn_tag = t.conn.as_ref().map(|c| match c.kind {
            ConnKind::Direct => Tag::success().small().child(s::DIRECT).into_any_element(),
            ConnKind::Relay => Tag::warning()
                .small()
                .child(match &c.relay {
                    Some(r) => format!("{} ({r})", s::VIA_RELAY),
                    None => s::VIA_RELAY.to_owned(),
                })
                .into_any_element(),
        });

        let mut buttons = h_flex().gap_1();
        match t.status {
            TransferStatus::PendingAccept => {
                let (a, b, c) = (id.clone(), id.clone(), id.clone());
                buttons = buttons
                    .child(
                        Button::new(SharedString::from(format!("accept-{id}")))
                            .label(s::ACCEPT)
                            .primary()
                            .small()
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
                    )
                    .child(
                        Button::new(SharedString::from(format!("accept-to-{id}")))
                            .label(s::ACCEPT_TO)
                            .small()
                            .on_click(cx.listener(move |app, _, window, cx| {
                                app.accept_to(b.clone(), window, cx)
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("reject-{id}")))
                            .label(s::REJECT)
                            .danger()
                            .small()
                            .on_click(cx.listener(move |app, _, window, cx| {
                                app.act(Request::Reject { id: c.clone() }, window, cx)
                            })),
                    );
            }
            s if !s.is_terminal() => {
                let (a, b) = (id.clone(), id.clone());
                let paused = s == TransferStatus::Paused;
                buttons = buttons
                    .child(
                        Button::new(SharedString::from(format!("pause-{id}")))
                            .label(if paused { s::RESUME } else { s::PAUSE })
                            .small()
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
                            .small()
                            .on_click(cx.listener(move |app, _, window, cx| {
                                app.act(Request::Cancel { id: b.clone() }, window, cx)
                            })),
                    );
            }
            _ => {
                if let Some(dir) = t
                    .dest_dir
                    .clone()
                    .filter(|_| t.direction == Direction::Receive)
                {
                    buttons = buttons.child(
                        Button::new(SharedString::from(format!("open-{id}")))
                            .label(s::OPEN_FOLDER)
                            .small()
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.open_with_system(std::path::Path::new(&dir))
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
                            .small()
                            .on_click(cx.listener(move |app, _, window, cx| {
                                app.act(Request::Retry { id: a.clone() }, window, cx)
                            })),
                    );
                }
            }
        }

        let mut row = v_flex()
            .gap_1p5()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .child(t.label.clone()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(arrow),
                    )
                    .children(conn_tag),
            );
        if !t.status.is_terminal() && t.status != TransferStatus::PendingAccept {
            row = row.child(
                Progress::new(SharedString::from(format!("progress-{id}")))
                    .value(t.percent() as f32),
            );
        }
        row = row.child(
            h_flex()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(details.join(" · ")),
                )
                .child(buttons),
        );
        if let Some(note) = t.note.clone().filter(|_| !t.status.is_terminal()) {
            row = row.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(note),
            );
        }
        if let Some(err) = t
            .error
            .clone()
            .filter(|_| t.status != TransferStatus::Completed)
        {
            row = row.child(div().text_xs().text_color(theme.danger).child(err));
        }
        row.into_any_element()
    }

    fn section_title(title: &str, cx: &App) -> impl IntoElement {
        div()
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(cx.theme().muted_foreground)
            .child(title.to_owned())
    }

    fn render_queue(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let requests: Vec<_> = self
            .transfers
            .iter()
            .filter(|t| t.status == TransferStatus::PendingAccept)
            .cloned()
            .collect();
        let active: Vec<_> = self
            .transfers
            .iter()
            .filter(|t| t.status != TransferStatus::PendingAccept)
            .cloned()
            .collect();
        let mut col = v_flex().gap_3();
        if !requests.is_empty() {
            col = col.child(Self::section_title(s::INCOMING, cx));
            for t in &requests {
                col = col.child(self.render_transfer(t, cx));
            }
        }
        col = col.child(Self::section_title(s::TRANSFERS, cx));
        if active.is_empty() {
            col = col.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(s::NO_TRANSFERS),
            );
        }
        for t in &active {
            col = col.child(self.render_transfer(t, cx));
        }
        col
    }

    fn render_contact_page(&mut self, node_id: &str, cx: &mut Context<Self>) -> AnyElement {
        let Some(c) = self.contacts.iter().find(|c| c.node_id == node_id).cloned() else {
            return div().child(s::SELECT_CONTACT).into_any_element();
        };
        let theme = cx.theme().clone();
        let (id_files, id_folder, id_drop) =
            (c.node_id.clone(), c.node_id.clone(), c.node_id.clone());
        let id_trust = c.node_id.clone();
        let id_rename = c.node_id.clone();
        let id_remove = c.node_id.clone();
        let armed = self.remove_armed;

        let drop_zone = v_flex()
            .id("drop-zone")
            .items_center()
            .justify_center()
            .gap_3()
            .h(px(170.))
            .rounded_xl()
            .border_2()
            .border_dashed()
            .border_color(theme.border)
            .drag_over::<ExternalPaths>(|style, _, _, cx| {
                style
                    .bg(cx.theme().drop_target)
                    .border_color(cx.theme().drag_border)
            })
            .on_drop(cx.listener(move |app, paths: &ExternalPaths, window, cx| {
                app.send_paths(id_drop.clone(), paths.paths().to_vec(), window, cx)
            }))
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::MEDIUM)
                    .child(s::DROP_HINT),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(Button::new("choose-files").label(s::CHOOSE_FILES).on_click(
                        cx.listener(move |app, _, window, cx| {
                            app.pick_and_send(id_files.clone(), false, window, cx)
                        }),
                    ))
                    .child(
                        Button::new("choose-folder")
                            .label(s::CHOOSE_FOLDER)
                            .on_click(cx.listener(move |app, _, window, cx| {
                                app.pick_and_send(id_folder.clone(), true, window, cx)
                            })),
                    ),
            );

        v_flex()
            .gap_4()
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        v_flex()
                            .flex_1()
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(s::send_to(&c.name)),
                            )
                            .child(div().text_xs().text_color(theme.muted_foreground).child(
                                format!(
                                    "{} · {}",
                                    if c.online { s::ONLINE } else { s::OFFLINE },
                                    c.node_id
                                ),
                            )),
                    )
                    .child(
                        Button::new("remove-contact")
                            .label(if armed { s::REMOVE_CONFIRM } else { s::REMOVE })
                            .danger()
                            .small()
                            .on_click(cx.listener(move |app, _, window, cx| {
                                if app.remove_armed {
                                    app.remove_armed = false;
                                    app.act(
                                        Request::ContactRemove {
                                            contact: id_remove.clone(),
                                        },
                                        window,
                                        cx,
                                    );
                                } else {
                                    app.remove_armed = true;
                                }
                                cx.notify();
                            })),
                    ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        Switch::new("auto-accept")
                            .checked(c.auto_accept)
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
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .w(px(180.))
                            .child(Input::new(&self.rename_input).small()),
                    )
                    .child(
                        Button::new("rename")
                            .label(s::RENAME)
                            .small()
                            .on_click(cx.listener(move |app, _, window, cx| {
                                let name = app.rename_input.read(cx).value().to_string();
                                if !name.trim().is_empty() {
                                    app.act(
                                        Request::ContactRename {
                                            contact: id_rename.clone(),
                                            name,
                                        },
                                        window,
                                        cx,
                                    );
                                }
                            })),
                    ),
            )
            .child(drop_zone)
            .child(self.render_queue(cx))
            .into_any_element()
    }

    fn render_pair_confirm(&self, p: &PendingPair, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let (id_ok, id_no) = (p.id.clone(), p.id.clone());
        let input = self.pair_name_inputs.get(&p.id).cloned();
        let mut card = v_flex()
            .gap_3()
            .p_4()
            .rounded_lg()
            .border_1()
            .border_color(theme.primary)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(s::CONFIRM_TITLE),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(format!("{} — {}", p.remote_name, s::CONFIRM_TEXT)),
            )
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .child(p.fingerprint.to_uppercase()),
            );
        if let Some(input) = &input {
            card = card.child(div().w(px(280.)).child(Input::new(input)));
        }
        let input_for_ok = input.clone();
        card.child(
            h_flex()
                .gap_2()
                .child(
                    Button::new(SharedString::from(format!("pair-ok-{}", p.id)))
                        .label(s::WORDS_MATCH)
                        .primary()
                        .on_click(cx.listener(move |app, _, window, cx| {
                            let name = input_for_ok
                                .as_ref()
                                .map(|i| i.read(cx).value().to_string())
                                .unwrap_or_default();
                            let id = id_ok.clone();
                            app.call(
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
                        })),
                )
                .child(
                    Button::new(SharedString::from(format!("pair-no-{}", p.id)))
                        .label(s::REJECT)
                        .danger()
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
        let theme = cx.theme().clone();
        let mut col = v_flex().gap_5().child(
            div()
                .text_xl()
                .font_weight(FontWeight::SEMIBOLD)
                .child(s::ADD_CONTACT),
        );
        for p in self.pending_pairs.clone() {
            col = col.child(self.render_pair_confirm(&p, cx));
        }

        // Share our code.
        let mut share = v_flex()
            .gap_3()
            .p_4()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(s::SHARE_CODE_TITLE),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(s::SHARE_CODE_TEXT),
            );
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
                        .gap_4()
                        .items_start()
                        .children(qr::qr(&code))
                        .child(
                            v_flex()
                                .gap_2()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    // Groups separated by spaces so the code
                                    // wraps; it is parsed either way.
                                    div()
                                        .p_2()
                                        .rounded_md()
                                        .bg(theme.muted)
                                        .font_family("monospace")
                                        .text_sm()
                                        .child(code.replace('-', " ")),
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(
                                            Button::new("copy-code")
                                                .label(s::COPY)
                                                .small()
                                                .on_click(cx.listener(move |_, _, window, cx| {
                                                    cx.write_to_clipboard(
                                                        ClipboardItem::new_string(
                                                            copy_code.clone(),
                                                        ),
                                                    );
                                                    window.push_notification(
                                                        Notification::success(s::COPIED),
                                                        cx,
                                                    );
                                                })),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.muted_foreground)
                                                .child(s::expires_in(left)),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(theme.muted_foreground)
                                        .child(s::WAITING_OTHER),
                                ),
                        ),
                );
            }
        }
        col = col.child(share);

        // Use the other device's code.
        let code_input = self.code_input.clone();
        col = col.child(
            v_flex()
                .gap_3()
                .p_4()
                .rounded_lg()
                .border_1()
                .border_color(theme.border)
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(s::USE_CODE_TITLE),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(s::USE_CODE_TEXT),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().flex_1().child(Input::new(&self.code_input)))
                        .child(Button::new("join").label(s::CONNECT).primary().on_click(
                            cx.listener(move |app, _, window, cx| {
                                let code = code_input.read(cx).value().to_string();
                                if code.trim().is_empty() {
                                    return;
                                }
                                app.call(
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
                            }),
                        )),
                ),
        );
        col.into_any_element()
    }

    fn render_history(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let mut col = v_flex().gap_3().child(
            h_flex()
                .child(
                    div()
                        .flex_1()
                        .text_xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(s::HISTORY),
                )
                .child(
                    Button::new("clear-history")
                        .label(s::CLEAR_HISTORY)
                        .ghost()
                        .small()
                        .disabled(self.history.is_empty())
                        .on_click(cx.listener(|app, _, window, cx| {
                            app.call(Request::ClearHistory, window, cx, |app, _, _, cx| {
                                app.history.clear();
                                cx.notify();
                            })
                        })),
                ),
        );
        if self.history.is_empty() {
            col = col.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(s::NO_HISTORY),
            );
        }
        for t in self.history.clone() {
            col = col.child(self.render_transfer(&t, cx));
        }
        col.into_any_element()
    }

    fn render_settings(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let switch = |id: &'static str,
                      label: &'static str,
                      key: &'static str,
                      app: &Self,
                      cx: &mut Context<Self>| {
            Switch::new(id)
                .checked(app.config_bool(key))
                .label(label)
                .on_click(cx.listener(move |app, on: &bool, window, cx| {
                    app.set_config(key, on.to_string(), window, cx)
                }))
        };
        let row =
            |label: &'static str| div().text_sm().font_weight(FontWeight::MEDIUM).child(label);
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
        let (device, relay, limit) = (
            self.device_input.clone(),
            self.relay_input.clone(),
            self.limit_input.clone(),
        );

        v_flex()
            .gap_4()
            .max_w(px(640.))
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(s::SETTINGS),
            )
            .child(row(s::SETTINGS_DOWNLOADS))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().text_sm().truncate().child(download))
                    .child(
                        Button::new("change-download")
                            .label(s::CHANGE)
                            .small()
                            .on_click(cx.listener(|app, _, window, cx| {
                                app.choose_download_dir(window, cx)
                            })),
                    ),
            )
            .child(row(s::SETTINGS_DEVICE))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&self.device_input)))
                    .child(Button::new("save-device").label(s::SAVE).small().on_click(
                        cx.listener(move |app, _, window, cx| {
                            let v = device.read(cx).value().to_string();
                            app.set_config("device_name", v, window, cx)
                        }),
                    )),
            )
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
                    .label(s::SETTINGS_START_LOGIN)
                    .on_click(cx.listener(|app, on: &bool, window, cx| {
                        app.toggle_start_at_login(*on, window, cx)
                    })),
            )
            .child(switch(
                "sw-direct",
                s::SETTINGS_DIRECT_ONLY,
                "direct_only",
                self,
                cx,
            ))
            .child(switch(
                "sw-discovery",
                s::SETTINGS_DISCOVERY,
                "discovery",
                self,
                cx,
            ))
            .child(row(s::SETTINGS_RELAY))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&self.relay_input)))
                    .child(
                        Button::new("save-relay")
                            .label(s::SAVE)
                            .small()
                            .on_click(cx.listener(move |app, _, window, cx| {
                                let v = relay.read(cx).value().to_string();
                                app.set_config("relay_url", v, window, cx)
                            })),
                    ),
            )
            .child(row(s::SETTINGS_LIMIT))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().w(px(160.)).child(Input::new(&self.limit_input)))
                    .child(
                        Button::new("save-limit")
                            .label(s::SAVE)
                            .small()
                            .on_click(cx.listener(move |app, _, window, cx| {
                                let v = limit.read(cx).value().to_string();
                                app.set_config("upload_limit_kib", v, window, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(s::RESTART_NOTE),
            )
            .child(row(s::SETTINGS_NODE))
            .child(
                div()
                    .text_xs()
                    .font_family("monospace")
                    .text_color(theme.muted_foreground)
                    .child(node),
            )
            .into_any_element()
    }
}

impl Render for GigapairApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
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
            .p_6()
            .gap_4()
            .overflow_y_scroll();
        if !self.connected {
            main = main.child(
                div()
                    .p_2()
                    .rounded_md()
                    .bg(theme.warning)
                    .text_color(theme.warning_foreground)
                    .text_sm()
                    .child(match &self.last_error {
                        Some(e) => format!("{} ({e})", s::CONNECTING_SERVICE),
                        None => s::CONNECTING_SERVICE.to_owned(),
                    }),
            );
        }
        if let Some(relay) = self.relay_banner.clone() {
            main = main.child(
                v_flex()
                    .gap_2()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.warning)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("{} ({relay})", s::RELAY_SLOWER)),
                    )
                    .child(div().text_sm().child(RELAY_NOTICE))
                    .child(
                        Button::new("dismiss-relay")
                            .label(s::DISMISS)
                            .small()
                            .on_click(cx.listener(|app, _, window, cx| {
                                app.relay_banner = None;
                                app.act(Request::AckRelayNotice, window, cx);
                                cx.notify();
                            })),
                    ),
            );
        }
        main = main.child(div().w_full().max_w(px(900.)).child(page));

        h_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(self.render_sidebar(cx))
            .child(main)
    }
}
