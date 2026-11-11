//! Receiver side: offers, downloading with per-block verification,
//! resuming, the final whole-file check and moving files into place.

use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use iroh::EndpointId;
use iroh::endpoint::Connection;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::net::{NetError, conn_info};
use super::{Backoff, EngineError, EngineResult, Inner, wait};
use crate::api::Event;
use crate::db::{Db, DirRow, FileRow, TransferRow};
use crate::manifest::{
    self, BLOCK_SIZE, HASH_LEN, block_count, block_len, hash_block, system_time_from_ns,
};
use crate::paths::{
    self, Platform, create_unique_dir, ensure_partial_dir, for_io, move_into_unique,
    safe_relative_path, sanitize_component, set_mtime, transfer_partial_dir,
};
use crate::types::{ConnKind, Direction, TransferStatus, format_bytes, now_secs};
use crate::wire::{ErrorCode, Notice, Offer, OfferState, PeerRequest, PeerResponse};

/// Free space to keep on top of the transfer size.
const SPACE_MARGIN: u64 = 64 * 1024 * 1024;
/// How long to wait for a direct path when "direct only" is on.
const DIRECT_WAIT: Duration = Duration::from_secs(15);
/// Give up after this many failed whole-file checks in a row.
const MAX_INTEGRITY_FAILURES: u32 = 3;
/// Give up on a stream that delivers nothing for this long.
const BLOCK_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) fn owns(inner: &Inner, peer: &str, id: &str) -> bool {
    matches!(inner.db.transfer(id), Ok(Some(r)) if r.direction == Direction::Receive && r.peer == peer)
}

fn offer_state(row: &TransferRow) -> OfferState {
    match row.status {
        TransferStatus::PendingAccept => OfferState::Pending,
        TransferStatus::Completed => OfferState::Completed,
        TransferStatus::Cancelled => OfferState::Cancelled,
        TransferStatus::Rejected => OfferState::Rejected {
            reason: row.error.clone().unwrap_or_else(|| "Rejected".into()),
        },
        TransferStatus::Failed => OfferState::Rejected {
            reason: format!(
                "the transfer failed on the receiving device: {}",
                row.error.clone().unwrap_or_default()
            ),
        },
        _ => OfferState::Accepted,
    }
}

fn validate_offer(offer: &Offer) -> Result<(), String> {
    if offer.files.is_empty() && offer.dirs.is_empty() {
        return Err("empty offer".into());
    }
    if offer.files.len() > 1_000_000 || offer.dirs.len() > 1_000_000 {
        return Err("too many files".into());
    }
    if offer.transfer_id.len() != 32 || !offer.transfer_id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid transfer id".into());
    }
    let mut total = 0u64;
    for f in &offer.files {
        safe_relative_path(&f.path, Platform::current()).map_err(|e| e.to_string())?;
        let hex_ok = |h: &str| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit());
        if !hex_ok(&f.root_hash) || !hex_ok(&f.blocks_hash) {
            return Err(format!("invalid hash for {}", f.path));
        }
        total = total
            .checked_add(f.size)
            .ok_or_else(|| "size overflow".to_string())?;
    }
    for d in &offer.dirs {
        safe_relative_path(&d.path, Platform::current()).map_err(|e| e.to_string())?;
    }
    if total != offer.total_size {
        return Err("sizes don't add up".into());
    }
    Ok(())
}

fn check_space(dest: &Path, needed: u64) -> Result<(), String> {
    match paths::available_space(dest) {
        Ok(free) if free < needed.saturating_add(SPACE_MARGIN) => Err(format!(
            "Not enough free space in {}: needs {}, only {} free",
            dest.display(),
            format_bytes(needed),
            format_bytes(free)
        )),
        Ok(_) => Ok(()),
        Err(e) => {
            tracing::warn!("could not check free space in {}: {e}", dest.display());
            Ok(())
        }
    }
}

pub(super) fn handle_offer(inner: &Arc<Inner>, remote: EndpointId, offer: Offer) -> PeerResponse {
    let peer = remote.to_string();
    if let Ok(Some(row)) = inner.db.transfer(&offer.transfer_id) {
        if row.peer != peer || row.direction != Direction::Receive {
            return PeerResponse::error(ErrorCode::BadRequest, "transfer id already in use");
        }
        return PeerResponse::Offer(offer_state(&row));
    }
    if let Err(e) = validate_offer(&offer) {
        return PeerResponse::error(ErrorCode::BadRequest, format!("invalid offer: {e}"));
    }
    let Ok(Some(contact)) = inner.db.contact(&peer) else {
        return PeerResponse::error(ErrorCode::NotPaired, "not paired");
    };

    let dest = inner.download_dir();
    let mut roots: Vec<String> = Vec::new();
    let mut root_is_dir: Vec<bool> = Vec::new();
    for path in offer
        .dirs
        .iter()
        .map(|d| (&d.path, true))
        .chain(offer.files.iter().map(|f| (&f.path, false)))
    {
        let mut parts = path.0.split('/').filter(|p| !p.is_empty());
        let first = parts.next().unwrap_or_default().to_owned();
        let is_dir = path.1 || parts.next().is_some();
        if !roots.contains(&first) {
            roots.push(first);
            root_is_dir.push(is_dir);
        }
    }
    let row = TransferRow {
        id: offer.transfer_id.clone(),
        direction: Direction::Receive,
        peer: peer.clone(),
        peer_name: contact.name.clone(),
        status: TransferStatus::PendingAccept,
        label: {
            // Prefer the sender's label so both sides show the same text.
            let l: String = offer
                .label
                .chars()
                .filter(|c| !c.is_control())
                .take(200)
                .collect();
            if l.trim().is_empty() {
                manifest::make_label(&roots, &root_is_dir)
            } else {
                l
            }
        },
        total_size: offer.total_size,
        file_count: offer.files.len() as u64,
        done_bytes: 0,
        dest_dir: Some(dest.display().to_string()),
        conn: None,
        error: None,
        created_at: now_secs(),
        finished_at: None,
        sources: Vec::new(),
    };
    let files: Vec<FileRow> = offer
        .files
        .iter()
        .enumerate()
        .map(|(i, f)| FileRow {
            idx: i as u32,
            rel_path: f.path.clone(),
            size: f.size,
            mtime_ns: f.mtime_ns,
            root_hash: Some(f.root_hash.to_ascii_lowercase()),
            blocks_hash: Some(f.blocks_hash.to_ascii_lowercase()),
            block_hashes: None,
            src_path: None,
            verified_blocks: 0,
            done: false,
            final_path: None,
        })
        .collect();
    let dirs: Vec<DirRow> = offer
        .dirs
        .iter()
        .map(|d| DirRow {
            rel_path: d.path.clone(),
            mtime_ns: d.mtime_ns,
        })
        .collect();
    if let Err(e) = inner.db.insert_transfer(&row, &files, &dirs) {
        return PeerResponse::error(ErrorCode::Internal, e.to_string());
    }
    let id = row.id.clone();

    if let Err(reason) = check_space(&dest, offer.total_size) {
        let _ = inner.db.set_error(&id, Some(&reason));
        inner.set_status(&id, TransferStatus::Rejected);
        inner.emit(Event::Notice {
            message: format!("Rejected {} from {}: {reason}", row.label, contact.name),
        });
        return PeerResponse::Offer(OfferState::Rejected {
            reason: format!("{reason} on {}", inner.device_name()),
        });
    }

    if contact.auto_accept {
        let _ = inner.db.set_status(&id, TransferStatus::Queued);
        inner.update_active();
        inner.emit_transfer(&id);
        start_download(inner, &id);
        PeerResponse::Offer(OfferState::Accepted)
    } else {
        if let Some(info) = inner.transfer_info(&id) {
            inner.emit(Event::IncomingRequest(info.clone()));
            inner.emit(Event::Transfer(info));
        }
        PeerResponse::Offer(OfferState::Pending)
    }
}

pub(super) fn accept(inner: &Arc<Inner>, id: &str, dest: Option<PathBuf>) -> EngineResult<()> {
    let row = inner
        .db
        .transfer(id)?
        .filter(|r| r.direction == Direction::Receive)
        .ok_or_else(|| EngineError::user("no such incoming transfer"))?;
    if row.status != TransferStatus::PendingAccept {
        return Err(EngineError::user(format!(
            "this request was already answered ({})",
            row.status.label().to_lowercase()
        )));
    }
    let dest = dest
        .or_else(|| row.dest_dir.as_ref().map(PathBuf::from))
        .unwrap_or_else(|| inner.download_dir());
    if !dest.is_absolute() {
        return Err(EngineError::user("choose an absolute destination folder"));
    }
    std::fs::create_dir_all(for_io(&dest))
        .map_err(|e| EngineError::user(format!("could not create {}: {e}", dest.display())))?;
    check_space(&dest, row.total_size)
        .map_err(|e| EngineError::user(format!("{e}. Free up space or choose another folder")))?;
    inner.db.set_dest_dir(id, &dest.display().to_string())?;
    inner.set_status(id, TransferStatus::Queued);
    start_download(inner, id);
    inner.notify_peer(
        row.peer.clone(),
        PeerRequest::Decision {
            transfer_id: id.to_owned(),
            accepted: true,
            reason: None,
        },
    );
    Ok(())
}

pub(super) fn reject(inner: &Arc<Inner>, id: &str) -> EngineResult<()> {
    let row = inner
        .db
        .transfer(id)?
        .filter(|r| r.direction == Direction::Receive)
        .ok_or_else(|| EngineError::user("no such incoming transfer"))?;
    if row.status != TransferStatus::PendingAccept {
        return Err(EngineError::user("this request was already answered"));
    }
    let reason = format!("Rejected by {}", inner.device_name());
    let _ = inner.db.set_error(id, Some("Rejected"));
    inner.set_status(id, TransferStatus::Rejected);
    inner.notify_peer(
        row.peer,
        PeerRequest::Decision {
            transfer_id: id.to_owned(),
            accepted: false,
            reason: Some(reason),
        },
    );
    Ok(())
}

pub(super) fn handle_notice(inner: &Arc<Inner>, id: &str, notice: Notice) -> PeerResponse {
    let Ok(Some(row)) = inner.db.transfer(id) else {
        return PeerResponse::error(ErrorCode::UnknownTransfer, "unknown transfer");
    };
    let name = &row.peer_name;
    match notice {
        Notice::Paused => inner.set_note(id, Some(format!("Paused by {name}"))),
        Notice::Resumed => {
            inner.set_note(id, None);
            inner.wake_handle(id).notify_one();
        }
        Notice::Cancelled => {
            if !row.status.is_terminal() {
                inner.stop_task(id);
                let _ = inner
                    .db
                    .set_error(id, Some(&format!("Cancelled by {name}")));
                inner.set_note(id, None);
                inner.set_status(id, TransferStatus::Cancelled);
                cleanup_partial(&row);
            }
        }
        Notice::Failed { reason } => {
            if !row.status.is_terminal() {
                inner.stop_task(id);
                inner.fail(id, &format!("Failed on {name}: {reason}"));
            }
        }
        Notice::Completed => {}
    }
    PeerResponse::Ok
}

/// Deletes the in-progress data of a transfer.
pub(super) fn cleanup_partial(row: &TransferRow) {
    let Some(dest) = row.dest_dir.as_ref().map(PathBuf::from) else {
        return;
    };
    let dir = transfer_partial_dir(&dest, &row.id);
    let _ = std::fs::remove_dir_all(for_io(&dir));
    // Remove `.gigapair-partial` too once nothing else is in it.
    let _ = std::fs::remove_dir(for_io(&paths::partial_dir(&dest)));
}

pub(super) fn start_download(inner: &Arc<Inner>, id: &str) {
    let id = id.to_owned();
    inner.spawn_task(&id.clone(), move |inner, token| async move {
        download_task(&inner, &id, &token).await;
    });
}

#[derive(Debug)]
enum DlError {
    /// Paused, cancelled locally or shutting down.
    Stopped,
    Retry(String),
    PeerPaused,
    Cancelled,
    Integrity(String),
    Fatal(String),
}

async fn download_task(inner: &Arc<Inner>, id: &str, token: &CancellationToken) {
    let permit = match inner.download_slots.clone().try_acquire_owned() {
        Ok(p) => p,
        Err(_) => {
            inner.set_note(id, Some("Waiting for other transfers to finish".into()));
            tokio::select! {
                _ = token.cancelled() => return,
                p = inner.download_slots.clone().acquire_owned() => match p {
                    Ok(p) => p,
                    Err(_) => return,
                },
            }
        }
    };
    let wake = inner.wake_handle(id);
    let mut backoff = Backoff::new();
    let mut integrity_failures = 0;
    loop {
        if token.is_cancelled() {
            break;
        }
        let before = inner.db.transfer(id).ok().flatten().map(|r| r.done_bytes);
        let result = run(inner, id, token).await;
        let after = inner.db.transfer(id).ok().flatten().map(|r| r.done_bytes);
        if after > before {
            backoff.reset();
        }
        let Ok(Some(row)) = inner.db.transfer(id) else {
            break;
        };
        match result {
            Ok(()) | Err(DlError::Stopped) => break,
            Err(DlError::Fatal(msg)) => {
                inner.fail(id, &msg);
                inner.notify_peer(
                    row.peer.clone(),
                    PeerRequest::Notice {
                        transfer_id: id.to_owned(),
                        notice: Notice::Failed { reason: msg },
                    },
                );
                break;
            }
            Err(DlError::Cancelled) => {
                let _ = inner
                    .db
                    .set_error(id, Some(&format!("Cancelled by {}", row.peer_name)));
                inner.set_note(id, None);
                inner.set_status(id, TransferStatus::Cancelled);
                cleanup_partial(&row);
                break;
            }
            Err(DlError::Integrity(msg)) => {
                integrity_failures += 1;
                if integrity_failures >= MAX_INTEGRITY_FAILURES {
                    inner.fail(
                        id,
                        &format!("{msg}. Gave up after {integrity_failures} attempts; check the disk and network, then retry"),
                    );
                    break;
                }
                tracing::warn!(transfer = id, "{msg}; downloading the damaged part again");
                inner.set_note(id, Some(format!("{msg}; downloading it again")));
            }
            Err(DlError::PeerPaused) => {
                requeue(inner, id, &row);
                inner.set_note(id, Some(format!("Paused by {}", row.peer_name)));
                if !wait(token, &wake, Duration::from_secs(60)).await {
                    break;
                }
            }
            Err(DlError::Retry(msg)) => {
                if let Ok(peer) = row.peer.parse::<EndpointId>() {
                    inner.forget_conn(peer).await;
                }
                requeue(inner, id, &row);
                let delay = backoff.next_delay();
                inner.set_note(id, Some(format!("{msg}; retrying")));
                if !wait(token, &wake, delay).await {
                    break;
                }
            }
        }
    }
    inner.flush_progress(id);
    drop(permit);
}

fn requeue(inner: &Arc<Inner>, id: &str, row: &TransferRow) {
    if matches!(
        row.status,
        TransferStatus::Transferring | TransferStatus::Verifying
    ) {
        inner.flush_progress(id);
        inner.set_status(id, TransferStatus::Queued);
    }
    let mut live = inner.live();
    if let Some(l) = live.get_mut(id) {
        l.conn = None;
    }
}

fn map_net(err: NetError, peer_name: &str) -> DlError {
    match err {
        NetError::Remote { code, message } => match code {
            ErrorCode::Paused => DlError::PeerPaused,
            ErrorCode::Cancelled => DlError::Cancelled,
            ErrorCode::DirectOnly => DlError::Retry(message),
            ErrorCode::UnknownTransfer => {
                DlError::Fatal(format!("{peer_name} no longer has this transfer"))
            }
            ErrorCode::NotPaired => DlError::Fatal(format!(
                "{peer_name} no longer has this device as a contact; pair again"
            )),
            _ => DlError::Fatal(message),
        },
        NetError::Wire(crate::wire::WireError::Incompatible { .. }) => {
            DlError::Fatal(err.to_string())
        }
        _ => DlError::Retry(format!("Connection to {peer_name} lost")),
    }
}

fn files_done_bytes(files: &[FileRow], except: Option<u32>) -> u64 {
    files
        .iter()
        .filter(|f| Some(f.idx) != except)
        .map(|f| {
            if f.done {
                f.size
            } else {
                (f.verified_blocks * BLOCK_SIZE).min(f.size)
            }
        })
        .sum()
}

async fn wait_direct(
    inner: &Inner,
    conn: &Connection,
    token: &CancellationToken,
    peer_name: &str,
) -> Result<(), DlError> {
    if !inner.config().direct_only {
        return Ok(());
    }
    let deadline = Instant::now() + DIRECT_WAIT;
    loop {
        if conn_info(conn).is_some_and(|c| c.kind == ConnKind::Direct) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(DlError::Retry(format!(
                "No direct connection to {peer_name} (direct only is on)"
            )));
        }
        tokio::select! {
            _ = token.cancelled() => return Err(DlError::Stopped),
            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
        }
    }
}

/// One attempt at downloading everything that is missing.
async fn run(inner: &Arc<Inner>, id: &str, token: &CancellationToken) -> Result<(), DlError> {
    let row = match inner.db.transfer(id) {
        Ok(Some(r)) => r,
        _ => return Err(DlError::Stopped),
    };
    if row.status.is_terminal()
        || matches!(
            row.status,
            TransferStatus::Paused | TransferStatus::PendingAccept
        )
    {
        return Err(DlError::Stopped);
    }
    let name = row.peer_name.clone();
    let dest = PathBuf::from(
        row.dest_dir
            .clone()
            .ok_or_else(|| DlError::Fatal("no destination folder".into()))?,
    );
    let files = inner
        .db
        .files(id)
        .map_err(|e| DlError::Fatal(e.to_string()))?;

    if files.iter().any(|f| !f.done) {
        let peer: EndpointId = row
            .peer
            .parse()
            .map_err(|_| DlError::Fatal("invalid contact id".into()))?;
        inner.set_note(id, Some(format!("Connecting to {name}")));
        let conn = tokio::select! {
            _ = token.cancelled() => return Err(DlError::Stopped),
            c = inner.connect(peer) => c.map_err(|_| DlError::Retry(format!("Waiting for {name} to come online")))?,
        };
        wait_direct(inner, &conn, token, &name).await?;
        inner.set_conn(id, conn_info(&conn));
        inner.set_note(id, None);
        inner.set_status(id, TransferStatus::Transferring);

        let tdir = ensure_partial_dir(&dest)
            .and_then(|p| {
                let t = p.join(id);
                std::fs::create_dir_all(for_io(&t)).map(|_| t)
            })
            .map_err(|e| DlError::Fatal(format!("could not write to {}: {e}", dest.display())))?;

        let count = files.len();
        for (pos, f) in files.iter().enumerate() {
            if f.done {
                continue;
            }
            if token.is_cancelled() {
                return Err(DlError::Stopped);
            }
            let hashes = match &f.block_hashes {
                Some(h) => h.clone(),
                None => fetch_hashes(inner, &conn, id, f, &name).await?,
            };
            let part = tdir.join(format!("{}.part", f.idx));
            let start = {
                let (part, hashes, size, verified) =
                    (part.clone(), hashes.clone(), f.size, f.verified_blocks);
                tokio::task::spawn_blocking(move || resume_point(&part, verified, &hashes, size))
                    .await
                    .map_err(|e| DlError::Fatal(e.to_string()))?
            };
            if start != f.verified_blocks {
                let _ = inner.db.set_verified_blocks(id, f.idx, start);
            }
            let current = inner
                .db
                .files(id)
                .map_err(|e| DlError::Fatal(e.to_string()))?;
            let others = files_done_bytes(&current, Some(f.idx));
            inner.progress(id, others + (start * BLOCK_SIZE).min(f.size));

            if start < block_count(f.size) {
                inner.set_status(id, TransferStatus::Transferring);
                fetch_data(
                    inner, &conn, id, f, start, hashes, &part, others, token, &name,
                )
                .await?;
                // The path may have changed while downloading.
                inner.set_conn(id, conn_info(&conn));
            } else {
                // Empty file, or everything already on disk.
                OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(for_io(&part))
                    .map_err(|e| {
                        DlError::Fatal(format!("could not write {}: {e}", part.display()))
                    })?;
            }

            verify_and_place(inner, id, f, &part, &dest, pos + 1, count, token).await?;
        }
    }

    finish(inner, id, &row, &dest).await
}

async fn fetch_hashes(
    inner: &Arc<Inner>,
    conn: &Connection,
    id: &str,
    f: &FileRow,
    name: &str,
) -> Result<Vec<u8>, DlError> {
    let req = PeerRequest::GetHashes {
        transfer_id: id.to_owned(),
        file: f.idx,
    };
    let (resp, mut recv) = inner
        .open_request(conn, &req)
        .await
        .map_err(|e| map_net(e, name))?;
    let PeerResponse::Hashes { count } = resp else {
        return Err(DlError::Fatal("unexpected answer from the sender".into()));
    };
    if count != block_count(f.size) {
        return Err(DlError::Fatal(format!(
            "the sender's block list for {} has the wrong length",
            f.rel_path
        )));
    }
    let mut buf = vec![0u8; count as usize * HASH_LEN];
    match tokio::time::timeout(BLOCK_TIMEOUT, recv.read_exact(&mut buf)).await {
        Ok(Ok(())) => {}
        _ => return Err(DlError::Retry(format!("Connection to {name} lost"))),
    }
    let expected = f.blocks_hash.clone().unwrap_or_default();
    if blake3::hash(&buf).to_hex().as_str() != expected {
        return Err(DlError::Fatal(format!(
            "the block list for {} does not match the offer",
            f.rel_path
        )));
    }
    inner
        .db
        .set_block_hashes(id, f.idx, &buf)
        .map_err(|e| DlError::Fatal(e.to_string()))?;
    Ok(buf)
}

/// First block that is not verified on disk. Blocks past the last
/// checkpoint that are already on disk and match their hash are kept, so
/// a crash between checkpoints costs nothing.
fn resume_point(part: &Path, verified: u64, hashes: &[u8], size: u64) -> u64 {
    let n = block_count(size);
    let Ok(mut file) = std::fs::File::open(for_io(part)) else {
        return 0;
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut start = verified.min(n);
    // The checkpoint claims more than is on disk: start over.
    if start > 0 && (start - 1) * BLOCK_SIZE + block_len(size, start - 1) > len {
        start = 0;
    }
    let mut buf = vec![0u8; BLOCK_SIZE as usize];
    while start < n {
        let blen = block_len(size, start) as usize;
        if start * BLOCK_SIZE + blen as u64 > len {
            break;
        }
        if file.seek(SeekFrom::Start(start * BLOCK_SIZE)).is_err() {
            break;
        }
        if manifest::read_full(&mut file, &mut buf[..blen]).ok() != Some(blen) {
            break;
        }
        let i = start as usize * HASH_LEN;
        if hash_block(&buf[..blen])[..] != hashes[i..i + HASH_LEN] {
            break;
        }
        start += 1;
    }
    start
}

#[allow(clippy::too_many_arguments)]
async fn fetch_data(
    inner: &Arc<Inner>,
    conn: &Connection,
    id: &str,
    f: &FileRow,
    start: u64,
    hashes: Vec<u8>,
    part: &Path,
    others: u64,
    token: &CancellationToken,
    name: &str,
) -> Result<(), DlError> {
    let req = PeerRequest::Get {
        transfer_id: id.to_owned(),
        file: f.idx,
        start_block: start,
    };
    let (resp, mut recv) = tokio::select! {
        _ = token.cancelled() => return Err(DlError::Stopped),
        r = inner.open_request(conn, &req) => r.map_err(|e| map_net(e, name))?,
    };
    let PeerResponse::Data { len } = resp else {
        return Err(DlError::Fatal("unexpected answer from the sender".into()));
    };
    if len != f.size - (start * BLOCK_SIZE).min(f.size) {
        return Err(DlError::Fatal(
            "the sender offered the wrong amount of data".into(),
        ));
    }

    let (tx, rx) = mpsc::channel::<(u64, Vec<u8>)>(4);
    let writer = {
        let inner = inner.clone();
        let ctx = WriteCtx {
            id: id.to_owned(),
            idx: f.idx,
            name: f.rel_path.clone(),
            part: part.to_path_buf(),
            size: f.size,
            start,
            others,
            hashes,
        };
        tokio::task::spawn_blocking(move || write_blocks(&inner, ctx, rx))
    };

    let mut read_result: Result<(), DlError> = Ok(());
    for b in start..block_count(f.size) {
        let mut buf = vec![0u8; block_len(f.size, b) as usize];
        let r = tokio::select! {
            _ = token.cancelled() => { read_result = Err(DlError::Stopped); break; }
            r = tokio::time::timeout(BLOCK_TIMEOUT, recv.read_exact(&mut buf)) => r,
        };
        if !matches!(r, Ok(Ok(()))) {
            read_result = Err(DlError::Retry(format!("Connection to {name} lost")));
            break;
        }
        if tx.send((b, buf)).await.is_err() {
            break; // The writer stopped; its error explains why.
        }
    }
    drop(tx);
    drop(recv);
    let write_result = writer
        .await
        .map_err(|e| DlError::Fatal(format!("writer crashed: {e}")))?;
    write_result?;
    read_result
}

struct WriteCtx {
    id: String,
    idx: u32,
    name: String,
    part: PathBuf,
    size: u64,
    start: u64,
    others: u64,
    hashes: Vec<u8>,
}

/// Verifies each block against the list and writes it at its offset,
/// checkpointing (fsync + SQLite) about once a second.
fn write_blocks(
    inner: &Inner,
    ctx: WriteCtx,
    mut rx: mpsc::Receiver<(u64, Vec<u8>)>,
) -> Result<(), DlError> {
    let io_fatal =
        |e: std::io::Error| DlError::Fatal(format!("could not write {}: {e}", ctx.part.display()));
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(for_io(&ctx.part))
        .map_err(io_fatal)?;
    let mut verified = ctx.start;
    let mut last_sync = Instant::now();
    let checkpoint = |file: &mut std::fs::File, verified: u64| -> Result<(), DlError> {
        file.sync_data().map_err(io_fatal)?;
        inner
            .db
            .set_verified_blocks(&ctx.id, ctx.idx, verified)
            .map_err(|e| DlError::Fatal(e.to_string()))
    };
    while let Some((b, buf)) = rx.blocking_recv() {
        if b != verified {
            return Err(DlError::Fatal("blocks arrived out of order".into()));
        }
        let i = b as usize * HASH_LEN;
        if hash_block(&buf)[..] != ctx.hashes[i..i + HASH_LEN] {
            checkpoint(&mut file, verified)?;
            return Err(DlError::Integrity(format!(
                "Block {b} of {} failed BLAKE3 verification",
                ctx.name
            )));
        }
        file.seek(SeekFrom::Start(b * BLOCK_SIZE))
            .map_err(io_fatal)?;
        file.write_all(&buf).map_err(io_fatal)?;
        verified += 1;
        inner.progress(&ctx.id, ctx.others + (verified * BLOCK_SIZE).min(ctx.size));
        if last_sync.elapsed() >= Duration::from_secs(1) {
            checkpoint(&mut file, verified)?;
            last_sync = Instant::now();
        }
    }
    checkpoint(&mut file, verified)
}

#[allow(clippy::too_many_arguments)]
async fn verify_and_place(
    inner: &Arc<Inner>,
    id: &str,
    f: &FileRow,
    part: &Path,
    dest: &Path,
    pos: usize,
    count: usize,
    token: &CancellationToken,
) -> Result<(), DlError> {
    inner.set_status(id, TransferStatus::Verifying);
    let short = f
        .rel_path
        .rsplit('/')
        .next()
        .unwrap_or(&f.rel_path)
        .to_owned();
    let label = if count > 1 {
        format!("Verifying {short} ({pos}/{count})")
    } else {
        format!("Verifying {short}")
    };
    inner.set_note(id, Some(label.clone()));

    let flag = Arc::new(AtomicBool::new(false));
    let watcher = {
        let flag = flag.clone();
        let token = token.clone();
        tokio::spawn(async move {
            token.cancelled().await;
            flag.store(true, Ordering::Relaxed);
        })
    };
    let result = {
        let (inner, id, part, size) = (inner.clone(), id.to_owned(), part.to_path_buf(), f.size);
        let mut last = Instant::now();
        tokio::task::spawn_blocking(move || {
            manifest::hash_file(&part, size, false, &flag, |n| {
                if last.elapsed() >= Duration::from_millis(500) && size > 0 {
                    last = Instant::now();
                    inner.set_note(&id, Some(format!("{label} {}%", n * 100 / size)));
                }
            })
        })
        .await
    };
    watcher.abort();
    let hashes = match result {
        Ok(Ok(h)) => h,
        Ok(Err(manifest::HashError::Cancelled)) => return Err(DlError::Stopped),
        Ok(Err(manifest::HashError::Changed(_))) => {
            let _ = inner.db.reset_file(id, f.idx);
            let _ = std::fs::remove_file(for_io(part));
            return Err(DlError::Integrity(format!(
                "{} has the wrong size on disk",
                f.rel_path
            )));
        }
        Ok(Err(e)) => return Err(DlError::Fatal(e.to_string())),
        Err(e) => return Err(DlError::Fatal(e.to_string())),
    };
    if Some(hashes.root.to_hex().as_str()) != f.root_hash.as_deref() {
        let _ = inner.db.reset_file(id, f.idx);
        let _ = std::fs::remove_file(for_io(part));
        return Err(DlError::Integrity(format!(
            "{} failed the final BLAKE3 check",
            f.rel_path
        )));
    }

    let placed = {
        let (inner, id, f, part, dest) = (
            inner.clone(),
            id.to_owned(),
            f.clone(),
            part.to_path_buf(),
            dest.to_path_buf(),
        );
        tokio::task::spawn_blocking(move || place_file(&inner.db, &dest, &id, &f, &part))
            .await
            .map_err(|e| DlError::Fatal(e.to_string()))?
    };
    let final_path = placed
        .map_err(|e| DlError::Fatal(format!("could not move {} into place: {e}", f.rel_path)))?;
    inner
        .db
        .set_file_done(id, f.idx, &final_path.display().to_string())
        .map_err(|e| DlError::Fatal(e.to_string()))?;
    inner.set_note(id, None);
    if pos < count {
        inner.set_status(id, TransferStatus::Transferring);
    }
    Ok(())
}

/// Folder that holds a top-level entry of the transfer: `dest/<name>` or,
/// if that name is taken, `dest/<name> (N)`. Chosen once and remembered.
fn root_dir(db: &Db, dest: &Path, id: &str, wire_root: &str) -> std::io::Result<PathBuf> {
    if let Ok(Some(name)) = db.root_name(id, wire_root) {
        let dir = dest.join(name);
        std::fs::create_dir_all(for_io(&dir))?;
        return Ok(dir);
    }
    let name = sanitize_component(wire_root, Platform::current());
    let dir = create_unique_dir(dest, &name)?;
    let final_name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or(name);
    db.set_root_name(id, wire_root, &final_name)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(dir)
}

/// Splits a wire path into its root and the rest.
fn split_root(rel: &str) -> (&str, Option<&str>) {
    let rel = rel.trim_start_matches('/');
    match rel.split_once('/') {
        Some((root, rest)) if !rest.trim_matches('/').is_empty() => (root, Some(rest)),
        Some((root, _)) => (root, None),
        None => (rel, None),
    }
}

fn place_file(
    db: &Db,
    dest: &Path,
    id: &str,
    f: &FileRow,
    part: &Path,
) -> std::io::Result<PathBuf> {
    set_mtime(part, system_time_from_ns(f.mtime_ns))?;
    let (root, rest) = split_root(&f.rel_path);
    match rest {
        None => {
            let name = sanitize_component(root, Platform::current());
            move_into_unique(part, dest, &name)
        }
        Some(rest) => {
            let dir = root_dir(db, dest, id, root)?;
            let rel = safe_relative_path(rest, Platform::current())
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let parent = match rel.parent() {
                Some(p) if !p.as_os_str().is_empty() => dir.join(p),
                _ => dir,
            };
            std::fs::create_dir_all(for_io(&parent))?;
            let name = rel
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".into());
            // `move_into_unique` also resolves names that collide after
            // sanitizing (`a:b` and `a_b` on Windows).
            move_into_unique(part, &parent, &name)
        }
    }
}

/// Recreates folders (including empty ones), restores their times,
/// removes the in-progress data and reports completion.
async fn finish(
    inner: &Arc<Inner>,
    id: &str,
    row: &TransferRow,
    dest: &Path,
) -> Result<(), DlError> {
    let dirs = inner
        .db
        .dirs(id)
        .map_err(|e| DlError::Fatal(e.to_string()))?;
    let result = {
        let (inner, id, dest) = (inner.clone(), id.to_owned(), dest.to_path_buf());
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            let mut made: Vec<(PathBuf, i64, usize)> = Vec::new();
            for d in dirs {
                let (root, rest) = split_root(&d.rel_path);
                let base = root_dir(&inner.db, &dest, &id, root)?;
                let path = match rest {
                    Some(rest) => base.join(
                        safe_relative_path(rest, Platform::current())
                            .map_err(|e| std::io::Error::other(e.to_string()))?,
                    ),
                    None => base,
                };
                std::fs::create_dir_all(for_io(&path))?;
                let depth = d.rel_path.matches('/').count();
                made.push((path, d.mtime_ns, depth));
            }
            // Deepest first: setting a child's time doesn't touch the parent.
            made.sort_by_key(|m| std::cmp::Reverse(m.2));
            for (path, mtime, _) in made {
                if let Err(e) = set_mtime(&path, system_time_from_ns(mtime)) {
                    tracing::debug!("could not set time of {}: {e}", path.display());
                }
            }
            Ok(())
        })
        .await
        .map_err(|e| DlError::Fatal(e.to_string()))?
    };
    result.map_err(|e| DlError::Fatal(format!("could not create folders: {e}")))?;

    cleanup_partial(row);
    let _ = inner.db.set_done_bytes(id, row.total_size);
    let _ = inner.db.set_error(id, None);
    {
        let mut live = inner.live();
        if let Some(l) = live.get_mut(id) {
            l.done_bytes = Some(row.total_size);
        }
    }
    inner.set_note(id, None);
    inner.set_status(id, TransferStatus::Completed);
    inner.notify_peer(
        row.peer.clone(),
        PeerRequest::Notice {
            transfer_id: id.to_owned(),
            notice: Notice::Completed,
        },
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_root_cases() {
        assert_eq!(split_root("a.mp4"), ("a.mp4", None));
        assert_eq!(split_root("talks/a.mp4"), ("talks", Some("a.mp4")));
        assert_eq!(split_root("talks/d/a.mp4"), ("talks", Some("d/a.mp4")));
        assert_eq!(split_root("talks"), ("talks", None));
    }

    #[test]
    fn resume_point_skips_verified_blocks_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("0.part");
        let size = BLOCK_SIZE * 2 + 10;
        let data: Vec<u8> = (0..size).map(|i| (i % 253) as u8).collect();
        let mut hashes = Vec::new();
        for b in 0..block_count(size) {
            let s = (b * BLOCK_SIZE) as usize;
            let e = (s + block_len(size, b) as usize).min(data.len());
            hashes.extend_from_slice(&hash_block(&data[s..e]));
        }
        // Two full blocks on disk, checkpoint says one.
        std::fs::write(&part, &data[..(BLOCK_SIZE * 2) as usize]).unwrap();
        assert_eq!(resume_point(&part, 1, &hashes, size), 2);
        // Corrupt the second block: resume from it.
        let mut bad = data[..(BLOCK_SIZE * 2) as usize].to_vec();
        bad[BLOCK_SIZE as usize + 5] ^= 0xff;
        std::fs::write(&part, &bad).unwrap();
        assert_eq!(resume_point(&part, 1, &hashes, size), 1);
        // Missing file: start over.
        std::fs::remove_file(&part).unwrap();
        assert_eq!(resume_point(&part, 2, &hashes, size), 0);
    }
}
