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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
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
#[serde(tag = "type", rename_all = "snake_case")]
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
