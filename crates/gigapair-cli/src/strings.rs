//! User-facing text of the CLI, kept in one place for later translation.

use gigapair_core::api::StatusInfo;
use gigapair_core::types::format_bytes;

pub const NO_REQUESTS: &str = "No incoming transfers waiting for an answer.";
pub const ACCEPT_HINT: &str =
    "Accept with `gigapair accept <id> [--to <folder>]` or reject with `gigapair reject <id>`.";
pub const ACCEPTED: &str = "Accepted";
pub const REJECTED: &str = "Rejected";
pub const HISTORY_CLEARED: &str = "History cleared.";
pub const NO_HISTORY: &str = "No finished transfers yet.";
pub const NO_ACTIVE: &str = "No active transfers.";
pub const NO_CONTACTS: &str = "No contacts yet. Run `gigapair pair` here, or `gigapair pair <code>` with the other device's code.";
pub const SENDING_AGAIN: &str = "Sending again as transfer";
pub const SERVICE_STARTED: &str = "Started the Gigapair service in the background.";
pub const SERVICE_STOPPED: &str =
    "The Gigapair service stopped; the transfer resumes when it starts again.";
pub const DETACH_HINT: &str =
    "Press Ctrl+C to stop watching; the transfer keeps going in the background.";
pub const WATCH_HINT: &str = "Refreshing. Press Ctrl+C to stop watching.";
pub const UNSET: &str = "(not set)";
pub const SAVED: &str = "Saved";
pub const RENAMED: &str = "Renamed to";
pub const REMOVED: &str = "Removed";
pub const CONNECTING: &str = "Connecting to the other device…";
pub const FINGERPRINT_INTRO: &str = "Both devices now show these four words:";
pub const FINGERPRINT_QUESTION: &str = "Does the other device show the same words? [y/N] ";
pub const NEED_TTY_OR_YES: &str =
    "can't ask for confirmation without a terminal; pass --yes after checking the words";
pub const PAIR_CANCELLED: &str = "Pairing cancelled.";
pub const PAIR_CODE_INTRO: &str = "Paste this code on the other device (`gigapair pair <code>` or \"Add contact\" in the app).\nIt works once and expires in 10 minutes.";
pub const PAIR_WAITING: &str = "Waiting for the other device…";
pub const CODE_EXPIRED: &str = "the pairing code expired; run `gigapair pair` again";
pub const NO_DAEMON_EXE: &str =
    "gigapaird was not found next to gigapair; build the workspace with `cargo build`";
pub const INSTALL_FAILED: &str = "could not install the service";
pub const ALREADY_RUNNING: &str = "The service is already running.";
pub const STARTED: &str = "The service is running.";
pub const START_TIMEOUT: &str = "the service did not start; check the log folder";
pub const NOT_RUNNING: &str = "The service is not running.";
pub const STOPPED: &str = "The service was stopped.";

pub fn name_question(default: &str) -> String {
    format!("Name for this contact [{default}]: ")
}

pub fn paired_with(name: &str) -> String {
    format!("Paired with {name}. Send files with `gigapair send \"{name}\" <files>`.")
}

pub fn connected_to(name: &str) -> String {
    format!("Connected to {name}.")
}

pub fn pair_request_from(name: &str) -> String {
    format!("{name} used the code.")
}

pub fn auto_accept_set(contact: &str, on: bool) -> String {
    if on {
        format!("Transfers from {contact} are now accepted automatically.")
    } else {
        format!("Transfers from {contact} now need your approval.")
    }
}

pub fn sending(label: &str, peer: &str, bytes: u64, id: &str) -> String {
    format!("Sending {label} ({}) to {peer} [{id}]", format_bytes(bytes))
}

pub fn status_header(info: &StatusInfo) -> String {
    let mut lines = vec![
        format!("Device:      {}", info.device_name),
        format!("Node id:     {}", info.node_id),
        format!(
            "Relay:       {}{}",
            info.home_relay.as_deref().unwrap_or("none"),
            if info.direct_only {
                " (direct only)"
            } else {
                ""
            }
        ),
        format!("Downloads:   {}", info.download_dir),
        format!(
            "Keep awake:  {}",
            match (info.keep_awake_enabled, info.keeping_awake) {
                (false, _) => "off",
                (true, true) => "on, preventing sleep now",
                (true, false) => "on",
            }
        ),
    ];
    if info.pending_requests > 0 {
        lines.push(format!(
            "Requests:    {} waiting (`gigapair requests`)",
            info.pending_requests
        ));
    }
    lines.join("\n")
}

pub fn service_status(installed: bool, location: Option<&str>, running: bool) -> String {
    format!(
        "Starts at login: {}{}\nRunning:         {}",
        if installed { "yes" } else { "no" },
        location
            .filter(|_| installed)
            .map(|l| format!(" ({l})"))
            .unwrap_or_default(),
        if running { "yes" } else { "no" }
    )
}
