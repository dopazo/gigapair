//! Data types shared by the engine, the database and the IPC API.

use serde::{Deserialize, Serialize};

/// Which side of a transfer this device is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Send,
    Receive,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Send => "send",
            Self::Receive => "receive",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "send" => Some(Self::Send),
            "receive" => Some(Self::Receive),
            _ => None,
        }
    }
}

/// State of a transfer (RF-03).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferStatus {
    /// Sender: hashing the files before offering them.
    Preparing,
    /// Sender: offer made (or being delivered); waiting for the receiver.
    WaitingAccept,
    /// Receiver: the user has to accept or reject.
    PendingAccept,
    /// Accepted, waiting for a free slot or for the peer to come online.
    Queued,
    Transferring,
    Paused,
    /// Receiver: re-hashing the finished files before moving them into place.
    Verifying,
    Completed,
    Rejected,
    Cancelled,
    Failed,
}

impl TransferStatus {
    pub const ALL: [Self; 11] = [
        Self::Preparing,
        Self::WaitingAccept,
        Self::PendingAccept,
        Self::Queued,
        Self::Transferring,
        Self::Paused,
        Self::Verifying,
        Self::Completed,
        Self::Rejected,
        Self::Cancelled,
        Self::Failed,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Preparing => "preparing",
            Self::WaitingAccept => "waiting_accept",
            Self::PendingAccept => "pending_accept",
            Self::Queued => "queued",
            Self::Transferring => "transferring",
            Self::Paused => "paused",
            Self::Verifying => "verifying",
            Self::Completed => "completed",
            Self::Rejected => "rejected",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.as_str() == s)
    }

    /// Finished for good; belongs in the history.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Rejected | Self::Cancelled | Self::Failed
        )
    }

    /// Moving data or verifying it: keeps the computer awake.
    pub fn is_active(self) -> bool {
        matches!(self, Self::Preparing | Self::Transferring | Self::Verifying)
    }

    /// Human-readable label for clients.
    pub fn label(self) -> &'static str {
        match self {
            Self::Preparing => "Preparing",
            Self::WaitingAccept => "Waiting for acceptance",
            Self::PendingAccept => "Awaiting your answer",
            Self::Queued => "Queued",
            Self::Transferring => "Transferring",
            Self::Paused => "Paused",
            Self::Verifying => "Verifying",
            Self::Completed => "Completed",
            Self::Rejected => "Rejected",
            Self::Cancelled => "Cancelled",
            Self::Failed => "Failed",
        }
    }
}

/// How the data travels between the two devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnKind {
    Direct,
    Relay,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnInfo {
    pub kind: ConnKind,
    /// Relay host when `kind` is `Relay`.
    pub relay: Option<String>,
}

impl ConnInfo {
    pub fn label(&self) -> String {
        match (self.kind, &self.relay) {
            (ConnKind::Direct, _) => "direct".to_owned(),
            (ConnKind::Relay, Some(r)) => format!("via relay ({r})"),
            (ConnKind::Relay, None) => "via relay".to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    /// iroh endpoint id (Ed25519 public key).
    pub node_id: String,
    pub name: String,
    pub auto_accept: bool,
    pub online: bool,
    /// Unix seconds of the last successful contact.
    pub last_seen: Option<i64>,
    pub added_at: i64,
}

/// Everything a client needs to show one transfer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransferInfo {
    pub id: String,
    pub direction: Direction,
    pub peer_id: String,
    pub peer_name: String,
    pub status: TransferStatus,
    /// Short description: `talk.mp4`, `talks/` or `talk.mp4 + 3 more`.
    pub label: String,
    pub total_bytes: u64,
    /// Bytes transferred (or hashed, while preparing; or re-hashed, while
    /// verifying).
    pub done_bytes: u64,
    pub file_count: u64,
    pub speed_bps: u64,
    pub eta_secs: Option<u64>,
    pub conn: Option<ConnInfo>,
    pub error: Option<String>,
    /// Short note about what the transfer is waiting for.
    pub note: Option<String>,
    pub dest_dir: Option<String>,
    pub created_at: i64,
    pub finished_at: Option<i64>,
}

impl TransferInfo {
    pub fn short_id(&self) -> &str {
        short_id(&self.id)
    }

    pub fn percent(&self) -> f64 {
        if self.total_bytes == 0 {
            if self.status == TransferStatus::Completed {
                100.0
            } else {
                0.0
            }
        } else {
            (self.done_bytes as f64 / self.total_bytes as f64 * 100.0).min(100.0)
        }
    }
}

/// First 8 characters of an id, as shown in lists.
pub fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

/// An incoming pairing that this device has to confirm.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingPair {
    pub id: String,
    pub node_id: String,
    pub remote_name: String,
    /// Four words both devices must show.
    pub fingerprint: String,
    /// True if the other device pasted our code; false if we pasted theirs.
    pub incoming: bool,
    pub expires_at: i64,
}

/// Current Unix time in seconds.
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Formats a byte count for people: `1.5 GB`.
pub fn format_bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut v = n as f64;
    let mut unit = 0;
    while v >= 1000.0 && unit < UNITS.len() - 1 {
        v /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

/// Formats seconds as `1h 02m`, `3m 05s` or `42s`.
pub fn format_duration(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}h {m:02}m")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_roundtrip() {
        for s in TransferStatus::ALL {
            assert_eq!(TransferStatus::parse(s.as_str()), Some(s));
        }
    }

    #[test]
    fn bytes_format() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1_500_000_000), "1.5 GB");
    }

    #[test]
    fn duration_format() {
        assert_eq!(format_duration(42), "42s");
        assert_eq!(format_duration(185), "3m 05s");
        assert_eq!(format_duration(3720), "1h 02m");
    }
}
