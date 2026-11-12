//! System tray icon (RF-14): global progress, open the window, quit.
//!
//! - Linux: StatusNotifierItem over D-Bus (`ksni`). Shown by KDE, COSMIC,
//!   Xfce, Cinnamon and others; GNOME needs the AppIndicator extension.
//! - Windows and macOS: `tray-icon`, on the main thread, whose events GPUI's
//!   own event loop already pumps.
//!
//! When no tray is available, closing the window quits the app as before.

use gigapair_core::types::{Direction, TransferInfo, TransferStatus, format_bytes};
use tokio::sync::mpsc::UnboundedSender;

use crate::strings as s;

#[cfg(target_os = "linux")]
mod sni;
#[cfg(target_os = "linux")]
pub use sni::Tray;

#[cfg(any(windows, target_os = "macos"))]
mod native;
#[cfg(any(windows, target_os = "macos"))]
pub use native::Tray;

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
mod none;
#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
pub use none::Tray;

/// What the user asked for from the tray (or from a second launch).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    Open,
    Quit,
}

pub type ActionSender = UnboundedSender<TrayAction>;

/// What the tray shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayView {
    /// One line: the first (disabled) menu entry and the tooltip text.
    pub status: String,
    /// Draws the "busy" badge on the icon.
    pub busy: bool,
}

impl TrayView {
    #[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
    pub fn tooltip(&self) -> String {
        format!("{}\n{}", s::APP_NAME, self.status)
    }
}

/// Sums up unfinished transfers for the tray.
pub fn summarize(connected: bool, transfers: &[TransferInfo]) -> TrayView {
    if !connected {
        return TrayView {
            status: s::TRAY_DISCONNECTED.into(),
            busy: false,
        };
    }
    let requests = transfers
        .iter()
        .filter(|t| t.direction == Direction::Receive && t.status == TransferStatus::PendingAccept)
        .count();
    let moving: Vec<&TransferInfo> = transfers
        .iter()
        .filter(|t| {
            !t.status.is_terminal()
                && !matches!(
                    t.status,
                    TransferStatus::PendingAccept | TransferStatus::WaitingAccept
                )
        })
        .collect();
    let busy = moving.iter().any(|t| t.status.is_active());

    let mut parts = Vec::new();
    if !moving.is_empty() {
        let total: u64 = moving.iter().map(|t| t.total_bytes).sum();
        let done: u64 = moving.iter().map(|t| t.done_bytes.min(t.total_bytes)).sum();
        let percent = if total == 0 {
            0
        } else {
            (done as f64 / total as f64 * 100.0).floor() as u32
        };
        parts.push(s::tray_transfers(moving.len(), percent));
        let speed: u64 = moving
            .iter()
            .filter(|t| t.status == TransferStatus::Transferring)
            .map(|t| t.speed_bps)
            .sum();
        if speed > 0 {
            parts.push(format!("{}/s", format_bytes(speed)));
        } else if !busy {
            parts.push(s::TRAY_PAUSED.into());
        }
    }
    if requests > 0 {
        parts.push(s::tray_requests(requests));
    }
    TrayView {
        status: if parts.is_empty() {
            s::TRAY_IDLE.into()
        } else {
            parts.join(" · ")
        },
        busy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(status: TransferStatus, done: u64, total: u64, speed: u64) -> TransferInfo {
        TransferInfo {
            id: format!("{status:?}{done}"),
            direction: Direction::Receive,
            peer_id: "p".into(),
            peer_name: "Laptop".into(),
            status,
            label: "x".into(),
            total_bytes: total,
            done_bytes: done,
            file_count: 1,
            speed_bps: speed,
            eta_secs: None,
            conn: None,
            error: None,
            note: None,
            dest_dir: None,
            created_at: 0,
            finished_at: None,
        }
    }

    #[test]
    fn idle_and_disconnected() {
        assert_eq!(summarize(true, &[]).status, s::TRAY_IDLE);
        assert!(!summarize(true, &[]).busy);
        assert_eq!(summarize(false, &[]).status, s::TRAY_DISCONNECTED);
    }

    #[test]
    fn progress_speed_and_requests() {
        let list = [
            t(TransferStatus::Transferring, 30, 100, 2 * 1024 * 1024),
            t(TransferStatus::Queued, 0, 100, 0),
            t(TransferStatus::PendingAccept, 0, 500, 0),
        ];
        let v = summarize(true, &list);
        assert!(v.busy);
        assert_eq!(
            v.status,
            format!(
                "{} · {}/s · {}",
                s::tray_transfers(2, 15),
                format_bytes(2 * 1024 * 1024),
                s::tray_requests(1)
            )
        );
    }

    #[test]
    fn paused_only() {
        let v = summarize(true, &[t(TransferStatus::Paused, 50, 100, 0)]);
        assert!(!v.busy);
        assert_eq!(
            v.status,
            format!("{} · {}", s::tray_transfers(1, 50), s::TRAY_PAUSED)
        );
    }
}
