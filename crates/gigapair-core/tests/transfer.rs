//! Two engines in one process, talking over loopback.
//!
//! Test data is generated on the fly. `GIGAPAIR_TEST_MB` sets the size of
//! the large file (default 48 MB) for manual multi-GB runs.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::time::Duration;

use gigapair_core::api::{Event, Request, Response};
use gigapair_core::config::{Config, KeyStorage};
use gigapair_core::db::Db;
use gigapair_core::dirs::AppDirs;
use gigapair_core::engine::{Engine, EngineOptions, NetworkMode};
use gigapair_core::keys;
use gigapair_core::types::{Direction, TransferInfo, TransferStatus};

const TIMEOUT: Duration = Duration::from_secs(120);

fn test_mb() -> u64 {
    std::env::var("GIGAPAIR_TEST_MB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(48)
}

async fn start(root: &Path, tweak: impl FnOnce(&mut Config)) -> Engine {
    let dirs = AppDirs::under(root);
    let mut config = Config {
        download_dir: Some(root.join("downloads")),
        keep_awake: false,
        notifications: false,
        ..Config::default()
    };
    tweak(&mut config);
    let (key, _) = keys::load_or_create(&dirs, KeyStorage::File).unwrap();
    Engine::start(EngineOptions {
        dirs,
        config,
        secret_key: key,
        network: NetworkMode::LocalOnly,
    })
    .await
    .unwrap()
}

/// Pairs `a` and `b`; each sees the other under `name_of_b` / `name_of_a`.
async fn pair(a: &Engine, b: &Engine, name_of_b: &str, name_of_a: &str) {
    let mut a_events = a.subscribe();
    let Response::PairCode { code, .. } = a.handle(Request::PairCreate).await else {
        panic!("no code")
    };
    let Response::PendingPair(on_b) = b.handle(Request::PairJoin { code: code.clone() }).await
    else {
        panic!("join failed")
    };
    let on_a = tokio::time::timeout(TIMEOUT, async {
        loop {
            if let Ok(Event::PairRequest(p)) = a_events.recv().await {
                return p;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        on_a.fingerprint, on_b.fingerprint,
        "fingerprints must match"
    );
    assert!(on_a.incoming && !on_b.incoming);

    assert!(matches!(
        a.handle(Request::PairConfirm {
            id: on_a.id,
            name: name_of_b.into()
        })
        .await,
        Response::Contact(_)
    ));
    assert!(matches!(
        b.handle(Request::PairConfirm {
            id: on_b.id,
            name: name_of_a.into()
        })
        .await,
        Response::Contact(_)
    ));

    // One-time code.
    let again = b.handle(Request::PairJoin { code }).await;
    assert!(matches!(again, Response::Error { .. }), "{again:?}");
}

async fn wait_for(
    engine: &Engine,
    id: &str,
    what: &str,
    pred: impl Fn(&TransferInfo) -> bool,
) -> TransferInfo {
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        if let Some(t) = engine.transfer(id) {
            if pred(&t) {
                return t;
            }
            if tokio::time::Instant::now() > deadline {
                panic!("timed out waiting for {what}; last state: {t:#?}");
            }
        } else if tokio::time::Instant::now() > deadline {
            panic!("timed out waiting for {what}; transfer unknown");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn status_is(s: TransferStatus) -> impl Fn(&TransferInfo) -> bool {
    move |t| t.status == s
}

/// Deterministic, incompressible data.
fn gen_data(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.truncate(len);
    out
}

fn write_file(path: &Path, len: usize, seed: u64) -> Vec<u8> {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let data = gen_data(len, seed);
    std::fs::write(path, &data).unwrap();
    data
}

fn mtime(path: &Path) -> std::time::SystemTime {
    std::fs::metadata(path).unwrap().modified().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn folder_transfer_with_manual_accept() {
    let tmp = tempfile::tempdir().unwrap();
    let a = start(&tmp.path().join("a"), |_| {}).await;
    let b = start(&tmp.path().join("b"), |_| {}).await;
    pair(&a, &b, "bob", "alice").await;

    // Source tree: nested files, an empty file, an empty folder, a file
    // spanning several blocks with a partial last block.
    let src = tmp.path().join("src/talks");
    let big = write_file(&src.join("day 1/keynote.mp4"), 9 * 1024 * 1024 + 3, 1);
    write_file(&src.join("notes.txt"), 1234, 2);
    write_file(&src.join("empty.bin"), 0, 3);
    std::fs::create_dir_all(src.join("day 2/empty folder")).unwrap();
    let old = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000);
    filetime::set_file_mtime(
        src.join("notes.txt"),
        filetime::FileTime::from_system_time(old),
    )
    .unwrap();
    filetime::set_file_mtime(
        src.join("day 2/empty folder"),
        filetime::FileTime::from_system_time(old),
    )
    .unwrap();
    let single = tmp.path().join("src/single.bin");
    write_file(&single, 100, 4);

    let Response::Sent { transfer, warnings } = a
        .handle(Request::Send {
            contact: "bob".into(),
            paths: vec![src.display().to_string(), single.display().to_string()],
        })
        .await
    else {
        panic!("send failed")
    };
    assert!(warnings.is_empty(), "{warnings:?}");
    let id = transfer.id.clone();
    assert_eq!(transfer.file_count, 4);
    assert_eq!(transfer.label, "talks/ + 1 more");

    // Receiver must accept.
    let incoming = wait_for(&b, &id, "offer", status_is(TransferStatus::PendingAccept)).await;
    assert_eq!(incoming.direction, Direction::Receive);
    assert_eq!(incoming.peer_name, "alice");
    let Response::Transfers(reqs) = b.handle(Request::Requests).await else {
        panic!()
    };
    assert_eq!(reqs.len(), 1);
    wait_for(&a, &id, "waiting", status_is(TransferStatus::WaitingAccept)).await;

    let dest = tmp.path().join("chosen");
    let r = b
        .handle(Request::Accept {
            id: id[..8].into(),
            dest_dir: Some(dest.display().to_string()),
        })
        .await;
    assert!(matches!(r, Response::Transfer(_)), "{r:?}");

    let done = wait_for(
        &b,
        &id,
        "receive done",
        status_is(TransferStatus::Completed),
    )
    .await;
    assert_eq!(done.done_bytes, done.total_bytes);
    wait_for(&a, &id, "send done", status_is(TransferStatus::Completed)).await;

    // Contents, structure and times.
    let got = dest.join("talks");
    assert_eq!(std::fs::read(got.join("day 1/keynote.mp4")).unwrap(), big);
    assert_eq!(
        std::fs::read(got.join("notes.txt")).unwrap(),
        std::fs::read(src.join("notes.txt")).unwrap()
    );
    assert_eq!(std::fs::read(got.join("empty.bin")).unwrap().len(), 0);
    assert!(got.join("day 2/empty folder").is_dir());
    assert_eq!(mtime(&got.join("notes.txt")), old);
    assert_eq!(mtime(&got.join("day 2/empty folder")), old);
    assert_eq!(
        std::fs::read(dest.join("single.bin")).unwrap(),
        std::fs::read(&single).unwrap()
    );
    // In-progress area is cleaned up.
    assert!(!dest.join(".gigapair-partial").exists());

    // History on both sides.
    for e in [&a, &b] {
        let Response::Transfers(h) = e.handle(Request::History { limit: 10 }).await else {
            panic!()
        };
        assert_eq!(h.len(), 1);
        assert!(h[0].conn.is_some(), "connection type is recorded");
    }

    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auto_accept_never_overwrites() {
    let tmp = tempfile::tempdir().unwrap();
    let a = start(&tmp.path().join("a"), |_| {}).await;
    let b = start(&tmp.path().join("b"), |_| {}).await;
    pair(&a, &b, "server", "workstation").await;
    assert!(matches!(
        b.handle(Request::ContactTrust {
            contact: "work".into(), // unique prefix
            auto_accept: true
        })
        .await,
        Response::Ok
    ));

    let downloads = tmp.path().join("b/downloads");
    std::fs::create_dir_all(&downloads).unwrap();
    std::fs::write(downloads.join("talk.mp4"), b"existing").unwrap();
    std::fs::create_dir_all(downloads.join("photos")).unwrap();

    let file = tmp.path().join("src/talk.mp4");
    let data = write_file(&file, 5 * 1024 * 1024, 7);
    let folder = tmp.path().join("src/photos");
    write_file(&folder.join("a.jpg"), 10, 8);

    let (t, _) = a
        .send_files("server", vec![file.clone(), folder.clone()])
        .await
        .unwrap();
    wait_for(&b, &t.id, "done", status_is(TransferStatus::Completed)).await;

    assert_eq!(
        std::fs::read(downloads.join("talk.mp4")).unwrap(),
        b"existing"
    );
    assert_eq!(std::fs::read(downloads.join("talk (1).mp4")).unwrap(), data);
    assert!(downloads.join("photos (1)/a.jpg").is_file());
    assert_eq!(
        std::fs::read_dir(downloads.join("photos")).unwrap().count(),
        0
    );

    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn receiver_restart_resumes_without_resending() {
    let tmp = tempfile::tempdir().unwrap();
    let mb = test_mb();
    // Throttle the sender so there is time to kill the receiver mid-way.
    let a = start(&tmp.path().join("a"), |c| {
        c.upload_limit_kib = Some(mb * 1024 / 6)
    })
    .await;
    let b_root = tmp.path().join("b");
    let b = start(&b_root, |_| {}).await;
    pair(&a, &b, "bob", "alice").await;
    b.handle(Request::ContactTrust {
        contact: "alice".into(),
        auto_accept: true,
    })
    .await;

    let file = tmp.path().join("src/recording.mov");
    let data = write_file(&file, (mb * 1024 * 1024) as usize, 42);
    let (t, _) = a.send_files("bob", vec![file]).await.unwrap();
    let id = t.id.clone();

    // Kill the receiver at roughly a third.
    let third = data.len() as u64 / 3;
    wait_for(&b, &id, "progress", |t| t.done_bytes >= third).await;
    b.shutdown().await;
    drop(b);

    let verified = {
        let db = Db::open(&AppDirs::under(&b_root).db_file()).unwrap();
        db.files(&id).unwrap()[0].verified_blocks
    };
    assert!(verified > 0, "progress was checkpointed");
    let partial = b_root
        .join("downloads/.gigapair-partial")
        .join(&id)
        .join("0.part");
    assert!(partial.exists(), "partial data stays in .gigapair-partial");
    assert!(!b_root.join("downloads/recording.mov").exists());

    // Watch what the sender serves after the restart.
    let mut a_events = a.subscribe();
    let b = start(&b_root, |_| {}).await;
    let min_served = tokio::spawn(async move {
        let mut min = u64::MAX;
        while let Ok(ev) = a_events.recv().await {
            if let Event::Transfer(t) = ev {
                if t.status == TransferStatus::Transferring {
                    min = min.min(t.done_bytes);
                }
                if t.status == TransferStatus::Completed {
                    break;
                }
            }
        }
        min
    });

    let done = wait_for(
        &b,
        &id,
        "done after restart",
        status_is(TransferStatus::Completed),
    )
    .await;
    assert_eq!(done.done_bytes, data.len() as u64);
    let got = std::fs::read(b_root.join("downloads/recording.mov")).unwrap();
    assert!(got == data, "file must be identical");

    wait_for(&a, &id, "sender done", status_is(TransferStatus::Completed)).await;
    let min = tokio::time::timeout(TIMEOUT, min_served)
        .await
        .unwrap()
        .unwrap();
    assert!(
        min >= verified * gigapair_core::manifest::BLOCK_SIZE,
        "sender resent verified blocks: served from {min}, verified {verified} blocks"
    );

    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sender_restart_and_pause_resume() {
    let tmp = tempfile::tempdir().unwrap();
    let a_root = tmp.path().join("a");
    let limit = |c: &mut Config| c.upload_limit_kib = Some(4 * 1024);
    let a = start(&a_root, limit).await;
    let b = start(&tmp.path().join("b"), |_| {}).await;
    pair(&a, &b, "bob", "alice").await;
    b.handle(Request::ContactTrust {
        contact: "alice".into(),
        auto_accept: true,
    })
    .await;

    let file = tmp.path().join("src/big.bin");
    let data = write_file(&file, 24 * 1024 * 1024, 9);
    let (t, _) = a.send_files("bob", vec![file]).await.unwrap();
    let id = t.id.clone();

    // Receiver pauses, then resumes.
    wait_for(&b, &id, "started", |t| t.done_bytes > 2 * 1024 * 1024).await;
    let r = b.handle(Request::Pause { id: id.clone() }).await;
    assert!(
        matches!(r, Response::Transfer(ref t) if t.status == TransferStatus::Paused),
        "{r:?}"
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    let paused_at = b.transfer(&id).unwrap().done_bytes;
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(
        b.transfer(&id).unwrap().done_bytes,
        paused_at,
        "no progress while paused"
    );
    b.handle(Request::Resume { id: id.clone() }).await;

    // Sender restarts mid-way.
    wait_for(&b, &id, "more progress", |t| {
        t.done_bytes > paused_at + 2 * 1024 * 1024
    })
    .await;
    a.shutdown().await;
    drop(a);
    let a = start(&a_root, limit).await;

    wait_for(&b, &id, "done", status_is(TransferStatus::Completed)).await;
    assert_eq!(
        std::fs::read(tmp.path().join("b/downloads/big.bin")).unwrap(),
        data
    );
    wait_for(&a, &id, "sender done", status_is(TransferStatus::Completed)).await;
    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reject_cancel_and_unpair() {
    let tmp = tempfile::tempdir().unwrap();
    let a = start(&tmp.path().join("a"), |c| c.upload_limit_kib = Some(1024)).await;
    let b = start(&tmp.path().join("b"), |_| {}).await;
    pair(&a, &b, "bob", "alice").await;

    let file = tmp.path().join("src/x.bin");
    write_file(&file, 8 * 1024 * 1024, 5);

    // Reject.
    let (t1, _) = a.send_files("bob", vec![file.clone()]).await.unwrap();
    wait_for(
        &b,
        &t1.id,
        "offer",
        status_is(TransferStatus::PendingAccept),
    )
    .await;
    b.handle(Request::Reject { id: t1.id.clone() }).await;
    let r = wait_for(&a, &t1.id, "rejected", status_is(TransferStatus::Rejected)).await;
    assert!(r.error.unwrap().contains("Rejected"));

    // Cancel by the sender while transferring: receiver cleans up.
    let (t2, _) = a.send_files("bob", vec![file.clone()]).await.unwrap();
    wait_for(
        &b,
        &t2.id,
        "offer",
        status_is(TransferStatus::PendingAccept),
    )
    .await;
    b.handle(Request::Accept {
        id: t2.id.clone(),
        dest_dir: None,
    })
    .await;
    wait_for(&b, &t2.id, "moving", |t| t.done_bytes > 0).await;
    a.handle(Request::Cancel { id: t2.id.clone() }).await;
    wait_for(
        &b,
        &t2.id,
        "cancelled",
        status_is(TransferStatus::Cancelled),
    )
    .await;
    let partial = tmp.path().join("b/downloads/.gigapair-partial");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!partial.join(&t2.id).exists());

    // Retry from history creates a new transfer with the same files.
    let Response::Sent { transfer: t3, .. } = a.handle(Request::Retry { id: t2.id.clone() }).await
    else {
        panic!("retry failed")
    };
    assert_ne!(t3.id, t2.id);
    wait_for(
        &b,
        &t3.id,
        "offer",
        status_is(TransferStatus::PendingAccept),
    )
    .await;
    b.handle(Request::Reject { id: t3.id.clone() }).await;

    // Removing a contact revokes access both ways.
    assert!(matches!(
        b.handle(Request::ContactRemove {
            contact: "alice".into()
        })
        .await,
        Response::Ok
    ));
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    while !a.contacts().unwrap().is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "unpair not delivered"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let r = a
        .handle(Request::Send {
            contact: "bob".into(),
            paths: vec![file.display().to_string()],
        })
        .await;
    assert!(matches!(r, Response::Error { .. }));

    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unpaired_device_cannot_send() {
    let tmp = tempfile::tempdir().unwrap();
    let a = start(&tmp.path().join("a"), |_| {}).await;
    let b = start(&tmp.path().join("b"), |_| {}).await;
    let c = start(&tmp.path().join("c"), |_| {}).await;
    pair(&a, &b, "bob", "alice").await;

    // c knows nothing; a pairing code it didn't get can't be guessed, and a
    // forged contact entry on c's side doesn't make b accept anything.
    let db = Db::open(&AppDirs::under(&tmp.path().join("c")).db_file()).unwrap();
    db.upsert_contact(&b.node_id().to_string(), "bob", false)
        .unwrap();
    db.set_contact_addr(
        &b.node_id().to_string(),
        &serde_json::to_string(&b.addr()).unwrap(),
    )
    .unwrap();
    drop(db);

    let file = tmp.path().join("src/x.bin");
    write_file(&file, 1000, 1);
    let (t, _) = c.send_files("bob", vec![file]).await.unwrap();
    let failed = wait_for(&c, &t.id, "refused", status_is(TransferStatus::Failed)).await;
    assert!(failed.error.unwrap().contains("pair again"));
    assert!(b.transfer(&t.id).is_none());

    for e in [a, b, c] {
        e.shutdown().await;
    }
}

/// Manual throughput check: `GIGAPAIR_TEST_MB=4096 cargo test --release
/// -p gigapair-core --test transfer throughput -- --ignored --nocapture`.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "large; run manually"]
async fn throughput() {
    let tmp = tempfile::tempdir().unwrap();
    let a = start(&tmp.path().join("a"), |_| {}).await;
    let b = start(&tmp.path().join("b"), |_| {}).await;
    pair(&a, &b, "bob", "alice").await;
    b.handle(Request::ContactTrust {
        contact: "alice".into(),
        auto_accept: true,
    })
    .await;

    let mb = std::env::var("GIGAPAIR_TEST_MB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1024u64);
    let file = tmp.path().join("src/huge.bin");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    {
        use std::io::Write;
        let mut f = std::io::BufWriter::new(std::fs::File::create(&file).unwrap());
        let chunk = gen_data(8 * 1024 * 1024, 3);
        for _ in 0..(mb / 8) {
            f.write_all(&chunk).unwrap();
        }
    }
    let size = std::fs::metadata(&file).unwrap().len();
    let expected = blake3::hash(&std::fs::read(&file).unwrap());

    let t0 = std::time::Instant::now();
    let (t, _) = a.send_files("bob", vec![file]).await.unwrap();
    wait_for(&a, &t.id, "prepared", |t| {
        t.status != TransferStatus::Preparing
    })
    .await;
    let hashed = t0.elapsed();
    let t1 = std::time::Instant::now();
    wait_for(&b, &t.id, "verifying", |t| {
        matches!(
            t.status,
            TransferStatus::Verifying | TransferStatus::Completed
        )
    })
    .await;
    let moved = t1.elapsed();
    wait_for(&b, &t.id, "done", status_is(TransferStatus::Completed)).await;
    let total = t0.elapsed();
    let rate = |d: Duration| size as f64 / d.as_secs_f64() / 1e6;
    eprintln!(
        "{size} bytes: hash {:.0} MB/s, transfer {:.0} MB/s, total {:.1}s",
        rate(hashed),
        rate(moved),
        total.as_secs_f64()
    );
    let got = blake3::hash(&std::fs::read(tmp.path().join("b/downloads/huge.bin")).unwrap());
    assert_eq!(got, expected);
    a.shutdown().await;
    b.shutdown().await;
}
