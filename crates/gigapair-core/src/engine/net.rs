//! Endpoint setup, connections and the incoming request dispatcher.

use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::{Connection, RecvStream, SendStream, presets};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMap, RelayMode, SecretKey, TransportAddr};

use super::{EngineError, EngineResult, Inner, NetworkMode, pair, recv, send};
use crate::config::Config;
use crate::types::{ConnInfo, ConnKind};
use crate::wire::{
    ALPN, ErrorCode, PROTOCOL_VERSION, PeerRequest, PeerResponse, WireError, read_msg, write_msg,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const PING_INTERVAL: Duration = Duration::from_secs(45);
/// How long to wait for the answer to a request.
pub(super) const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("could not reach the other device: {0}")]
    Connect(String),
    #[error("{0}")]
    Wire(#[from] WireError),
    #[error("{message}")]
    Remote { code: ErrorCode, message: String },
    #[error("unexpected answer from the other device")]
    Unexpected,
}

impl From<std::io::Error> for NetError {
    fn from(e: std::io::Error) -> Self {
        Self::Wire(WireError::Io(e))
    }
}

/// Binds the endpoint, reusing the UDP port of the last run when possible
/// so the direct addresses contacts remember stay valid across restarts
/// (useful on a LAN, or when relays are unreachable).
pub(super) async fn bind_endpoint(
    config: &Config,
    key: SecretKey,
    mode: NetworkMode,
    port_file: &std::path::Path,
) -> EngineResult<Endpoint> {
    let saved: Option<u16> = std::fs::read_to_string(port_file)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .filter(|p| *p != 0);
    let endpoint = match saved {
        Some(port) => match bind_on(config, key.clone(), mode, port).await {
            Ok(ep) => ep,
            Err(e) => {
                tracing::info!("UDP port {port} is not available ({e}); using a new one");
                bind_on(config, key, mode, 0).await?
            }
        },
        None => bind_on(config, key, mode, 0).await?,
    };
    if let Some(port) = endpoint
        .bound_sockets()
        .iter()
        .find(|a| a.is_ipv4())
        .map(|a| a.port())
        && Some(port) != saved
    {
        let _ = std::fs::write(port_file, port.to_string());
    }
    Ok(endpoint)
}

async fn bind_on(
    config: &Config,
    key: SecretKey,
    mode: NetworkMode,
    port: u16,
) -> EngineResult<Endpoint> {
    use iroh::endpoint::BindOpts;
    let bind_err = |e: String| EngineError::Bind(e);
    let builder = match mode {
        NetworkMode::LocalOnly => Endpoint::builder(presets::Minimal)
            .relay_mode(RelayMode::Disabled)
            .clear_ip_transports()
            .bind_addr(std::net::SocketAddr::from(([127, 0, 0, 1], port)))
            .map_err(|e| bind_err(e.to_string()))?,
        NetworkMode::Internet => {
            // N0 publishes only the relay URL (not IP addresses) to n0's
            // DNS, so contacts can find this device by its id.
            let mut b = Endpoint::builder(presets::N0)
                .clear_ip_transports()
                .bind_addr(std::net::SocketAddr::from(([0, 0, 0, 0], port)))
                .map_err(|e| bind_err(e.to_string()))?
                .bind_addr_with_opts(
                    std::net::SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port)),
                    BindOpts::default().set_is_required(false),
                )
                .map_err(|e| bind_err(e.to_string()))?;
            if !config.discovery {
                b = b.clear_address_lookup();
            }
            if let Some(url) = &config.relay_url {
                let url: iroh::RelayUrl = url
                    .parse()
                    .map_err(|_| bind_err(format!("invalid relay_url {url:?} in config.toml")))?;
                b = b.relay_mode(RelayMode::Custom(RelayMap::from(url)));
            }
            b
        }
    };
    builder
        .secret_key(key)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .map_err(|e| bind_err(e.to_string()))
}

/// This device's address as given to contacts: relay URLs, plus direct
/// addresses when `include_ips` or when there is no relay.
pub(super) fn advertised_addr(inner: &Inner, include_ips: bool) -> EndpointAddr {
    let full = inner.endpoint.addr();
    let has_relay = full.relay_urls().next().is_some();
    let keep_ips = include_ips || !has_relay || inner.network == NetworkMode::LocalOnly;
    EndpointAddr::from_parts(
        full.id,
        full.addrs
            .into_iter()
            .filter(|a| keep_ips || matches!(a, TransportAddr::Relay(_))),
    )
}

/// Remembers a contact's address hints: the new ones first, then the
/// ones already known (at most 8 in total).
pub(super) fn store_addr(inner: &Inner, addr: &EndpointAddr) {
    if addr.addrs.is_empty() {
        return;
    }
    let node = addr.id.to_string();
    let mut addrs: Vec<TransportAddr> = addr.addrs.iter().cloned().collect();
    if let Some(old) = inner
        .db
        .contact_addr(&node)
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str::<EndpointAddr>(&j).ok())
        .filter(|a| a.id == addr.id)
    {
        for a in old.addrs {
            if !addrs.contains(&a) {
                addrs.push(a);
            }
        }
    }
    addrs.truncate(8);
    if let Ok(json) = serde_json::to_string(&EndpointAddr::from_parts(addr.id, addrs)) {
        let _ = inner.db.set_contact_addr(&node, &json);
    }
}

/// Remembers where an authenticated contact connected from.
fn learn_from_conn(inner: &Inner, conn: &Connection, remote: EndpointId) {
    let addrs: Vec<TransportAddr> = conn
        .paths()
        .iter()
        .map(|p| p.remote_addr().clone())
        .collect();
    if !addrs.is_empty() {
        store_addr(inner, &EndpointAddr::from_parts(remote, addrs));
    }
}

/// Which path the connection is using right now.
pub(super) fn conn_info(conn: &Connection) -> Option<ConnInfo> {
    let paths = conn.paths();
    let selected = paths.iter().find(|p| p.is_selected())?;
    Some(match selected.remote_addr() {
        TransportAddr::Relay(url) => ConnInfo {
            kind: ConnKind::Relay,
            relay: url.host_str().map(str::to_owned),
        },
        _ => ConnInfo {
            kind: ConnKind::Direct,
            relay: None,
        },
    })
}

impl Inner {
    /// Returns a cached connection to `peer` or opens a new one.
    pub(super) async fn connect(&self, peer: EndpointId) -> Result<Connection, NetError> {
        {
            let conns = self.conns.lock().await;
            if let Some(c) = conns.get(&peer)
                && c.close_reason().is_none()
            {
                return Ok(c.clone());
            }
        }
        let addr = self
            .db
            .contact_addr(&peer.to_string())
            .ok()
            .flatten()
            .and_then(|json| serde_json::from_str::<EndpointAddr>(&json).ok())
            .filter(|a| a.id == peer)
            .unwrap_or_else(|| EndpointAddr::new(peer));
        self.connect_addr(addr).await
    }

    pub(super) async fn connect_addr(&self, addr: EndpointAddr) -> Result<Connection, NetError> {
        let peer = addr.id;
        let conn = tokio::time::timeout(CONNECT_TIMEOUT, self.endpoint.connect(addr, ALPN))
            .await
            .map_err(|_| NetError::Connect("timed out".into()))?
            .map_err(|e| NetError::Connect(e.to_string()))?;
        self.conns.lock().await.insert(peer, conn.clone());
        Ok(conn)
    }

    /// Sends one request on a new stream and reads the response header.
    /// Returns the receive side for responses followed by raw bytes.
    pub(super) async fn open_request(
        &self,
        conn: &Connection,
        req: &PeerRequest,
    ) -> Result<(PeerResponse, RecvStream), NetError> {
        let exchange = async {
            let (mut send, mut recv) = conn
                .open_bi()
                .await
                .map_err(|e| NetError::Connect(e.to_string()))?;
            write_msg(&mut send, req).await?;
            let _ = send.finish();
            let env = read_msg::<_, PeerResponse>(&mut recv).await?;
            Ok::<_, NetError>((env, recv))
        };
        // A connection whose peer vanished can stay open for a while;
        // don't wait on it forever.
        let (env, recv) = tokio::time::timeout(ANSWER_TIMEOUT, exchange)
            .await
            .map_err(|_| NetError::Connect("no answer".into()))??;
        if env.v != PROTOCOL_VERSION {
            return Err(WireError::Incompatible {
                peer: env.v,
                ours: PROTOCOL_VERSION,
            }
            .into());
        }
        match env.body {
            PeerResponse::Error { code, message } => {
                if code == ErrorCode::IncompatibleVersion {
                    return Err(NetError::Remote {
                        code,
                        message: format!(
                            "the other device runs an incompatible version of Gigapair ({message}); update both devices to the same version"
                        ),
                    });
                }
                Err(NetError::Remote { code, message })
            }
            body => Ok((body, recv)),
        }
    }

    /// Connects to `peer` and makes one request.
    pub(super) async fn request(
        &self,
        peer: EndpointId,
        req: &PeerRequest,
    ) -> Result<PeerResponse, NetError> {
        let conn = self.connect(peer).await?;
        match self.open_request(&conn, req).await {
            // A cached connection may have died silently: retry once fresh.
            Err(NetError::Connect(_) | NetError::Wire(WireError::Io(_))) => {
                self.forget_conn(peer).await;
                let conn = self.connect(peer).await?;
                match self.open_request(&conn, req).await {
                    Ok((r, _)) => Ok(r),
                    Err(e) => {
                        if !matches!(e, NetError::Remote { .. }) {
                            self.forget_conn(peer).await;
                        }
                        Err(e)
                    }
                }
            }
            other => Ok(other?.0),
        }
    }

    /// Drops the cached connection to `peer` so the next attempt dials
    /// again.
    pub(super) async fn forget_conn(&self, peer: EndpointId) {
        if let Some(c) = self.conns.lock().await.remove(&peer) {
            c.close(0u32.into(), b"reconnecting");
        }
    }

    /// Sends a request in the background, retrying for about a minute.
    pub(super) fn notify_peer(self: &Arc<Self>, peer: String, req: PeerRequest) {
        let inner = self.clone();
        tokio::spawn(async move {
            let Ok(peer_id) = peer.parse::<EndpointId>() else {
                return;
            };
            let mut delay = Duration::from_secs(1);
            for _ in 0..7 {
                match inner.request(peer_id, &req).await {
                    Ok(_) => return,
                    Err(NetError::Remote { .. }) => return,
                    Err(e) => tracing::debug!("notify {peer} failed: {e}"),
                }
                tokio::select! {
                    _ = inner.shutdown.cancelled() => return,
                    _ = tokio::time::sleep(delay) => {}
                }
                delay *= 2;
            }
        });
    }
}

pub(super) async fn accept_loop(inner: Arc<Inner>) {
    loop {
        let incoming = tokio::select! {
            _ = inner.shutdown.cancelled() => return,
            inc = inner.endpoint.accept() => inc,
        };
        let Some(incoming) = incoming else { return };
        let inner = inner.clone();
        tokio::spawn(async move {
            let accepting = match incoming.accept() {
                Ok(a) => a,
                Err(e) => {
                    tracing::debug!("incoming connection failed: {e}");
                    return;
                }
            };
            match accepting.await {
                Ok(conn) => handle_conn(inner, conn).await,
                Err(e) => tracing::debug!("incoming connection failed: {e}"),
            }
        });
    }
}

async fn handle_conn(inner: Arc<Inner>, conn: Connection) {
    let remote = conn.remote_id();
    loop {
        let (send, recv) = tokio::select! {
            _ = inner.shutdown.cancelled() => return,
            s = conn.accept_bi() => match s {
                Ok(s) => s,
                Err(_) => return,
            },
        };
        let inner = inner.clone();
        let conn = conn.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_stream(&inner, &conn, remote, send, recv).await {
                tracing::debug!(peer = %remote.fmt_short(), "request failed: {e}");
            }
        });
    }
}

async fn reply(send: &mut SendStream, resp: &PeerResponse) -> Result<(), NetError> {
    write_msg(send, resp).await?;
    let _ = send.finish();
    Ok(())
}

async fn handle_stream(
    inner: &Arc<Inner>,
    conn: &Connection,
    remote: EndpointId,
    mut send: SendStream,
    mut recv: RecvStream,
) -> Result<(), NetError> {
    let env = match read_msg::<_, PeerRequest>(&mut recv).await {
        Ok(env) => env,
        Err(WireError::Incompatible { .. }) => {
            return reply(
                &mut send,
                &PeerResponse::error(
                    ErrorCode::IncompatibleVersion,
                    format!("this device speaks protocol {PROTOCOL_VERSION}"),
                ),
            )
            .await;
        }
        Err(e) => return Err(e.into()),
    };
    if env.v != PROTOCOL_VERSION {
        return reply(
            &mut send,
            &PeerResponse::error(
                ErrorCode::IncompatibleVersion,
                format!("this device speaks protocol {PROTOCOL_VERSION}"),
            ),
        )
        .await;
    }

    let peer = remote.to_string();
    let resp = match env.body {
        PeerRequest::Pair {
            secret,
            device_name,
            addr,
        } => pair::handle_pair(inner, remote, &secret, &device_name, addr),
        PeerRequest::Unpair => pair::handle_unpair(inner, remote),
        _ if !inner.is_contact(&remote) => PeerResponse::error(
            ErrorCode::NotPaired,
            "this device is not paired with you; pair again to send files",
        ),
        PeerRequest::Ping { addr } => {
            learn_from_conn(inner, conn, remote);
            if let Some(addr) = addr.filter(|a| a.id == remote) {
                store_addr(inner, &addr);
            }
            inner.set_online(&peer, true);
            inner.wake_peer(&peer);
            PeerResponse::Pong {
                device_name: inner.device_name(),
            }
        }
        PeerRequest::Offer(offer) => {
            learn_from_conn(inner, conn, remote);
            inner.set_online(&peer, true);
            recv::handle_offer(inner, remote, offer)
        }
        PeerRequest::Decision {
            transfer_id,
            accepted,
            reason,
        } => send::handle_decision(inner, &peer, &transfer_id, accepted, reason),
        PeerRequest::Notice {
            transfer_id,
            notice,
        } => {
            if send::owns(inner, &peer, &transfer_id) {
                send::handle_notice(inner, &transfer_id, notice)
            } else if recv::owns(inner, &peer, &transfer_id) {
                recv::handle_notice(inner, &transfer_id, notice)
            } else {
                PeerResponse::error(ErrorCode::UnknownTransfer, "unknown transfer")
            }
        }
        PeerRequest::GetHashes { transfer_id, file } => {
            return send::serve_hashes(inner, &peer, &transfer_id, file, &mut send).await;
        }
        PeerRequest::Get {
            transfer_id,
            file,
            start_block,
        } => {
            inner.set_online(&peer, true);
            return send::serve_get(inner, conn, &peer, &transfer_id, file, start_block, send)
                .await;
        }
    };
    reply(&mut send, &resp).await
}

/// Pings every contact periodically to show who is online, and wakes
/// waiting transfers when a contact comes back.
pub(super) async fn ping_loop(inner: Arc<Inner>) {
    // Let the endpoint find its relay first.
    tokio::select! {
        _ = inner.shutdown.cancelled() => return,
        _ = tokio::time::sleep(Duration::from_secs(2)) => {}
    }
    loop {
        let contacts = inner.db.contacts().unwrap_or_default();
        let mut tasks = Vec::new();
        for c in contacts {
            let Ok(id) = c.node_id.parse::<EndpointId>() else {
                continue;
            };
            let inner = inner.clone();
            tasks.push(tokio::spawn(async move {
                let req = PeerRequest::Ping {
                    addr: Some(advertised_addr(&inner, true)),
                };
                let ok = matches!(
                    tokio::time::timeout(Duration::from_secs(15), inner.request(id, &req)).await,
                    Ok(Ok(PeerResponse::Pong { .. }))
                );
                let was = inner.is_online(&c.node_id);
                inner.set_online(&c.node_id, ok);
                if ok && !was {
                    inner.wake_peer(&c.node_id);
                }
            }));
        }
        for t in tasks {
            let _ = t.await;
        }
        tokio::select! {
            _ = inner.shutdown.cancelled() => return,
            _ = tokio::time::sleep(PING_INTERVAL) => {}
        }
    }
}
