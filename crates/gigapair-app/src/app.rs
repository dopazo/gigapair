//! The main window: contacts on the left; on the right, the selected
//! contact's drop zone and the transfer queue, or pairing, history or
//! settings. Everything goes through the service (no business logic here).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gigapair_core::api::{Event, RELAY_NOTICE, Request, Response, StatusInfo};
use gigapair_core::dirs::AppDirs;
use gigapair_core::types::{
    ConnInfo, ConnKind, Contact, Direction, PendingPair, TransferInfo, TransferStatus,
    format_bytes, format_duration, now_secs,
};
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Disableable, Sizable, Theme, WindowExt, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, AppContext, ClipboardItem, Context, Div, Entity, ExternalPaths, FontFeatures,
    FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, PathPromptOptions, Render,
    SharedString, Stateful, StatefulInteractiveElement, Styled, Svg, Window, div, px, svg,
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
    remove_armed: bool,
    /// The contact page shows the rename field.
    renaming: bool,
    start_at_login: Option<bool>,
    /// Where the service and the app write their log files.
    logs_dir: Option<PathBuf>,
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
            renaming: false,
            start_at_login: gigapair_core::service::status().ok().map(|s| s.installed),
            logs_dir: AppDirs::from_env().ok().map(|d| d.logs),
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
                                    .child(s::APP_NAME),
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
            col = col.child(
                nav_row(
                    SharedString::from(format!("contact-{}", c.node_id)),
                    selected,
                    &p,
                )
                .on_click(cx.listener(move |app, _, _, cx| {
                    app.page = Page::Contact(id.clone());
                    app.remove_armed = false;
                    app.renaming = false;
                    cx.notify();
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
                .on_click(cx.listener(move |app, _, _, cx| {
                    app.page = page.clone();
                    cx.notify();
                }))
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
                    .child(s::KEEPING_AWAKE),
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
        if let Some(dir) = t
            .dest_dir
            .clone()
            .filter(|_| t.direction == Direction::Receive)
        {
            buttons = buttons.child(
                Button::new(SharedString::from(format!("open-{id}")))
                    .label(s::OPEN_FOLDER)
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

        let rate = if status == TransferStatus::Transferring && t.speed_bps > 0 {
            v_flex()
                .items_end()
                .text_size(px(12.))
                .line_height(px(15.))
                .child(
                    div()
                        .text_color(p.ink)
                        .font_weight(FontWeight::SEMIBOLD)
                        .font_features(tabular())
                        .child(format!("{}/s", format_bytes(t.speed_bps))),
                )
                .when_some(t.eta_secs, |d, eta| {
                    d.child(
                        div()
                            .text_color(p.ink_2)
                            .font_features(tabular())
                            .child(s::time_left(&format_duration(eta))),
                    )
                })
        } else {
            v_flex().items_end().child(
                div()
                    .text_size(px(12.))
                    .text_color(p.ink_2)
                    .text_right()
                    .child(status.label()),
            )
        };

        let (big, small) = if mode.is_some() && status != TransferStatus::Preparing {
            amount_parts(Some(t.done_bytes), t.total_bytes)
        } else {
            amount_parts(None, t.total_bytes)
        };

        let row = h_flex()
            .gap(px(14.))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if waiting { p.ink_2 } else { p.ink })
                            .child(t.label.clone()),
                    )
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .min_w_0()
                            .overflow_hidden()
                            .text_size(px(12.))
                            .text_color(p.ink_3)
                            .child(div().flex_shrink_0().child(s::files(t.file_count)))
                            .children(t.conn.as_ref().map(|c| conn_tag(&t.id, c, &p))),
                    ),
            )
            .child(
                div()
                    .w(px(150.))
                    .flex_shrink_0()
                    .when_some(mode, |d, mode| d.child(strip(percent, mode, &p))),
            )
            .child(
                div()
                    .w(px(34.))
                    .flex_shrink_0()
                    .text_right()
                    .text_size(px(12.))
                    .text_color(p.ink_2)
                    .font_features(tabular())
                    .when(mode.is_some(), |d| d.child(format!("{}%", percent.floor()))),
            )
            .child(
                div()
                    .w(px(112.))
                    .flex_shrink_0()
                    .child(amount(big, small, &p)),
            )
            .child(div().w(px(74.)).flex_shrink_0().child(rate))
            .child(
                div()
                    .w(px(132.))
                    .flex_shrink_0()
                    .child(self.transfer_actions(t, cx)),
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
        h_flex()
            .gap(px(12.))
            .pl(px(12.))
            .pr(px(8.))
            .py(px(8.))
            .rounded(px(8.))
            .bg(p.ink)
            .text_color(p.bg)
            .child(dir_glyph(Direction::Receive, 16., p.bg, &p))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .gap(px(4.))
                    .whitespace_nowrap()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t.peer_name.clone()),
                    )
                    .child(s::WANTS_TO_SEND)
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t.label.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .opacity(0.7)
                            .font_features(tabular())
                            .child(format!(
                                "{}, {}",
                                format_bytes(t.total_bytes),
                                s::files(t.file_count)
                            )),
                    ),
            )
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
            )
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
            col = col.child(
                div()
                    .mt(px(8.))
                    .text_size(px(12.))
                    .text_color(p.ink_3)
                    .child(s::NO_TRANSFERS),
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
                        .child(title)
                        .child(
                            div()
                                .text_color(p.ink_3)
                                .font_weight(FontWeight::NORMAL)
                                .child(list.len().to_string()),
                        ),
                )
                .child(rows);
        }
        col
    }

    fn render_contact_page(&mut self, node_id: &str, cx: &mut Context<Self>) -> AnyElement {
        let Some(c) = self.contacts.iter().find(|c| c.node_id == node_id).cloned() else {
            return div().child(s::SELECT_CONTACT).into_any_element();
        };
        let p = Palette::get(cx);
        let (id_files, id_folder, id_drop) =
            (c.node_id.clone(), c.node_id.clone(), c.node_id.clone());
        let id_trust = c.node_id.clone();
        let id_rename = c.node_id.clone();
        let id_remove = c.node_id.clone();
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
            }));

        let rename: AnyElement = if self.renaming {
            let save_id = id_rename.clone();
            h_flex()
                .gap(px(6.))
                .child(div().w(px(170.)).child(Input::new(&self.rename_input)))
                .child(
                    Button::new("rename-save")
                        .label(s::SAVE)
                        .on_click(cx.listener(move |app, _, window, cx| {
                            let name = app.rename_input.read(cx).value().to_string();
                            if !name.trim().is_empty() {
                                app.act(
                                    Request::ContactRename {
                                        contact: save_id.clone(),
                                        name,
                                    },
                                    window,
                                    cx,
                                );
                            }
                            app.renaming = false;
                            cx.notify();
                        })),
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
                    app.rename_input
                        .update(cx, |i, cx| i.set_value(name, window, cx));
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
            .drag_over::<ExternalPaths>(|style, _, _, cx| {
                let p = Palette::get(cx);
                style.bg(p.signal.opacity(0.1)).border_color(p.ink)
            })
            .on_drop(cx.listener(move |app, paths: &ExternalPaths, window, cx| {
                app.send_paths(id_drop.clone(), paths.paths().to_vec(), window, cx)
            }))
            .child(
                h_flex()
                    .gap(px(3.))
                    .flex_shrink_0()
                    .children((0..6).map(|_| glyph(style::icon::CELL, 12., p.ink_3))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(s::drop_hint(&c.name)),
            )
            .child(
                h_flex()
                    .gap(px(6.))
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

        let meta = h_flex()
            .gap(px(10.))
            .min_w_0()
            .items_baseline()
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(12.))
                    .text_color(p.ink_2)
                    .child(if c.online { s::ONLINE } else { s::OFFLINE }),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(11.5))
                    .text_color(p.ink_3)
                    .child(c.node_id.clone()),
            );

        v_flex()
            .gap(px(10.))
            .child(page_head(
                &s::send_to(&c.name),
                Some(meta.into_any_element()),
                Some(remove.into_any_element()),
                &p,
            ))
            .child(
                h_flex()
                    .gap(px(12.))
                    .child(
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
                    )
                    .child(div().flex_1())
                    .child(rename),
            )
            .child(drop_zone)
            .child(self.render_queue(node_id, cx))
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
                    .child(s::CONFIRM_TITLE),
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
                    .child(pp.fingerprint.to_uppercase()),
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
        let input_for_ok = input.clone();
        card.child(
            h_flex()
                .gap(px(6.))
                .child(
                    Button::new(SharedString::from(format!("pair-ok-{}", pp.id)))
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
                                        .child(code.replace('-', " ")),
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
                                                .child(s::expires_in(left)),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(p.ink_2)
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
            section(s::USE_CODE_TITLE, s::USE_CODE_TEXT, &p).child(
                h_flex()
                    .gap(px(6.))
                    .child(div().flex_1().child(Input::new(&self.code_input)))
                    .child(Button::new("join").label(s::CONNECT).on_click(cx.listener(
                        move |app, _, window, cx| {
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
                        },
                    ))),
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
        let (big, small) = amount_parts(None, t.total_bytes);
        let when = local_time(t.finished_at.unwrap_or(t.created_at))
            .map(|d| d.format("%H:%M").to_string())
            .unwrap_or_default();
        let who = match t.direction {
            Direction::Send => s::sent_to(&t.peer_name, t.file_count),
            Direction::Receive => s::received_from(&t.peer_name, t.file_count),
        };

        h_flex()
            .gap(px(14.))
            .py(px(8.))
            .px(px(2.))
            .border_t_1()
            .border_color(p.line)
            .child(dir_glyph(t.direction, 16., p.ink, &p))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t.label.clone()),
                    )
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .min_w_0()
                            .overflow_hidden()
                            .text_size(px(12.))
                            .text_color(p.ink_3)
                            .child(div().flex_shrink_0().child(who))
                            .children(t.conn.as_ref().map(|c| conn_tag(&t.id, c, &p))),
                    ),
            )
            .child(
                div()
                    .w(px(96.))
                    .flex_shrink_0()
                    .child(amount(big, small, &p)),
            )
            .child(
                v_flex()
                    .w(px(220.))
                    .flex_shrink_0()
                    .text_size(px(12.))
                    .line_height(px(16.))
                    .child(div().text_color(color).child(headline))
                    .when_some(detail, |d, detail| {
                        d.child(div().text_color(p.ink_2).line_clamp(2).child(detail))
                    }),
            )
            .child(
                div()
                    .w(px(40.))
                    .flex_shrink_0()
                    .text_right()
                    .text_size(px(12.))
                    .text_color(p.ink_3)
                    .font_features(tabular())
                    .child(when),
            )
            .child(
                div()
                    .w(px(96.))
                    .flex_shrink_0()
                    .child(self.transfer_actions(t, cx)),
            )
            .into_any_element()
    }

    fn render_history(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = Palette::get(cx);
        let clear = Button::new("clear-history")
            .label(s::CLEAR_HISTORY)
            .ghost()
            .disabled(self.history.is_empty())
            .on_click(cx.listener(|app, _, window, cx| {
                app.call(Request::ClearHistory, window, cx, |app, _, _, cx| {
                    app.history.clear();
                    cx.notify();
                })
            }));
        let meta = div()
            .text_size(px(12.))
            .text_color(p.ink_2)
            .child(s::HISTORY_META);
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
                        .child(s::NO_HISTORY),
                )
                .into_any_element();
        }

        let today = chrono::Local::now().date_naive();
        let mut day: Option<chrono::NaiveDate> = None;
        let mut rows: Option<Div> = None;
        for t in self.history.clone() {
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
                        .child(date.map(|d| day_label(d, today)).unwrap_or_default()),
                );
                rows = Some(v_flex().border_b_1().border_color(p.line));
            }
            let row = self.render_history_row(&t, cx);
            rows = rows.map(|r| r.child(row));
        }
        if let Some(r) = rows {
            col = col.child(r);
        }
        col.into_any_element()
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
                .child(text)
        };
        let note = |text: &'static str| div().text_size(px(12.)).text_color(p.ink_3).child(text);
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
        let (device, relay, limit) = (
            self.device_input.clone(),
            self.relay_input.clone(),
            self.limit_input.clone(),
        );

        v_flex()
            .gap(px(8.))
            .max_w(px(640.))
            .child(page_head(s::SETTINGS, None, None, &p))
            .child(label(s::SETTINGS_DOWNLOADS))
            .child(
                h_flex()
                    .gap(px(6.))
                    .child(div().flex_1().min_w_0().truncate().child(download))
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
                            .on_click(cx.listener(move |app, _, window, cx| {
                                let v = device.read(cx).value().to_string();
                                app.set_config("device_name", v, window, cx)
                            })),
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
                            .on_click(cx.listener(move |app, _, window, cx| {
                                let v = relay.read(cx).value().to_string();
                                app.set_config("relay_url", v, window, cx)
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
                            .on_click(cx.listener(move |app, _, window, cx| {
                                let v = limit.read(cx).value().to_string();
                                app.set_config("upload_limit_kib", v, window, cx)
                            })),
                    ),
            )
            .child(note(s::RESTART_NOTE))
            .child(label(s::SETTINGS_LOGS))
            .child(
                h_flex()
                    .gap(px(6.))
                    .child(
                        div().flex_1().min_w_0().truncate().child(
                            logs.as_ref()
                                .map(|p| p.display().to_string())
                                .unwrap_or_default(),
                        ),
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
            .child(div().text_size(px(11.5)).text_color(p.ink_3).child(node))
            .into_any_element()
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
                    .child(match &self.last_error {
                        Some(e) => format!("{} ({e})", s::CONNECTING_SERVICE),
                        None => s::CONNECTING_SERVICE.to_owned(),
                    }),
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
                            .child(format!("{} ({relay})", s::RELAY_SLOWER)),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(p.ink_2)
                            .child(RELAY_NOTICE),
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
        main = main.child(div().w_full().max_w(px(1100.)).child(page));

        h_flex()
            .size_full()
            .bg(p.bg)
            .text_color(p.ink)
            .text_size(px(13.))
            .child(self.render_sidebar(cx))
            .child(main)
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

fn small_label(text: &'static str, p: &Palette) -> Div {
    div()
        .text_size(px(11.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(p.ink_3)
        .child(text)
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
                        .truncate()
                        .child(title.to_owned()),
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
        .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
        .child(div().text_size(px(12.)).text_color(p.ink_2).child(text))
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
        .child(label);
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

/// An amount set wide, with its unit (and the total) small beside it.
fn amount(big: String, small: String, p: &Palette) -> Div {
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
                .child(big),
        )
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(p.ink_3)
                .font_features(tabular())
                .child(small),
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
