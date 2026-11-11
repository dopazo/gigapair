//! Peer-to-peer protocol between two Gigapair services.
//!
//! Runs over iroh QUIC connections with ALPN [`ALPN`]. Every exchange uses
//! its own bidirectional stream: the opener writes one [`Envelope`] with a
//! request, the other side answers with one envelope holding a
//! [`PeerResponse`], optionally followed by raw bytes (block hashes or file
//! data). Every envelope carries [`PROTOCOL_VERSION`]; a mismatch is
//! answered with [`ErrorCode::IncompatibleVersion`] before anything else
//! happens, so incompatible versions fail up front with a clear message.
//!
//! Messages are length-prefixed (u32, big endian) JSON.

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const ALPN: &[u8] = b"gigapair/1";
pub const PROTOCOL_VERSION: u32 = 1;

/// Upper bound for one JSON message (an offer with many files can be large).
pub const MAX_MESSAGE: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub v: u32,
    pub body: T,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PeerRequest {
    /// Sent by the device that pasted a pairing code.
    Pair {
        /// Hex-encoded one-time secret from the code.
        secret: String,
        device_name: String,
        /// How to reach the device that sent this.
        addr: Option<iroh::EndpointAddr>,
    },
    /// Reachability check; also refreshes "last seen" and the address.
    Ping { addr: Option<iroh::EndpointAddr> },
    /// The sender removed us as a contact.
    Unpair,
    /// Sender → receiver: please accept these files.
    Offer(Offer),
    /// Receiver → sender: the user accepted or rejected the offer.
    Decision {
        transfer_id: String,
        accepted: bool,
        reason: Option<String>,
    },
    /// Either side: state change the other side should mirror.
    Notice { transfer_id: String, notice: Notice },
    /// Receiver → sender: block hash list of one file. Answered with
    /// [`PeerResponse::Hashes`] followed by the raw list.
    GetHashes { transfer_id: String, file: u32 },
    /// Receiver → sender: file data from `start_block` to the end.
    /// Answered with [`PeerResponse::Data`] followed by the raw bytes.
    Get {
        transfer_id: String,
        file: u32,
        start_block: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Notice {
    Paused,
    Resumed,
    Cancelled,
    /// Receiver: every file is verified and in place.
    Completed,
    Failed {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Offer {
    pub transfer_id: String,
    pub sender_name: String,
    pub label: String,
    pub total_size: u64,
    pub files: Vec<OfferFile>,
    pub dirs: Vec<OfferDir>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfferFile {
    /// `/`-separated relative path.
    pub path: String,
    pub size: u64,
    pub mtime_ns: i64,
    /// BLAKE3 of the whole file, hex.
    pub root_hash: String,
    /// BLAKE3 of the block hash list, hex.
    pub blocks_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfferDir {
    pub path: String,
    pub mtime_ns: i64,
}

/// Receiver's state for an offer, as seen by the sender.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OfferState {
    /// Waiting for the receiving user.
    Pending,
    Accepted,
    Rejected {
        reason: String,
    },
    Cancelled,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PeerResponse {
    Ok,
    Pong {
        device_name: String,
    },
    Paired {
        device_name: String,
    },
    Offer(OfferState),
    /// Followed by `count * 32` bytes.
    Hashes {
        count: u64,
    },
    /// Followed by the file bytes from the requested block to the end.
    Data {
        len: u64,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    IncompatibleVersion,
    NotPaired,
    InvalidCode,
    UnknownTransfer,
    Cancelled,
    Rejected,
    Paused,
    SourceChanged,
    DirectOnly,
    BadRequest,
    Internal,
}

#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error("connection error: {0}")]
    Io(#[from] std::io::Error),
    #[error("malformed message from peer: {0}")]
    Json(#[from] serde_json::Error),
    #[error("message from peer is too large ({0} bytes)")]
    TooLarge(usize),
    #[error(
        "the other device runs an incompatible version of Gigapair (protocol {peer}, this device {ours}); update both devices to the same version"
    )]
    Incompatible { peer: u32, ours: u32 },
}

pub async fn write_msg<W, T>(w: &mut W, body: &T) -> Result<(), WireError>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let bytes = serde_json::to_vec(&Envelope {
        v: PROTOCOL_VERSION,
        body,
    })?;
    if bytes.len() > MAX_MESSAGE {
        return Err(WireError::TooLarge(bytes.len()));
    }
    w.write_all(&(bytes.len() as u32).to_be_bytes()).await?;
    w.write_all(&bytes).await?;
    Ok(())
}

/// Reads one envelope. Returns the peer's version alongside the body when
/// it parses; a version mismatch whose body doesn't parse is reported as
/// [`WireError::Incompatible`].
pub async fn read_msg<R, T>(r: &mut R) -> Result<Envelope<T>, WireError>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_MESSAGE {
        return Err(WireError::TooLarge(len));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    match serde_json::from_slice::<Envelope<T>>(&buf) {
        Ok(env) => Ok(env),
        Err(e) => {
            #[derive(Deserialize)]
            struct VersionOnly {
                v: u32,
            }
            match serde_json::from_slice::<VersionOnly>(&buf) {
                Ok(VersionOnly { v }) if v != PROTOCOL_VERSION => Err(WireError::Incompatible {
                    peer: v,
                    ours: PROTOCOL_VERSION,
                }),
                _ => Err(e.into()),
            }
        }
    }
}

impl PeerResponse {
    pub fn error(code: ErrorCode, message: impl Into<String>) -> Self {
        Self::Error {
            code,
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn roundtrip() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        let req = PeerRequest::Get {
            transfer_id: "t".into(),
            file: 2,
            start_block: 9,
        };
        write_msg(&mut a, &req).await.unwrap();
        let env: Envelope<PeerRequest> = read_msg(&mut b).await.unwrap();
        assert_eq!(env.v, PROTOCOL_VERSION);
        assert_eq!(env.body, req);
    }

    #[tokio::test]
    async fn unknown_newer_message_reports_version() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        let bytes = br#"{"v":99,"body":{"type":"teleport"}}"#;
        a.write_all(&(bytes.len() as u32).to_be_bytes())
            .await
            .unwrap();
        a.write_all(bytes).await.unwrap();
        let err = read_msg::<_, PeerRequest>(&mut b).await.unwrap_err();
        assert!(matches!(err, WireError::Incompatible { peer: 99, .. }));
    }

    #[tokio::test]
    async fn oversized_is_rejected() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        a.write_all(&(u32::MAX).to_be_bytes()).await.unwrap();
        assert!(matches!(
            read_msg::<_, PeerRequest>(&mut b).await,
            Err(WireError::TooLarge(_))
        ));
    }
}
