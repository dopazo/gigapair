//! Requests from clients, mapped onto the engine.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::{EngineError, EngineResult, Inner, recv, send};
use crate::api::{ConfigEntry, Event, Request, Response, StatusInfo};
use crate::config::KEYS;
use crate::types::{Contact, Direction, TransferInfo, TransferStatus};
use crate::wire::{Notice, PROTOCOL_VERSION, PeerRequest};

impl super::Engine {
    /// Handles one client request. `Hello`, `Subscribe` and `Shutdown` are
    /// connection-level and handled by the service.
    pub async fn handle(&self, req: Request) -> Response {
        match self.inner.handle(req).await {
            Ok(r) => r,
            Err(e) => Response::Error {
                message: e.to_string(),
            },
        }
    }

    /// Sends files to a contact (convenience for tests and embedders).
    pub async fn send_files(
        &self,
        contact: &str,
        paths: Vec<PathBuf>,
    ) -> EngineResult<(TransferInfo, Vec<String>)> {
        let c = self.inner.resolve_contact(contact)?;
        send::create(&self.inner, &c, paths).await
    }

    pub fn transfer(&self, id: &str) -> Option<TransferInfo> {
        self.inner.transfer_info(id)
    }

    pub fn contacts(&self) -> EngineResult<Vec<Contact>> {
        self.inner.contacts()
    }
}

impl Inner {
    async fn handle(self: &Arc<Self>, req: Request) -> EngineResult<Response> {
        Ok(match req {
            Request::Hello { .. } | Request::Subscribe | Request::Shutdown => {
                return Err(EngineError::user("not supported here"));
            }
            Request::Status => Response::Status(self.status()),

            Request::PairCreate => {
                let (code, expires_at) = self.pair_create();
                Response::PairCode { code, expires_at }
            }
            Request::PairJoin { code } => Response::PendingPair(self.pair_join(&code).await?),
            Request::PairConfirm { id, name } => Response::Contact(self.pair_confirm(&id, &name)?),
            Request::PairReject { id } => {
                self.pair_reject(&id)?;
                Response::Ok
            }
            Request::PendingPairs => Response::PendingPairs(self.pending_pairs()),

            Request::Contacts => Response::Contacts(self.contacts()?),
            Request::ContactRename { contact, name } => {
                let c = self.resolve_contact(&contact)?;
                let name = name.trim();
                if name.is_empty() {
                    return Err(EngineError::user("the name can't be empty"));
                }
                self.db.rename_contact(&c.node_id, name)?;
                self.emit(Event::ContactsChanged);
                Response::Ok
            }
            Request::ContactRemove { contact } => {
                let c = self.resolve_contact(&contact)?;
                self.db.remove_contact(&c.node_id)?;
                self.cancel_peer_transfers(&c.node_id, true);
                self.notify_peer(c.node_id.clone(), PeerRequest::Unpair);
                self.online
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .remove(&c.node_id);
                self.emit(Event::ContactsChanged);
                Response::Ok
            }
            Request::ContactTrust {
                contact,
                auto_accept,
            } => {
                let c = self.resolve_contact(&contact)?;
                self.db.set_auto_accept(&c.node_id, auto_accept)?;
                self.emit(Event::ContactsChanged);
                Response::Ok
            }

            Request::Send { contact, paths } => {
                if paths.is_empty() {
                    return Err(EngineError::user("choose at least one file or folder"));
                }
                let c = self.resolve_contact(&contact)?;
                let (transfer, warnings) =
                    send::create(self, &c, paths.into_iter().map(PathBuf::from).collect()).await?;
                Response::Sent { transfer, warnings }
            }
            Request::Requests => Response::Transfers(
                self.db
                    .active_transfers()?
                    .iter()
                    .filter(|r| {
                        r.direction == Direction::Receive
                            && r.status == TransferStatus::PendingAccept
                    })
                    .map(|r| self.info(r))
                    .collect(),
            ),
            Request::Accept { id, dest_dir } => {
                let id = self.resolve_transfer(&id)?;
                recv::accept(self, &id, dest_dir.map(PathBuf::from))?;
                Response::Transfer(self.info_or_err(&id)?)
            }
            Request::Reject { id } => {
                let id = self.resolve_transfer(&id)?;
                recv::reject(self, &id)?;
                Response::Transfer(self.info_or_err(&id)?)
            }
            Request::Transfers => Response::Transfers(
                self.db
                    .active_transfers()?
                    .iter()
                    .map(|r| self.info(r))
                    .collect(),
            ),
            Request::History { limit } => Response::Transfers(
                self.db
                    .history(limit.clamp(1, 10_000))?
                    .iter()
                    .map(|r| self.info(r))
                    .collect(),
            ),
            Request::ClearHistory => {
                self.db.clear_history()?;
                Response::Ok
            }
            Request::Pause { id } => {
                let id = self.resolve_transfer(&id)?;
                self.pause(&id)?;
                Response::Transfer(self.info_or_err(&id)?)
            }
            Request::Resume { id } => {
                let id = self.resolve_transfer(&id)?;
                self.resume(&id)?;
                Response::Transfer(self.info_or_err(&id)?)
            }
            Request::Cancel { id } => {
                let id = self.resolve_transfer(&id)?;
                self.cancel(&id)?;
                Response::Transfer(self.info_or_err(&id)?)
            }
            Request::Retry { id } => {
                let id = self.resolve_transfer(&id)?;
                self.retry(&id).await?
            }

            Request::ConfigGet { key } => {
                let cfg = self.config().clone();
                let keys: Vec<&str> = match &key {
                    Some(k) => vec![k.as_str()],
                    None => KEYS.to_vec(),
                };
                let mut out = Vec::new();
                for k in keys {
                    let value = cfg.get(k).map_err(|e| EngineError::user(e.to_string()))?;
                    out.push(ConfigEntry {
                        key: k.to_owned(),
                        value,
                    });
                }
                Response::Config(out)
            }
            Request::ConfigSet { key, value } => {
                self.config_set(&key, &value)?;
                Response::Ok
            }
            Request::AckRelayNotice => {
                self.config().relay_notice_shown = true;
                self.save_config()?;
                Response::Ok
            }
        })
    }

    fn status(&self) -> StatusInfo {
        let cfg = self.config().clone();
        let active = self.db.active_transfers().unwrap_or_default();
        StatusInfo {
            node_id: self.endpoint.id().to_string(),
            device_name: cfg.effective_device_name(),
            service_version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol_version: PROTOCOL_VERSION,
            home_relay: self
                .endpoint
                .addr()
                .relay_urls()
                .next()
                .map(|u| u.to_string()),
            direct_only: cfg.direct_only,
            keep_awake_enabled: cfg.keep_awake,
            keeping_awake: self.keeping_awake.load(Ordering::Relaxed),
            relay_notice_shown: cfg.relay_notice_shown,
            download_dir: cfg
                .effective_download_dir(&self.dirs.downloads)
                .display()
                .to_string(),
            active_transfers: active.iter().filter(|r| r.status.is_active()).count() as u32,
            pending_requests: active
                .iter()
                .filter(|r| {
                    r.direction == Direction::Receive && r.status == TransferStatus::PendingAccept
                })
                .count() as u32,
        }
    }

    fn info_or_err(&self, id: &str) -> EngineResult<TransferInfo> {
        self.transfer_info(id)
            .ok_or_else(|| EngineError::user("no such transfer"))
    }

    /// Finds a contact by node id, name or unique prefix of either.
    pub(super) fn resolve_contact(&self, query: &str) -> EngineResult<Contact> {
        let q = query.trim();
        let contacts = self.contacts()?;
        if let Some(c) = contacts
            .iter()
            .find(|c| c.node_id == q || c.name.eq_ignore_ascii_case(q))
        {
            return Ok(c.clone());
        }
        let ql = q.to_lowercase();
        let matches: Vec<&Contact> = contacts
            .iter()
            .filter(|c| {
                (q.len() >= 4 && c.node_id.starts_with(q)) || c.name.to_lowercase().starts_with(&ql)
            })
            .collect();
        match matches.as_slice() {
            [one] => Ok((*one).clone()),
            [] => Err(EngineError::user(format!(
                "no contact matches {q:?}; run `gigapair contacts` to list them"
            ))),
            many => Err(EngineError::user(format!(
                "{q:?} matches several contacts ({}); be more specific",
                many.iter()
                    .map(|c| c.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }

    /// Finds a transfer by id or unique prefix.
    fn resolve_transfer(&self, query: &str) -> EngineResult<String> {
        let q = query.trim();
        if q.is_empty() || !q.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(EngineError::user(format!("{q:?} is not a transfer id")));
        }
        let ids = self.db.resolve_transfer_id(&q.to_ascii_lowercase())?;
        match ids.as_slice() {
            [one] => Ok(one.clone()),
            [] => Err(EngineError::user(format!("no transfer with id {q}"))),
            _ => Err(EngineError::user(format!(
                "{q} matches several transfers; use more characters"
            ))),
        }
    }

    fn row(&self, id: &str) -> EngineResult<crate::db::TransferRow> {
        self.db
            .transfer(id)?
            .ok_or_else(|| EngineError::user("no such transfer"))
    }

    fn pause(self: &Arc<Self>, id: &str) -> EngineResult<()> {
        let row = self.row(id)?;
        if row.status.is_terminal() {
            return Err(EngineError::user("this transfer has already finished"));
        }
        if row.status == TransferStatus::PendingAccept {
            return Err(EngineError::user("accept or reject this request first"));
        }
        if row.status == TransferStatus::Paused {
            return Ok(());
        }
        self.stop_task(id);
        self.flush_progress(id);
        self.set_note(id, None);
        self.set_status(id, TransferStatus::Paused);
        self.notify_peer(
            row.peer,
            PeerRequest::Notice {
                transfer_id: id.to_owned(),
                notice: Notice::Paused,
            },
        );
        Ok(())
    }

    fn resume(self: &Arc<Self>, id: &str) -> EngineResult<()> {
        let row = self.row(id)?;
        if row.status != TransferStatus::Paused {
            return Ok(());
        }
        match row.direction {
            Direction::Send => {
                let needs_hash = self.db.files(id)?.iter().any(|f| f.root_hash.is_none());
                if needs_hash {
                    self.set_status(id, TransferStatus::Preparing);
                    send::start_prepare(self, id);
                } else {
                    self.set_status(id, TransferStatus::WaitingAccept);
                    send::start_offer(self, id);
                }
            }
            Direction::Receive => {
                self.set_status(id, TransferStatus::Queued);
                recv::start_download(self, id);
            }
        }
        self.notify_peer(
            row.peer,
            PeerRequest::Notice {
                transfer_id: id.to_owned(),
                notice: Notice::Resumed,
            },
        );
        Ok(())
    }

    fn cancel(self: &Arc<Self>, id: &str) -> EngineResult<()> {
        let row = self.row(id)?;
        if row.status.is_terminal() {
            return Err(EngineError::user("this transfer has already finished"));
        }
        if row.direction == Direction::Receive && row.status == TransferStatus::PendingAccept {
            return recv::reject(self, id);
        }
        self.stop_task(id);
        self.flush_progress(id);
        self.set_note(id, None);
        self.set_status(id, TransferStatus::Cancelled);
        if row.direction == Direction::Receive {
            recv::cleanup_partial(&row);
        }
        self.notify_peer(
            row.peer,
            PeerRequest::Notice {
                transfer_id: id.to_owned(),
                notice: Notice::Cancelled,
            },
        );
        Ok(())
    }

    async fn retry(self: &Arc<Self>, id: &str) -> EngineResult<Response> {
        let row = self.row(id)?;
        match (row.direction, row.status) {
            (Direction::Send, s) if s.is_terminal() => {
                if row.sources.is_empty() {
                    return Err(EngineError::user("the original files are not known"));
                }
                let c = self.resolve_contact(&row.peer)?;
                let (transfer, warnings) =
                    send::create(self, &c, row.sources.iter().map(PathBuf::from).collect()).await?;
                Ok(Response::Sent { transfer, warnings })
            }
            (Direction::Receive, TransferStatus::Failed) => {
                let _ = self.db.set_error(id, None);
                self.set_status(id, TransferStatus::Queued);
                recv::start_download(self, id);
                Ok(Response::Transfer(self.info_or_err(id)?))
            }
            (Direction::Receive, s) if s.is_terminal() => {
                Err(EngineError::user("ask the sender to send it again"))
            }
            _ => Err(EngineError::user(
                "only finished transfers can be retried; use resume for paused ones",
            )),
        }
    }

    /// Cancels every unfinished transfer with `node` (contact removed).
    pub(super) fn cancel_peer_transfers(self: &Arc<Self>, node: &str, notify: bool) {
        let rows = self.db.active_transfers().unwrap_or_default();
        for row in rows.into_iter().filter(|r| r.peer == node) {
            self.stop_task(&row.id);
            let _ = self.db.set_error(&row.id, Some("Contact removed"));
            self.set_note(&row.id, None);
            self.set_status(&row.id, TransferStatus::Cancelled);
            if row.direction == Direction::Receive {
                recv::cleanup_partial(&row);
            }
            if notify {
                self.notify_peer(
                    row.peer.clone(),
                    PeerRequest::Notice {
                        transfer_id: row.id.clone(),
                        notice: Notice::Cancelled,
                    },
                );
            }
        }
    }

    fn config_set(&self, key: &str, value: &str) -> EngineResult<()> {
        if key == "download_dir" && !value.trim().is_empty() {
            let p = PathBuf::from(value.trim());
            if !p.is_absolute() {
                return Err(EngineError::user("download_dir must be an absolute path"));
            }
            std::fs::create_dir_all(&p)
                .map_err(|e| EngineError::user(format!("could not create {}: {e}", p.display())))?;
        }
        {
            let mut cfg = self.config();
            cfg.set(key, value)
                .map_err(|e| EngineError::user(e.to_string()))?;
            if key == "max_parallel_downloads" {
                let new = cfg.max_parallel_downloads as u64;
                let old = self.slot_count.load(Ordering::Relaxed);
                if new > old {
                    self.download_slots.add_permits((new - old) as usize);
                    self.slot_count.store(new, Ordering::Relaxed);
                }
            }
        }
        self.save_config()?;
        if matches!(key, "relay_url" | "discovery" | "key_storage")
            || (key == "max_parallel_downloads"
                && (self.config().max_parallel_downloads as u64)
                    < self.slot_count.load(Ordering::Relaxed))
        {
            self.emit(Event::Notice {
                message: format!("Restart the Gigapair service to apply {key}"),
            });
        }
        Ok(())
    }
}
