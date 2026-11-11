//! The transfer engine: owns the iroh endpoint, the persisted state and
//! every running transfer. `gigapaird` wraps it with IPC and OS
//! integration; tests run two engines in one process.
//!
//! Data flow: the receiver pulls. After an offer is accepted, the receiver
//! asks the sender for each file's block list and then for the data from
//! its first missing block, verifies each block as it arrives, and
//! checkpoints progress in SQLite. After a drop, a restart or a reboot it
//! asks again from the checkpoint, so verified blocks are never sent twice
//! (RF-05).

mod api;
mod net;
mod pair;
mod recv;
mod send;

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use iroh::{Endpoint, EndpointId, SecretKey};
use tokio::sync::{Notify, Semaphore, broadcast, watch};
use tokio_util::sync::CancellationToken;

use crate::api::Event;
use crate::config::Config;
use crate::db::{Db, DbError, TransferRow};
use crate::dirs::AppDirs;
use crate::pairing::PairSecret;
use crate::types::{
    ConnInfo, Contact, Direction, PendingPair, TransferInfo, TransferStatus, now_secs,
};

pub use net::NetError;

/// How the engine reaches the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkMode {
    /// Relays and discovery as configured. The normal mode.
    Internet,
    /// Loopback only, no relays, no discovery. For tests.
    LocalOnly,
}

pub struct EngineOptions {
    pub dirs: AppDirs,
    pub config: Config,
    pub secret_key: SecretKey,
    pub network: NetworkMode,
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("{0}")]
    Db(#[from] DbError),
    #[error("could not start networking: {0}")]
    Bind(String),
    #[error("{0}")]
    User(String),
}

impl EngineError {
    pub(crate) fn user(msg: impl Into<String>) -> Self {
        Self::User(msg.into())
    }
}

pub type EngineResult<T> = Result<T, EngineError>;

/// Handle to a running engine. Cheap to clone.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

pub(crate) struct Inner {
    pub endpoint: Endpoint,
    pub db: Db,
    pub dirs: AppDirs,
    pub config: Mutex<Config>,
    pub network: NetworkMode,
    pub events: broadcast::Sender<Event>,
    pub pair_codes: Mutex<HashMap<PairSecret, i64>>,
    pub pending_pairs: Mutex<HashMap<String, pair::PendingEntry>>,
    pub live: Mutex<HashMap<String, Live>>,
    pub conns: tokio::sync::Mutex<HashMap<EndpointId, iroh::endpoint::Connection>>,
    pub online: Mutex<HashMap<String, bool>>,
    pub download_slots: Arc<Semaphore>,
    pub slot_count: AtomicU64,
    pub shutdown: CancellationToken,
    pub active_tx: watch::Sender<bool>,
    pub keeping_awake: AtomicBool,
    pub task_gen: AtomicU64,
}

/// In-memory state of a transfer: progress, speed and its running task.
#[derive(Default)]
pub(crate) struct Live {
    /// Token and generation of the running task, if any.
    pub task: Option<(u64, CancellationToken)>,
    /// Cancels the data being served to the receiver (sender side).
    pub serving: Option<(u64, CancellationToken)>,
    pub done_bytes: Option<u64>,
    pub speed: Speed,
    pub conn: Option<ConnInfo>,
    pub note: Option<String>,
    pub wake: Arc<Notify>,
    pub last_emit: Option<Instant>,
    pub last_persist: Option<Instant>,
}

/// Transfer rate over the last few seconds.
#[derive(Default)]
pub(crate) struct Speed {
    samples: VecDeque<(Instant, u64)>,
}

impl Speed {
    const WINDOW: Duration = Duration::from_secs(5);

    pub fn record(&mut self, bytes: u64) {
        let now = Instant::now();
        // A drop in the counter (restart from a checkpoint) resets the window.
        if self.samples.back().is_some_and(|&(_, b)| bytes < b) {
            self.samples.clear();
        }
        self.samples.push_back((now, bytes));
        while self
            .samples
            .front()
            .is_some_and(|&(t, _)| now.duration_since(t) > Self::WINDOW)
            && self.samples.len() > 2
        {
            self.samples.pop_front();
        }
    }

    pub fn reset(&mut self) {
        self.samples.clear();
    }

    pub fn rate(&self) -> u64 {
        match (self.samples.front(), self.samples.back()) {
            (Some(&(t0, b0)), Some(&(t1, b1))) if t1 > t0 => {
                // Stale: nothing recorded for a while.
                if t1.elapsed() > Duration::from_secs(3) {
                    return 0;
                }
                let dt = t1.duration_since(t0).as_secs_f64();
                ((b1.saturating_sub(b0)) as f64 / dt) as u64
            }
            _ => 0,
        }
    }
}

impl Engine {
    /// Opens the database, binds the endpoint and resumes unfinished
    /// transfers.
    pub async fn start(opts: EngineOptions) -> EngineResult<Self> {
        opts.dirs
            .create_all()
            .map_err(|e| EngineError::user(format!("could not create Gigapair folders: {e}")))?;
        let db = Db::open(&opts.dirs.db_file())?;
        import_seed_contacts(&db, &opts.config);

        let endpoint = net::bind_endpoint(
            &opts.config,
            opts.secret_key,
            opts.network,
            &opts.dirs.data.join("udp-port"),
        )
        .await?;
        let (events, _) = broadcast::channel(1024);
        let (active_tx, _) = watch::channel(false);
        let slots = opts.config.max_parallel_downloads.max(1);

        let inner = Arc::new(Inner {
            endpoint,
            db,
            dirs: opts.dirs,
            config: Mutex::new(opts.config),
            network: opts.network,
            events,
            pair_codes: Mutex::new(HashMap::new()),
            pending_pairs: Mutex::new(HashMap::new()),
            live: Mutex::new(HashMap::new()),
            conns: tokio::sync::Mutex::new(HashMap::new()),
            online: Mutex::new(HashMap::new()),
            download_slots: Arc::new(Semaphore::new(slots as usize)),
            slot_count: AtomicU64::new(slots as u64),
            shutdown: CancellationToken::new(),
            active_tx,
            keeping_awake: AtomicBool::new(false),
            task_gen: AtomicU64::new(0),
        });

        tokio::spawn(net::accept_loop(inner.clone()));
        tokio::spawn(net::ping_loop(inner.clone()));
        inner.resume_all();
        inner.update_active();
        tracing::info!(node = %inner.endpoint.id(), "engine started");
        Ok(Self { inner })
    }

    pub fn node_id(&self) -> EndpointId {
        self.inner.endpoint.id()
    }

    /// Full address, including direct addresses. Tests use it to connect
    /// engines without discovery.
    pub fn addr(&self) -> iroh::EndpointAddr {
        self.inner.endpoint.addr()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.inner.events.subscribe()
    }

    /// True while any transfer is moving or verifying data.
    pub fn watch_active(&self) -> watch::Receiver<bool> {
        self.inner.active_tx.subscribe()
    }

    /// Records whether the service is holding the keep-awake guard.
    pub fn set_keeping_awake(&self, on: bool) {
        if self.inner.keeping_awake.swap(on, Ordering::Relaxed) != on {
            self.inner.emit(Event::KeepAwake { active: on });
        }
    }

    pub fn config(&self) -> Config {
        self.inner.config().clone()
    }

    /// Stops all tasks and closes the endpoint. Progress is already
    /// checkpointed, so everything resumes on the next start.
    pub async fn shutdown(&self) {
        self.inner.shutdown.cancel();
        // Give tasks a moment to write their last checkpoint.
        tokio::time::sleep(Duration::from_millis(300)).await;
        self.inner.endpoint.close().await;
    }

    pub fn is_shut_down(&self) -> bool {
        self.inner.shutdown.is_cancelled()
    }
}

fn import_seed_contacts(db: &Db, config: &Config) {
    for c in &config.contacts {
        match c.node_id.parse::<EndpointId>() {
            Ok(id) => {
                if let Err(e) = db.upsert_contact(&id.to_string(), &c.name, c.auto_accept) {
                    tracing::warn!("could not import contact {:?}: {e}", c.name);
                }
            }
            Err(_) => tracing::warn!(
                "config.toml: contact {:?} has an invalid node_id; skipped",
                c.name
            ),
        }
    }
}

impl Inner {
    pub fn config(&self) -> MutexGuard<'_, Config> {
        self.config.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn live(&self) -> MutexGuard<'_, HashMap<String, Live>> {
        self.live.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }

    pub fn device_name(&self) -> String {
        self.config().effective_device_name()
    }

    pub fn download_dir(&self) -> std::path::PathBuf {
        self.config().effective_download_dir(&self.dirs.downloads)
    }

    pub fn save_config(&self) -> EngineResult<()> {
        let cfg = self.config().clone();
        cfg.save(&self.dirs.config_file())
            .map_err(|e| EngineError::user(e.to_string()))
    }

    pub fn is_online(&self, node: &str) -> bool {
        self.online
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(node)
            .copied()
            .unwrap_or(false)
    }

    /// Marks a contact online/offline; emits when it changes.
    pub fn set_online(&self, node: &str, on: bool) {
        let changed = {
            let mut map = self.online.lock().unwrap_or_else(|p| p.into_inner());
            map.insert(node.to_owned(), on) != Some(on)
        };
        if on {
            let _ = self.db.touch_contact(node);
        }
        if changed {
            self.emit(Event::ContactsChanged);
        }
    }

    pub fn contacts(&self) -> EngineResult<Vec<Contact>> {
        let mut list = self.db.contacts()?;
        for c in &mut list {
            c.online = self.is_online(&c.node_id);
        }
        Ok(list)
    }

    pub fn is_contact(&self, node: &EndpointId) -> bool {
        matches!(self.db.contact(&node.to_string()), Ok(Some(_)))
    }

    pub fn info(&self, row: &TransferRow) -> TransferInfo {
        let live = self.live();
        let l = live.get(&row.id);
        let done = l
            .and_then(|l| l.done_bytes)
            .unwrap_or(row.done_bytes)
            .min(row.total_size.max(row.done_bytes));
        let moving = matches!(
            row.status,
            TransferStatus::Transferring | TransferStatus::Preparing
        );
        let speed = if moving {
            l.map(|l| l.speed.rate()).unwrap_or(0)
        } else {
            0
        };
        let eta = (speed > 0 && row.total_size > done).then(|| (row.total_size - done) / speed);
        TransferInfo {
            id: row.id.clone(),
            direction: row.direction,
            peer_id: row.peer.clone(),
            peer_name: row.peer_name.clone(),
            status: row.status,
            label: row.label.clone(),
            total_bytes: row.total_size,
            done_bytes: if row.status == TransferStatus::Completed {
                row.total_size
            } else {
                done
            },
            file_count: row.file_count,
            speed_bps: speed,
            eta_secs: eta,
            conn: l.and_then(|l| l.conn.clone()).or_else(|| row.conn.clone()),
            error: row.error.clone(),
            note: if row.status.is_terminal() {
                None
            } else {
                l.and_then(|l| l.note.clone())
            },
            dest_dir: row.dest_dir.clone(),
            created_at: row.created_at,
            finished_at: row.finished_at,
        }
    }

    pub fn transfer_info(&self, id: &str) -> Option<TransferInfo> {
        self.db
            .transfer(id)
            .ok()
            .flatten()
            .map(|row| self.info(&row))
    }

    pub fn emit_transfer(&self, id: &str) {
        if let Some(info) = self.transfer_info(id) {
            self.emit(Event::Transfer(info));
        }
    }

    /// Persists a status change and notifies clients.
    pub fn set_status(&self, id: &str, status: TransferStatus) {
        if let Err(e) = self.db.set_status(id, status) {
            tracing::error!("could not save status of {id}: {e}");
        }
        if status.is_terminal() || status != TransferStatus::Transferring {
            let mut live = self.live();
            if let Some(l) = live.get_mut(id) {
                l.speed.reset();
            }
        }
        self.update_active();
        self.emit_transfer(id);
    }

    pub fn fail(&self, id: &str, message: &str) {
        tracing::warn!(transfer = id, "transfer failed: {message}");
        let _ = self.db.set_error(id, Some(message));
        self.set_note(id, None);
        self.set_status(id, TransferStatus::Failed);
    }

    pub fn set_note(&self, id: &str, note: Option<String>) {
        {
            let mut live = self.live();
            let l = live.entry(id.to_owned()).or_default();
            if l.note == note {
                return;
            }
            l.note = note;
        }
        self.emit_transfer(id);
    }

    pub fn set_conn(&self, id: &str, conn: Option<ConnInfo>) {
        let changed = {
            let mut live = self.live();
            let l = live.entry(id.to_owned()).or_default();
            let changed = l.conn != conn;
            l.conn = conn.clone();
            changed
        };
        if changed {
            let _ = self.db.set_conn(id, conn.as_ref());
            if let Some(c) = &conn
                && c.kind == crate::types::ConnKind::Relay
            {
                self.relay_first_use(c.relay.clone().unwrap_or_default());
            }
            self.emit_transfer(id);
        }
    }

    fn relay_first_use(&self, relay: String) {
        let first = {
            let mut cfg = self.config();
            let first = !cfg.relay_notice_shown;
            cfg.relay_notice_shown = true;
            first
        };
        if first {
            let _ = self.save_config();
            self.emit(Event::RelayFirstUse { relay });
        }
    }

    /// Updates live progress; emits at most 4 times per second and
    /// persists every 2 seconds.
    pub fn progress(&self, id: &str, done: u64) {
        let (emit, persist) = {
            let mut live = self.live();
            let l = live.entry(id.to_owned()).or_default();
            l.done_bytes = Some(done);
            l.speed.record(done);
            let now = Instant::now();
            let emit = l
                .last_emit
                .is_none_or(|t| now.duration_since(t) >= Duration::from_millis(250));
            if emit {
                l.last_emit = Some(now);
            }
            let persist = l
                .last_persist
                .is_none_or(|t| now.duration_since(t) >= Duration::from_secs(2));
            if persist {
                l.last_persist = Some(now);
            }
            (emit, persist)
        };
        if persist {
            let _ = self.db.set_done_bytes(id, done);
        }
        if emit {
            self.emit_transfer(id);
        }
    }

    /// Saves the live progress counter, e.g. before a status change.
    pub fn flush_progress(&self, id: &str) {
        let done = self.live().get(id).and_then(|l| l.done_bytes);
        if let Some(done) = done {
            let _ = self.db.set_done_bytes(id, done);
        }
    }

    pub fn wake_handle(&self, id: &str) -> Arc<Notify> {
        self.live().entry(id.to_owned()).or_default().wake.clone()
    }

    /// Wakes tasks of `peer` that are waiting to retry.
    pub fn wake_peer(&self, peer: &str) {
        let ids: Vec<String> = match self.db.active_transfers() {
            Ok(rows) => rows
                .into_iter()
                .filter(|r| r.peer == peer)
                .map(|r| r.id)
                .collect(),
            Err(_) => return,
        };
        let live = self.live();
        for id in ids {
            if let Some(l) = live.get(&id) {
                l.wake.notify_one();
            }
        }
    }

    pub fn update_active(&self) {
        let active = self
            .db
            .active_transfers()
            .map(|rows| rows.iter().any(|r| r.status.is_active()))
            .unwrap_or(false);
        self.active_tx.send_if_modified(|v| {
            let changed = *v != active;
            *v = active;
            changed
        });
    }

    /// Runs `f` as the transfer's task unless one is already running.
    pub fn spawn_task<F, Fut>(self: &Arc<Self>, id: &str, f: F)
    where
        F: FnOnce(Arc<Inner>, CancellationToken) -> Fut,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let token = self.shutdown.child_token();
        let gen_id = self.task_gen.fetch_add(1, Ordering::Relaxed);
        {
            let mut live = self.live();
            let l = live.entry(id.to_owned()).or_default();
            if l.task.is_some() {
                return;
            }
            l.task = Some((gen_id, token.clone()));
        }
        let fut = f(self.clone(), token);
        let inner = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            fut.await;
            let mut live = inner.live();
            if let Some(l) = live.get_mut(&id)
                && l.task.as_ref().is_some_and(|(g, _)| *g == gen_id)
            {
                l.task = None;
            }
        });
    }

    /// Stops the transfer's task and any data being served.
    pub fn stop_task(&self, id: &str) {
        let mut live = self.live();
        if let Some(l) = live.get_mut(id) {
            if let Some((_, t)) = l.task.take() {
                t.cancel();
            }
            if let Some((_, t)) = l.serving.take() {
                t.cancel();
            }
        }
    }

    /// Restarts work for every unfinished transfer after a start.
    fn resume_all(self: &Arc<Self>) {
        let rows = match self.db.active_transfers() {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("could not load transfers: {e}");
                return;
            }
        };
        for row in rows {
            match (row.direction, row.status) {
                (_, TransferStatus::Paused) | (_, TransferStatus::PendingAccept) => {}
                (Direction::Send, TransferStatus::Preparing) => send::start_prepare(self, &row.id),
                (Direction::Send, status) => {
                    if status == TransferStatus::Transferring {
                        let _ = self.db.set_status(&row.id, TransferStatus::Queued);
                    }
                    send::start_offer(self, &row.id);
                }
                (Direction::Receive, status) => {
                    if status != TransferStatus::Queued {
                        let _ = self.db.set_status(&row.id, TransferStatus::Queued);
                    }
                    recv::start_download(self, &row.id);
                }
            }
        }
    }

    pub fn pending_pairs(&self) -> Vec<PendingPair> {
        let now = now_secs();
        let mut map = self.pending_pairs.lock().unwrap_or_else(|p| p.into_inner());
        map.retain(|_, e| e.info.expires_at > now);
        let mut list: Vec<_> = map.values().map(|e| e.info.clone()).collect();
        list.sort_by_key(|p| p.expires_at);
        list
    }
}

/// Exponential backoff between retries: 1 s doubling up to 60 s.
pub(crate) struct Backoff {
    next: Duration,
}

impl Backoff {
    pub fn new() -> Self {
        Self {
            next: Duration::from_secs(1),
        }
    }

    pub fn reset(&mut self) {
        self.next = Duration::from_secs(1);
    }

    pub fn next_delay(&mut self) -> Duration {
        let d = self.next;
        self.next = (self.next * 2).min(Duration::from_secs(60));
        d
    }
}

/// Waits for `delay`, a wake-up, or cancellation. Returns false if
/// cancelled.
pub(crate) async fn wait(token: &CancellationToken, wake: &Notify, delay: Duration) -> bool {
    tokio::select! {
        _ = token.cancelled() => false,
        _ = wake.notified() => true,
        _ = tokio::time::sleep(delay) => true,
    }
}

/// New random transfer id: 32 lowercase hex characters.
pub(crate) fn new_id() -> String {
    let bytes: [u8; 16] = rand::random();
    data_encoding::HEXLOWER.encode(&bytes)
}
