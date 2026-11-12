//! User-facing text of the app, kept in one place for later translation.

use gigapair_core::types::format_bytes;

pub const APP_NAME: &str = "Gigapair";
pub const CONTACTS: &str = "Contacts";
pub const ADD_CONTACT: &str = "Add contact";
pub const HISTORY: &str = "History";
pub const SETTINGS: &str = "Settings";
pub const NO_CONTACTS: &str = "No contacts yet. Pair with another device to start.";
pub const ONLINE: &str = "Online";
pub const OFFLINE: &str = "Offline";
pub const KEEPING_AWAKE: &str = "Preventing sleep";
pub const CONNECTING_SERVICE: &str = "Connecting to the Gigapair service…";
pub const SELECT_CONTACT: &str = "Select a contact to send files, or add one.";

pub const DROP_HINT: &str = "Drop files or folders here";
pub const CHOOSE_FILES: &str = "Choose files…";
pub const CHOOSE_FOLDER: &str = "Choose folder…";
pub const AUTO_ACCEPT_LABEL: &str = "Accept transfers from this contact automatically";
pub const RENAME: &str = "Rename";
pub const REMOVE: &str = "Remove contact";
pub const REMOVE_CONFIRM: &str = "Click again to remove";

pub const INCOMING: &str = "Incoming requests";
pub const TRANSFERS: &str = "Transfers";
pub const NO_TRANSFERS: &str = "No active transfers.";
pub const ACCEPT: &str = "Accept";
pub const ACCEPT_TO: &str = "Accept to…";
pub const REJECT: &str = "Reject";
pub const PAUSE: &str = "Pause";
pub const RESUME: &str = "Resume";
pub const CANCEL: &str = "Cancel";
pub const RETRY: &str = "Retry";
pub const OPEN_FOLDER: &str = "Open folder";
pub const CLEAR_HISTORY: &str = "Clear history";
pub const NO_HISTORY: &str = "No finished transfers yet.";
pub const DIRECT: &str = "Direct";
pub const VIA_RELAY: &str = "Via relay";
pub const RELAY_SLOWER: &str =
    "Connected through a relay; this can be slower than a direct connection.";
pub const DISMISS: &str = "Got it";

pub const SHARE_CODE_TITLE: &str = "Share your code";
pub const SHARE_CODE_TEXT: &str = "Create a one-time code and send it to the other device (chat, email, or scan the QR with your phone). It expires in 10 minutes.";
pub const CREATE_CODE: &str = "Create code";
pub const NEW_CODE: &str = "New code";
pub const COPY: &str = "Copy";
pub const COPIED: &str = "Code copied";
pub const WAITING_OTHER: &str = "Waiting for the other device to use the code…";
pub const USE_CODE_TITLE: &str = "Use a code";
pub const USE_CODE_TEXT: &str = "Paste the code shown on the other device.";
pub const CODE_PLACEHOLDER: &str = "gp1-…";
pub const CONNECT: &str = "Connect";
pub const CONFIRM_TITLE: &str = "Confirm the pairing";
pub const CONFIRM_TEXT: &str = "Both devices must show the same four words. If they differ, reject: someone may have changed the code.";
pub const NAME_PLACEHOLDER: &str = "Contact name";
pub const WORDS_MATCH: &str = "Words match, pair";

pub const SETTINGS_DOWNLOADS: &str = "Download folder";
pub const CHANGE: &str = "Change…";
pub const SETTINGS_DEVICE: &str = "Device name";
pub const SAVE: &str = "Save";
pub const SETTINGS_KEEP_AWAKE: &str = "Keep the computer awake while transfers run";
pub const SETTINGS_NOTIFICATIONS: &str = "Desktop notifications";
pub const SETTINGS_START_LOGIN: &str = "Start the Gigapair service at login";
pub const SETTINGS_DIRECT_ONLY: &str = "Direct connections only (never use a relay)";
pub const SETTINGS_DISCOVERY: &str =
    "Let contacts find this device after network changes (publishes only the relay address)";
pub const SETTINGS_RELAY: &str = "Custom relay URL (empty: public n0 relays)";
pub const SETTINGS_LIMIT: &str = "Upload limit in KiB/s (0: unlimited)";
pub const SETTINGS_NODE: &str = "This device's id";
pub const RESTART_NOTE: &str = "Some changes apply after the service restarts.";

pub fn incoming_request(name: &str, label: &str, bytes: u64) -> String {
    format!("{name} wants to send you {label} ({})", format_bytes(bytes))
}

pub fn send_to(name: &str) -> String {
    format!("Send to {name}")
}

pub fn paired(name: &str) -> String {
    format!("Paired with {name}")
}

pub fn pair_request(name: &str) -> String {
    format!("{name} used your pairing code")
}

pub fn expires_in(secs: i64) -> String {
    format!("Expires in {}:{:02}", secs / 60, secs % 60)
}

pub fn sent_warnings(n: usize) -> String {
    format!("{n} item(s) skipped (symlinks or unreadable); see the service log")
}

pub const SETTINGS_CLOSE_TO_TRAY: &str =
    "Keep Gigapair in the tray when the window is closed (quit from the tray icon)";
pub const TRAY_OPEN: &str = "Open Gigapair";
pub const TRAY_QUIT: &str = "Quit Gigapair";
pub const TRAY_IDLE: &str = "No active transfers";
pub const TRAY_DISCONNECTED: &str = "Connecting to the service…";
pub const TRAY_PAUSED: &str = "paused";

pub fn tray_transfers(n: usize, percent: u32) -> String {
    if n == 1 {
        format!("1 transfer, {percent}%")
    } else {
        format!("{n} transfers, {percent}%")
    }
}

pub fn tray_requests(n: usize) -> String {
    if n == 1 {
        "1 request waiting".into()
    } else {
        format!("{n} requests waiting")
    }
}
