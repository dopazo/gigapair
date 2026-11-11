//! Desktop notifications (RF-13): incoming requests, finished and failed
//! transfers. Best effort: headless machines simply have none.

use std::collections::HashMap;

use gigapair_core::api::{Event, RELAY_NOTICE};
use gigapair_core::engine::Engine;
use gigapair_core::types::{Direction, TransferStatus, format_bytes};
use tokio::sync::broadcast::error::RecvError;

pub fn spawn(engine: Engine) {
    tokio::spawn(async move {
        let mut events = engine.subscribe();
        let mut last: HashMap<String, TransferStatus> = HashMap::new();
        loop {
            let ev = match events.recv().await {
                Ok(ev) => ev,
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return,
            };
            if !engine.config().notifications {
                continue;
            }
            match ev {
                Event::IncomingRequest(t) => show(
                    "Incoming transfer",
                    &format!(
                        "{} wants to send you {} ({}). Open Gigapair to accept.",
                        t.peer_name,
                        t.label,
                        format_bytes(t.total_bytes)
                    ),
                ),
                Event::PairRequest(p) => show(
                    "Pairing request",
                    &format!(
                        "{} used your pairing code. Check that both devices show: {}",
                        p.remote_name, p.fingerprint
                    ),
                ),
                Event::RelayFirstUse { .. } => show("Transfer via relay", RELAY_NOTICE),
                Event::Transfer(t) => {
                    let prev = last.insert(t.id.clone(), t.status);
                    if prev == Some(t.status) || prev.is_none() && t.status.is_terminal() {
                        continue;
                    }
                    let verb = match t.direction {
                        Direction::Send => "Sent",
                        Direction::Receive => "Received",
                    };
                    match t.status {
                        TransferStatus::Completed => show(
                            &format!("{verb} {}", t.label),
                            &format!("{} · {}", format_bytes(t.total_bytes), t.peer_name),
                        ),
                        TransferStatus::Failed => show(
                            &format!("Transfer failed: {}", t.label),
                            t.error.as_deref().unwrap_or("Unknown error"),
                        ),
                        TransferStatus::Rejected if t.direction == Direction::Send => show(
                            &format!("{} declined {}", t.peer_name, t.label),
                            t.error.as_deref().unwrap_or(""),
                        ),
                        _ => {}
                    }
                    if t.status.is_terminal() {
                        last.remove(&t.id);
                    }
                }
                _ => {}
            }
        }
    });
}

fn show(summary: &str, body: &str) {
    let summary = summary.to_owned();
    let body = body.to_owned();
    // Some backends block (D-Bus round trip); keep them off the runtime.
    std::thread::spawn(move || {
        let result = notify_rust::Notification::new()
            .appname("Gigapair")
            .summary(&summary)
            .body(&body)
            .show();
        if let Err(e) = result {
            tracing::debug!("notification not shown: {e}");
        }
    });
}
