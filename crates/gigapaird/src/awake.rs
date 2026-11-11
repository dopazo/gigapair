//! Keeps the computer awake only while transfers are active (RF-07).
//!
//! The guard lives on its own thread because the platform handles are not
//! guaranteed to be `Send`.

use std::sync::mpsc;
use std::time::Duration;

use gigapair_core::engine::Engine;

pub fn spawn(engine: Engine) {
    let (tx, rx) = mpsc::channel::<bool>();
    let handle = tokio::runtime::Handle::current();
    let engine_for_thread = engine.clone();
    std::thread::Builder::new()
        .name("keep-awake".into())
        .spawn(move || guard_thread(rx, engine_for_thread, handle))
        .ok();

    tokio::spawn(async move {
        let mut active = engine.watch_active();
        // Re-check now and then so the "keep awake" setting applies
        // without waiting for a transfer to change state.
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        loop {
            let want = *active.borrow_and_update() && engine.config().keep_awake;
            if tx.send(want).is_err() {
                return;
            }
            tokio::select! {
                r = active.changed() => if r.is_err() { let _ = tx.send(false); return; },
                _ = tick.tick() => {}
            }
            if engine.is_shut_down() {
                let _ = tx.send(false);
                return;
            }
        }
    });
}

fn guard_thread(rx: mpsc::Receiver<bool>, engine: Engine, _rt: tokio::runtime::Handle) {
    let mut guard: Option<keepawake::KeepAwake> = None;
    let mut failed_once = false;
    while let Ok(mut want) = rx.recv() {
        // Coalesce bursts of updates.
        while let Ok(next) = rx.try_recv() {
            want = next;
        }
        match (want, guard.is_some()) {
            (true, false) => {
                match keepawake::Builder::default()
                    .idle(true)
                    .reason("Gigapair transfer in progress")
                    .app_name("Gigapair")
                    .app_reverse_domain("dev.gigapair.Gigapair")
                    .create()
                {
                    Ok(g) => {
                        tracing::info!("keeping the computer awake while transfers run");
                        guard = Some(g);
                        engine.set_keeping_awake(true);
                    }
                    Err(e) => {
                        if !failed_once {
                            tracing::warn!("could not keep the computer awake: {e}");
                            failed_once = true;
                        }
                    }
                }
            }
            (false, true) => {
                guard = None;
                tracing::info!("transfers finished; the computer may sleep again");
                engine.set_keeping_awake(false);
            }
            _ => {}
        }
    }
    drop(guard);
    engine.set_keeping_awake(false);
}
