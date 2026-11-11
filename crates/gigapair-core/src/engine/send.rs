//! Sender side: hashing, offering and serving file data.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use iroh::EndpointId;
use iroh::endpoint::{Connection, SendStream};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::sync::CancellationToken;

use super::net::{NetError, conn_info};
use super::{Backoff, EngineError, EngineResult, Inner, new_id, wait};
use crate::db::{DirRow, FileRow, TransferRow};
use crate::manifest::{self, BLOCK_SIZE, HashError, block_count};
use crate::types::{ConnKind, Direction, TransferInfo, TransferStatus, now_secs};
use crate::wire::{
    ErrorCode, Notice, Offer, OfferDir, OfferFile, OfferState, PeerRequest, PeerResponse, write_msg,
};

const CHUNK: usize = 1024 * 1024;

/// Scans `paths`, records a new outgoing transfer and starts hashing.
pub(super) async fn create(
    inner: &Arc<Inner>,
    peer: &crate::types::Contact,
    paths: Vec<PathBuf>,
) -> EngineResult<(TransferInfo, Vec<String>)> {
    for p in &paths {
        if !p.is_absolute() {
            return Err(EngineError::user(format!(
                "{} is not an absolute path",
                p.display()
            )));
        }
    }
    let sources: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
    let scan = tokio::task::spawn_blocking(move || manifest::scan(&paths))
        .await
        .map_err(|e| EngineError::user(e.to_string()))?
        .map_err(|e| EngineError::user(e.to_string()))?;

    let id = new_id();
    let row = TransferRow {
        id: id.clone(),
        direction: Direction::Send,
        peer: peer.node_id.clone(),
        peer_name: peer.name.clone(),
        status: TransferStatus::Preparing,
        label: scan.label(),
        total_size: scan.total_size,
        file_count: scan.files.len() as u64,
        done_bytes: 0,
        dest_dir: None,
        conn: None,
        error: None,
        created_at: now_secs(),
        finished_at: None,
        sources,
    };
    let files: Vec<FileRow> = scan
        .files
        .iter()
        .enumerate()
        .map(|(i, f)| FileRow {
            idx: i as u32,
            rel_path: f.rel_path.clone(),
            size: f.size,
            mtime_ns: f.mtime_ns,
            root_hash: None,
            blocks_hash: None,
            block_hashes: None,
            src_path: Some(f.src.display().to_string()),
            verified_blocks: 0,
            done: false,
            final_path: None,
        })
        .collect();
    let dirs: Vec<DirRow> = scan
        .dirs
        .iter()
        .map(|d| DirRow {
            rel_path: d.rel_path.clone(),
            mtime_ns: d.mtime_ns,
        })
        .collect();
    inner.db.insert_transfer(&row, &files, &dirs)?;
    inner.update_active();
    let info = inner.info(&row);
    inner.emit(crate::api::Event::Transfer(info.clone()));
    start_prepare(inner, &id);
    Ok((info, scan.warnings))
}

pub(super) fn start_prepare(inner: &Arc<Inner>, id: &str) {
    let id = id.to_owned();
    inner.spawn_task(&id.clone(), move |inner, token| async move {
        if prepare(&inner, &id, &token).await {
            offer_loop(&inner, &id, &token).await;
        }
    });
}

pub(super) fn start_offer(inner: &Arc<Inner>, id: &str) {
    let id = id.to_owned();
    inner.spawn_task(&id.clone(), move |inner, token| async move {
        offer_loop(&inner, &id, &token).await;
    });
}

/// Hashes every file that has no hashes yet. Returns true when done.
async fn prepare(inner: &Arc<Inner>, id: &str, token: &CancellationToken) -> bool {
    let files = match inner.db.files(id) {
        Ok(f) => f,
        Err(e) => {
            inner.fail(id, &e.to_string());
            return false;
        }
    };
    let mut done: u64 = files
        .iter()
        .filter(|f| f.root_hash.is_some())
        .map(|f| f.size)
        .sum();
    inner.set_note(id, Some("Computing BLAKE3 hashes".into()));
    for f in files.into_iter().filter(|f| f.root_hash.is_none()) {
        let Some(src) = f.src_path.clone() else {
            inner.fail(id, "internal error: source path missing");
            return false;
        };
        let flag = Arc::new(AtomicBool::new(false));
        let watcher = {
            let flag = flag.clone();
            let token = token.clone();
            tokio::spawn(async move {
                token.cancelled().await;
                flag.store(true, Ordering::Relaxed);
            })
        };
        let inner2 = inner.clone();
        let id2 = id.to_owned();
        let base = done;
        let size = f.size;
        let res = tokio::task::spawn_blocking(move || {
            manifest::hash_file(PathBuf::from(&src).as_path(), size, true, &flag, |n| {
                inner2.progress(&id2, base + n)
            })
        })
        .await;
        watcher.abort();
        match res {
            Ok(Ok(h)) => {
                let root = h.root.to_hex().to_string();
                let blocks_hash = h.blocks_hash().to_hex().to_string();
                if let Err(e) = inner
                    .db
                    .set_file_hashes(id, f.idx, &root, &blocks_hash, &h.blocks)
                {
                    inner.fail(id, &e.to_string());
                    return false;
                }
                done += f.size;
            }
            Ok(Err(HashError::Cancelled)) => return false,
            Ok(Err(e)) => {
                inner.fail(id, &e.to_string());
                return false;
            }
            Err(e) => {
                inner.fail(id, &format!("hashing failed: {e}"));
                return false;
            }
        }
    }
    {
        let mut live = inner.live();
        let l = live.entry(id.to_owned()).or_default();
        l.done_bytes = Some(0);
        l.speed.reset();
    }
    let _ = inner.db.set_done_bytes(id, 0);
    inner.set_note(id, None);
    inner.set_status(id, TransferStatus::WaitingAccept);
    true
}

fn build_offer(inner: &Inner, row: &TransferRow) -> Option<Offer> {
    let files = inner.db.files(&row.id).ok()?;
    let dirs = inner.db.dirs(&row.id).ok()?;
    let mut out = Vec::with_capacity(files.len());
    for f in files {
        out.push(OfferFile {
            path: f.rel_path,
            size: f.size,
            mtime_ns: f.mtime_ns,
            root_hash: f.root_hash?,
            blocks_hash: f.blocks_hash?,
        });
    }
    Some(Offer {
        transfer_id: row.id.clone(),
        sender_name: inner.device_name(),
        label: row.label.clone(),
        total_size: row.total_size,
        files: out,
        dirs: dirs
            .into_iter()
            .map(|d| OfferDir {
                path: d.rel_path,
                mtime_ns: d.mtime_ns,
            })
            .collect(),
    })
}

/// Delivers the offer, retrying until the receiver answers.
async fn offer_loop(inner: &Arc<Inner>, id: &str, token: &CancellationToken) {
    let wake = inner.wake_handle(id);
    let mut backoff = Backoff::new();
    loop {
        if token.is_cancelled() {
            return;
        }
        let Ok(Some(row)) = inner.db.transfer(id) else {
            return;
        };
        if row.status.is_terminal() || row.status == TransferStatus::Paused {
            return;
        }
        let Some(offer) = build_offer(inner, &row) else {
            // Hashes missing (e.g. crash while preparing): hash again.
            inner.set_status(id, TransferStatus::Preparing);
            if !prepare(inner, id, token).await {
                return;
            }
            continue;
        };
        let Ok(peer) = row.peer.parse::<EndpointId>() else {
            inner.fail(id, "invalid contact id");
            return;
        };
        let req = PeerRequest::Offer(offer);
        let result = tokio::select! {
            _ = token.cancelled() => return,
            r = inner.request(peer, &req) => r,
        };
        let name = &row.peer_name;
        match result {
            Ok(PeerResponse::Offer(state)) => {
                backoff.reset();
                match state {
                    OfferState::Pending => {
                        if row.status != TransferStatus::WaitingAccept {
                            inner.set_status(id, TransferStatus::WaitingAccept);
                        }
                        inner.set_note(id, Some(format!("Waiting for {name} to accept")));
                        // Check again now and then in case an answer was lost.
                        if !wait(token, &wake, Duration::from_secs(120)).await {
                            return;
                        }
                    }
                    OfferState::Accepted => {
                        let current = inner.db.transfer(id).ok().flatten().map(|r| r.status);
                        if current == Some(TransferStatus::WaitingAccept) {
                            inner.set_status(id, TransferStatus::Queued);
                        }
                        inner.set_note(id, Some(format!("Accepted; waiting for {name}")));
                        return;
                    }
                    OfferState::Rejected { reason } => {
                        let _ = inner.db.set_error(id, Some(&reason));
                        inner.set_status(id, TransferStatus::Rejected);
                        return;
                    }
                    OfferState::Cancelled => {
                        let _ = inner
                            .db
                            .set_error(id, Some(&format!("Cancelled by {name}")));
                        inner.set_status(id, TransferStatus::Cancelled);
                        return;
                    }
                    OfferState::Completed => {
                        inner.set_status(id, TransferStatus::Completed);
                        return;
                    }
                }
            }
            Ok(_) => {
                if !wait(token, &wake, backoff.next_delay()).await {
                    return;
                }
            }
            Err(NetError::Remote { code, message }) => match code {
                ErrorCode::NotPaired => {
                    inner.fail(
                        id,
                        &format!("{name} no longer has this device as a contact; pair again"),
                    );
                    return;
                }
                ErrorCode::IncompatibleVersion | ErrorCode::BadRequest => {
                    inner.fail(id, &message);
                    return;
                }
                _ => {
                    inner.set_note(id, Some(message));
                    if !wait(token, &wake, backoff.next_delay()).await {
                        return;
                    }
                }
            },
            Err(e) => {
                tracing::debug!(transfer = id, "offer not delivered: {e}");
                inner.set_note(id, Some(format!("Waiting for {name} to come online")));
                if !wait(token, &wake, backoff.next_delay()).await {
                    return;
                }
            }
        }
    }
}

/// True if `id` is an outgoing transfer to `peer`.
pub(super) fn owns(inner: &Inner, peer: &str, id: &str) -> bool {
    matches!(inner.db.transfer(id), Ok(Some(r)) if r.direction == Direction::Send && r.peer == peer)
}

fn own_row(inner: &Inner, peer: &str, id: &str) -> Option<TransferRow> {
    inner
        .db
        .transfer(id)
        .ok()
        .flatten()
        .filter(|r| r.direction == Direction::Send && r.peer == peer)
}

pub(super) fn handle_decision(
    inner: &Arc<Inner>,
    peer: &str,
    id: &str,
    accepted: bool,
    reason: Option<String>,
) -> PeerResponse {
    let Some(row) = own_row(inner, peer, id) else {
        return PeerResponse::error(ErrorCode::UnknownTransfer, "unknown transfer");
    };
    if row.status.is_terminal() {
        return PeerResponse::Ok;
    }
    if accepted {
        if row.status == TransferStatus::WaitingAccept {
            inner.set_status(id, TransferStatus::Queued);
        }
        inner.set_note(id, Some(format!("Accepted; waiting for {}", row.peer_name)));
    } else {
        inner.stop_task(id);
        let reason = reason.unwrap_or_else(|| format!("Rejected by {}", row.peer_name));
        let _ = inner.db.set_error(id, Some(&reason));
        inner.set_note(id, None);
        inner.set_status(id, TransferStatus::Rejected);
    }
    PeerResponse::Ok
}

pub(super) fn handle_notice(inner: &Arc<Inner>, id: &str, notice: Notice) -> PeerResponse {
    let Ok(Some(row)) = inner.db.transfer(id) else {
        return PeerResponse::error(ErrorCode::UnknownTransfer, "unknown transfer");
    };
    let name = &row.peer_name;
    match notice {
        Notice::Paused => inner.set_note(id, Some(format!("Paused by {name}"))),
        Notice::Resumed => inner.set_note(id, None),
        Notice::Cancelled => {
            if !row.status.is_terminal() {
                inner.stop_task(id);
                let _ = inner
                    .db
                    .set_error(id, Some(&format!("Cancelled by {name}")));
                inner.set_note(id, None);
                inner.set_status(id, TransferStatus::Cancelled);
            }
        }
        Notice::Completed => {
            if row.status != TransferStatus::Completed {
                inner.stop_task(id);
                let _ = inner.db.set_error(id, None);
                let _ = inner.db.set_done_bytes(id, row.total_size);
                inner.set_note(id, None);
                inner.set_status(id, TransferStatus::Completed);
            }
        }
        Notice::Failed { reason } => {
            if !row.status.is_terminal() {
                inner.stop_task(id);
                inner.fail(id, &format!("Failed on {name}: {reason}"));
            }
        }
    }
    PeerResponse::Ok
}

async fn reply_err(send: &mut SendStream, code: ErrorCode, msg: &str) -> Result<(), NetError> {
    write_msg(send, &PeerResponse::error(code, msg)).await?;
    let _ = send.finish();
    Ok(())
}

pub(super) async fn serve_hashes(
    inner: &Arc<Inner>,
    peer: &str,
    id: &str,
    file: u32,
    send: &mut SendStream,
) -> Result<(), NetError> {
    let Some(row) = own_row(inner, peer, id) else {
        return reply_err(send, ErrorCode::UnknownTransfer, "unknown transfer").await;
    };
    if row.status == TransferStatus::Cancelled {
        return reply_err(
            send,
            ErrorCode::Cancelled,
            "the sender cancelled this transfer",
        )
        .await;
    }
    let files = inner.db.files(id).unwrap_or_default();
    let Some(blocks) = files
        .into_iter()
        .find(|f| f.idx == file)
        .and_then(|f| f.block_hashes)
    else {
        return reply_err(send, ErrorCode::BadRequest, "no such file").await;
    };
    write_msg(
        send,
        &PeerResponse::Hashes {
            count: (blocks.len() / manifest::HASH_LEN) as u64,
        },
    )
    .await?;
    tokio::io::AsyncWriteExt::write_all(send, &blocks).await?;
    let _ = send.finish();
    Ok(())
}

pub(super) async fn serve_get(
    inner: &Arc<Inner>,
    conn: &Connection,
    peer: &str,
    id: &str,
    file: u32,
    start_block: u64,
    mut send: SendStream,
) -> Result<(), NetError> {
    let Some(row) = own_row(inner, peer, id) else {
        return reply_err(&mut send, ErrorCode::UnknownTransfer, "unknown transfer").await;
    };
    match row.status {
        TransferStatus::Cancelled => {
            return reply_err(
                &mut send,
                ErrorCode::Cancelled,
                "the sender cancelled this transfer",
            )
            .await;
        }
        TransferStatus::Paused => {
            return reply_err(
                &mut send,
                ErrorCode::Paused,
                "the sender paused this transfer",
            )
            .await;
        }
        TransferStatus::Preparing => {
            return reply_err(
                &mut send,
                ErrorCode::Paused,
                "the sender is still preparing",
            )
            .await;
        }
        _ => {}
    }
    let info = conn_info(conn);
    if inner.config().direct_only && info.as_ref().is_some_and(|c| c.kind == ConnKind::Relay) {
        return reply_err(
            &mut send,
            ErrorCode::DirectOnly,
            "the sending device only allows direct connections and none could be made",
        )
        .await;
    }

    let files = inner.db.files(id).unwrap_or_default();
    let Some(f) = files.iter().find(|f| f.idx == file).cloned() else {
        return reply_err(&mut send, ErrorCode::BadRequest, "no such file").await;
    };
    if start_block > block_count(f.size) {
        return reply_err(&mut send, ErrorCode::BadRequest, "block out of range").await;
    }
    let src = PathBuf::from(f.src_path.clone().unwrap_or_default());
    let unchanged = std::fs::metadata(crate::paths::for_io(&src))
        .map(|m| m.len() == f.size && manifest::mtime_ns(&m) == f.mtime_ns)
        .unwrap_or(false);
    if !unchanged {
        let msg = format!(
            "{} changed or was removed after it was offered; send it again",
            src.display()
        );
        inner.fail(id, &msg);
        return reply_err(&mut send, ErrorCode::SourceChanged, &msg).await;
    }

    let offset = start_block * BLOCK_SIZE;
    let len = f.size.saturating_sub(offset);
    let mut file_handle = tokio::fs::File::open(crate::paths::for_io(&src)).await?;
    file_handle.seek(std::io::SeekFrom::Start(offset)).await?;
    write_msg(&mut send, &PeerResponse::Data { len }).await?;

    let serving = inner.shutdown.child_token();
    let serving_gen = inner
        .task_gen
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    {
        let mut live = inner.live();
        let l = live.entry(id.to_owned()).or_default();
        if let Some((_, old)) = l.serving.replace((serving_gen, serving.clone())) {
            old.cancel();
        }
    }
    if row.status != TransferStatus::Transferring {
        inner.set_status(id, TransferStatus::Transferring);
    }
    inner.set_conn(id, info);
    inner.set_note(id, None);

    let base: u64 = files.iter().filter(|g| g.idx < file).map(|g| g.size).sum();
    let result = stream_file(
        inner,
        id,
        &mut file_handle,
        &mut send,
        len,
        base + offset,
        &serving,
    )
    .await;
    {
        let mut live = inner.live();
        if let Some(l) = live.get_mut(id)
            && l.serving.as_ref().is_some_and(|(g, _)| *g == serving_gen)
        {
            l.serving = None;
        }
    }
    let finished = result.is_ok();
    if finished {
        let _ = send.finish();
        // Wait until the receiver has the data before going idle.
        let _ = tokio::time::timeout(Duration::from_secs(30), send.stopped()).await;
    }
    inner.flush_progress(id);
    if let Ok(Some(r)) = inner.db.transfer(id)
        && r.status == TransferStatus::Transferring
    {
        inner.set_status(id, TransferStatus::Queued);
        let last = file as usize + 1 == files.len();
        let note = if finished && last {
            format!("Waiting for {} to verify", r.peer_name)
        } else {
            format!("Waiting for {}", r.peer_name)
        };
        inner.set_note(id, Some(note));
    }
    result
}

async fn stream_file(
    inner: &Arc<Inner>,
    id: &str,
    file: &mut tokio::fs::File,
    send: &mut SendStream,
    len: u64,
    start_done: u64,
    token: &CancellationToken,
) -> Result<(), NetError> {
    let mut buf = vec![0u8; CHUNK];
    let mut sent = 0u64;
    let started = Instant::now();
    while sent < len {
        let want = ((len - sent) as usize).min(CHUNK);
        let n = tokio::select! {
            _ = token.cancelled() => return Err(NetError::Connect("stopped".into())),
            r = file.read(&mut buf[..want]) => r?,
        };
        if n == 0 {
            return Err(NetError::Connect("source file ended early".into()));
        }
        tokio::select! {
            _ = token.cancelled() => return Err(NetError::Connect("stopped".into())),
            r = tokio::io::AsyncWriteExt::write_all(send, &buf[..n]) => r?,
        }
        sent += n as u64;
        inner.progress(id, start_done + sent);

        let limit = inner.config().upload_limit_kib;
        if let Some(kib) = limit.filter(|k| *k > 0) {
            let expected = Duration::from_secs_f64(sent as f64 / (kib as f64 * 1024.0));
            let elapsed = started.elapsed();
            if expected > elapsed {
                tokio::select! {
                    _ = token.cancelled() => return Err(NetError::Connect("stopped".into())),
                    _ = tokio::time::sleep(expected - elapsed) => {}
                }
            }
        }
    }
    Ok(())
}
