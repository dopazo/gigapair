//! The local API between clients (app, CLI) and the `gigapaird` service.
//!
//! Every client capability lives here first, so the app and the CLI stay at
//! feature parity (RF-10).

use serde::{Deserialize, Serialize};

use crate::types::{Contact, PendingPair, TransferInfo};

/// Bumped on incompatible changes to [`Request`], [`Response`] or [`Event`].
pub const IPC_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// First message on every connection.
    Hello {
        version: u32,
        client: String,
    },
    Status,

    /// Create a one-time pairing code.
    PairCreate,
    /// Use the other device's code.
    PairJoin {
        code: String,
    },
    /// Confirm that the fingerprint matches and name the contact.
    PairConfirm {
        id: String,
        name: String,
    },
    PairReject {
        id: String,
    },
    PendingPairs,

    Contacts,
    /// `contact` is a name or a node id (or a unique prefix of one).
    ContactRename {
        contact: String,
        name: String,
    },
    ContactRemove {
        contact: String,
    },
    ContactTrust {
        contact: String,
        auto_accept: bool,
    },

    Send {
        contact: String,
        /// Absolute paths of files and folders.
        paths: Vec<String>,
    },
    /// Incoming offers waiting for an answer.
    Requests,
    /// `id` may be a unique prefix.
    Accept {
        id: String,
        dest_dir: Option<String>,
    },
    Reject {
        id: String,
    },
    /// Transfers that are not finished.
    Transfers,
    History {
        limit: u32,
    },
    ClearHistory,
    Pause {
        id: String,
    },
    Resume {
        id: String,
    },
    Cancel {
        id: String,
    },
    /// Send the same files again (failed or cancelled outgoing transfer).
    Retry {
        id: String,
    },

    /// `None` lists every setting.
    ConfigGet {
        key: Option<String>,
    },
    ConfigSet {
        key: String,
        value: String,
    },
    /// Records that the relay explanation was shown.
    AckRelayNotice,

    /// Turns this connection into a stream of [`Event`]s.
    Subscribe,
    /// Stops the service.
    Shutdown,
}

// Adjacently tagged: internally tagged enums can't hold lists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Response {
    Ok,
    Hello {
        version: u32,
        service_version: String,
    },
    Status(StatusInfo),
    PairCode {
        code: String,
        expires_at: i64,
    },
    PendingPair(PendingPair),
    PendingPairs(Vec<PendingPair>),
    Contact(Contact),
    Contacts(Vec<Contact>),
    Sent {
        transfer: TransferInfo,
        warnings: Vec<String>,
    },
    Transfer(TransferInfo),
    Transfers(Vec<TransferInfo>),
    Config(Vec<ConfigEntry>),
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigEntry {
    pub key: String,
    pub value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusInfo {
    pub node_id: String,
    pub device_name: String,
    pub service_version: String,
    pub protocol_version: u32,
    /// Home relay this device is reachable through.
    pub home_relay: Option<String>,
    pub direct_only: bool,
    pub keep_awake_enabled: bool,
    /// The computer is currently being kept awake.
    pub keeping_awake: bool,
    pub relay_notice_shown: bool,
    pub download_dir: String,
    pub active_transfers: u32,
    pub pending_requests: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Event {
    /// A transfer was created or changed.
    Transfer(TransferInfo),
    /// A new offer needs the user's answer.
    IncomingRequest(TransferInfo),
    /// Another device used our pairing code; confirm the fingerprint.
    PairRequest(PendingPair),
    PairCompleted(Contact),
    ContactsChanged,
    /// The first transfer through a relay: explain what the relay sees.
    RelayFirstUse {
        relay: String,
    },
    KeepAwake {
        active: bool,
    },
    Notice {
        message: String,
    },
}

/// Explanation shown the first time a relay is used (spec, "Arquitectura").
pub const RELAY_NOTICE: &str = "This transfer goes through a relay because a direct connection \
could not be made. The relay only forwards end-to-end encrypted data and cannot read your files, \
but it can see both devices' IP addresses, when you transfer and how much. You can turn on \
\"direct only\" or use your own relay in the settings.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Direction, TransferStatus};

    fn sample_transfer() -> TransferInfo {
        TransferInfo {
            id: "ab".into(),
            direction: Direction::Send,
            peer_id: "p".into(),
            peer_name: "Ana".into(),
            status: TransferStatus::Transferring,
            label: "x".into(),
            total_bytes: 1,
            done_bytes: 0,
            file_count: 1,
            speed_bps: 0,
            eta_secs: None,
            conn: None,
            error: None,
            note: None,
            dest_dir: None,
            created_at: 0,
            finished_at: None,
        }
    }

    fn roundtrip<T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(v: T) {
        let text = serde_json::to_string(&v).unwrap();
        assert!(!text.contains('\n'));
        let back: T = serde_json::from_str(&text).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn every_message_serializes() {
        let contact = Contact {
            node_id: "n".into(),
            name: "Ana".into(),
            auto_accept: false,
            online: true,
            last_seen: None,
            added_at: 0,
        };
        let pending = PendingPair {
            id: "1".into(),
            node_id: "n".into(),
            remote_name: "r".into(),
            fingerprint: "a b c d".into(),
            incoming: true,
            expires_at: 0,
        };
        for r in [
            Response::Ok,
            Response::Hello {
                version: 1,
                service_version: "x".into(),
            },
            Response::PairCode {
                code: "c".into(),
                expires_at: 1,
            },
            Response::PendingPair(pending.clone()),
            Response::PendingPairs(vec![pending.clone()]),
            Response::Contact(contact.clone()),
            Response::Contacts(vec![contact.clone()]),
            Response::Sent {
                transfer: sample_transfer(),
                warnings: vec!["w".into()],
            },
            Response::Transfer(sample_transfer()),
            Response::Transfers(vec![sample_transfer()]),
            Response::Config(vec![ConfigEntry {
                key: "k".into(),
                value: None,
            }]),
            Response::Error {
                message: "e".into(),
            },
        ] {
            roundtrip(r);
        }
        for e in [
            Event::Transfer(sample_transfer()),
            Event::IncomingRequest(sample_transfer()),
            Event::PairRequest(pending),
            Event::PairCompleted(contact),
            Event::ContactsChanged,
            Event::RelayFirstUse { relay: "r".into() },
            Event::KeepAwake { active: true },
            Event::Notice {
                message: "m".into(),
            },
        ] {
            roundtrip(e);
        }
        for r in [
            Request::Status,
            Request::Send {
                contact: "a".into(),
                paths: vec!["/x".into()],
            },
            Request::ConfigGet { key: None },
            Request::History { limit: 3 },
        ] {
            roundtrip(r);
        }
    }
}
