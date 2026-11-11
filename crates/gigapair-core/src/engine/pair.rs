//! Pairing (RF-01): one-time codes, fingerprint confirmation, unpairing.

use std::sync::Arc;

use iroh::{EndpointAddr, EndpointId};

use super::net::{advertised_addr, store_addr};
use super::{EngineError, EngineResult, Inner, new_id};
use crate::api::Event;
use crate::pairing::{CODE_TTL_SECS, PairSecret, PairingCode, fingerprint};
use crate::types::{Contact, PendingPair, now_secs};
use crate::wire::{ErrorCode, PeerRequest, PeerResponse};

pub(crate) struct PendingEntry {
    pub info: PendingPair,
    pub addr: Option<EndpointAddr>,
}

impl Inner {
    pub(super) fn pair_create(&self) -> (String, i64) {
        let secret: PairSecret = rand::random();
        let expires_at = now_secs() + CODE_TTL_SECS;
        {
            let mut codes = self.pair_codes.lock().unwrap_or_else(|p| p.into_inner());
            let now = now_secs();
            codes.retain(|_, exp| *exp > now);
            codes.insert(secret, expires_at);
        }
        let code = PairingCode::new(advertised_addr(self, false), secret);
        (code.encode(), expires_at)
    }

    pub(super) async fn pair_join(self: &Arc<Self>, code: &str) -> EngineResult<PendingPair> {
        let code = PairingCode::decode(code).map_err(|e| EngineError::user(e.to_string()))?;
        if code.addr.id == self.endpoint.id() {
            return Err(EngineError::user(
                "this is this device's own pairing code; paste it on the other device",
            ));
        }
        let conn = self
            .connect_addr(code.addr.clone())
            .await
            .map_err(|e| {
                EngineError::user(format!(
                    "{e}. Make sure Gigapair is running on the other device and the code is less than 10 minutes old"
                ))
            })?;
        let req = PeerRequest::Pair {
            secret: data_encoding::HEXLOWER.encode(&code.secret),
            device_name: self.device_name(),
            addr: Some(advertised_addr(self, true)),
        };
        let remote_name = match self.open_request(&conn, &req).await {
            Ok((PeerResponse::Paired { device_name }, _)) => device_name,
            Ok(_) => return Err(EngineError::user("unexpected answer from the other device")),
            Err(e) => return Err(EngineError::user(e.to_string())),
        };
        let info = PendingPair {
            id: new_id()[..8].to_owned(),
            node_id: code.addr.id.to_string(),
            remote_name,
            fingerprint: fingerprint(&self.endpoint.id(), &code.addr.id, &code.secret),
            incoming: false,
            expires_at: now_secs() + CODE_TTL_SECS,
        };
        self.pending_pairs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(
                info.id.clone(),
                PendingEntry {
                    info: info.clone(),
                    addr: Some(code.addr),
                },
            );
        Ok(info)
    }

    pub(super) fn pair_confirm(&self, id: &str, name: &str) -> EngineResult<Contact> {
        let name = name.trim();
        if name.is_empty() {
            return Err(EngineError::user("give the contact a name"));
        }
        let entry = self
            .pending_pairs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(id)
            .filter(|e| e.info.expires_at > now_secs())
            .ok_or_else(|| {
                EngineError::user(
                    "this pairing request expired or was already answered; pair again",
                )
            })?;
        let node = entry.info.node_id.clone();
        self.db.upsert_contact(&node, name, false)?;
        if let Some(addr) = &entry.addr {
            store_addr(self, addr);
        }
        self.set_online(&node, true);
        let contact = self
            .contacts()?
            .into_iter()
            .find(|c| c.node_id == node)
            .ok_or_else(|| EngineError::user("contact could not be saved"))?;
        self.emit(Event::PairCompleted(contact.clone()));
        self.emit(Event::ContactsChanged);
        Ok(contact)
    }

    pub(super) fn pair_reject(self: &Arc<Self>, id: &str) -> EngineResult<()> {
        let entry = self
            .pending_pairs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(id)
            .ok_or_else(|| EngineError::user("no such pairing request"))?;
        // The other side may already have saved us; tell it to forget us.
        self.notify_peer(entry.info.node_id, PeerRequest::Unpair);
        Ok(())
    }
}

pub(super) fn handle_pair(
    inner: &Arc<Inner>,
    remote: EndpointId,
    secret_hex: &str,
    device_name: &str,
    addr: Option<EndpointAddr>,
) -> PeerResponse {
    let invalid = || {
        PeerResponse::error(
            ErrorCode::InvalidCode,
            "the pairing code expired or was already used; create a new one on the other device",
        )
    };
    let Ok(bytes) = data_encoding::HEXLOWER_PERMISSIVE.decode(secret_hex.as_bytes()) else {
        return invalid();
    };
    let Ok(secret) = PairSecret::try_from(bytes.as_slice()) else {
        return invalid();
    };
    // One-time: the code is consumed whatever happens next.
    let valid = inner
        .pair_codes
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(&secret)
        .is_some_and(|exp| exp > now_secs());
    if !valid {
        return invalid();
    }
    let name: String = device_name.trim().chars().take(64).collect();
    let info = PendingPair {
        id: new_id()[..8].to_owned(),
        node_id: remote.to_string(),
        remote_name: if name.is_empty() {
            "Unnamed device".to_owned()
        } else {
            name
        },
        fingerprint: fingerprint(&inner.endpoint.id(), &remote, &secret),
        incoming: true,
        expires_at: now_secs() + CODE_TTL_SECS,
    };
    inner
        .pending_pairs
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(
            info.id.clone(),
            PendingEntry {
                info: info.clone(),
                addr: addr.filter(|a| a.id == remote),
            },
        );
    inner.emit(Event::PairRequest(info));
    PeerResponse::Paired {
        device_name: inner.device_name(),
    }
}

pub(super) fn handle_unpair(inner: &Arc<Inner>, remote: EndpointId) -> PeerResponse {
    let node = remote.to_string();
    inner
        .pending_pairs
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .retain(|_, e| e.info.node_id != node);
    if matches!(inner.db.remove_contact(&node), Ok(true)) {
        tracing::info!(peer = %remote.fmt_short(), "contact removed by the other device");
        inner.cancel_peer_transfers(&node, false);
        inner.emit(Event::ContactsChanged);
        inner.emit(Event::Notice {
            message: "A contact removed you; transfers with that device were cancelled".into(),
        });
    }
    PeerResponse::Ok
}
