# Gigapair

**Move huge files between the devices you trust. Pair once, send anytime.**

Gigapair is a desktop app and CLI for sending very large files (tens to hundreds of GB, any file type) directly between two computers: peer-to-peer, end-to-end encrypted, verified and resumable. No cloud storage, no size limits, no one-time codes for every transfer.

*Originally built to send talk recordings between the organizers of [builderia.lat](https://builderia.lat).*

> **Status:** pre-release. The service, CLI and desktop app work when built from source (see [Building from source](#building-from-source)); there are no installers yet. See the [roadmap](#roadmap).

## Why Gigapair

**Large files first.** Gigapair is built for transfers of hundreds of GB:

- **Survives interruptions.** If the network drops, the app closes or a computer reboots, the transfer resumes where it stopped. Blocks that were already verified are not sent again.
- **Doesn't take twice the space.** Data is written next to its final location and moved into place with an atomic rename, so receiving a 200 GB file needs 200 GB of space, not 400 GB.
- **Doesn't stall because a computer went to sleep.** Gigapair keeps the system awake only while a transfer is active, and lets it sleep again when it finishes.

**Frequent contacts.** Pair two devices once, then send by dragging and dropping onto a contact. You can even send to a headless server that receives on its own, running Gigapair as a system service with auto-accept turned on.

## Features (v1)

- Send single files, multiple files or whole folders to a paired contact, by drag and drop or file picker.
- One-time pairing with a code shown as text and as a QR, confirmed with a 4-word fingerprint on both sides.
- End-to-end encryption with QUIC / TLS 1.3. Each device is identified by an Ed25519 public key.
- BLAKE3 verification of every block as it arrives, and of the whole file at the end. A file is never marked complete unless its root hash matches.
- Automatic resume after network drops, app restarts and reboots.
- Works behind NAT and CGNAT: direct connection with hole punching, encrypted relay as a fallback.
- Always shows whether a transfer is **direct** or **via relay**, and which relay is used.
- Transfer queue with progress, speed, ETA, pause, resume and cancel.
- Receiver approval by default; per-contact **auto-accept** for trusted devices and servers.
- Free-space check before accepting a transfer.
- Desktop notifications, tray icon and transfer history.
- Full-featured CLI and a background service for headless machines.
- Windows 10/11, macOS (Apple Silicon and Intel) and Linux (Wayland and X11).

Not in scope for v1: one-to-many or swarm transfers, cloud storage or store-and-forward when the receiver is offline, two-way folder sync, video compression and mobile apps.

## How it works

Each computer runs a small background service, `gigapaird`, which owns the network node, the transfer queue and the persisted state. The desktop app and the CLI are thin clients that talk to the service over local IPC (a Unix socket on macOS and Linux, a named pipe on Windows). Closing the window doesn't stop transfers, and a server can receive without any UI.

```
 Device A                                      Device B
┌──────────────────────┐                     ┌──────────────────────┐
│ Desktop app  │  CLI  │                     │ Desktop app  │  CLI  │
├──────────────┴───────┤                     ├──────────────┴───────┤
│   local IPC          │                     │   local IPC          │
├──────────────────────┤   QUIC (direct)     ├──────────────────────┤
│   gigapaird          │◄═══════════════════►│   gigapaird          │
│   iroh (QUIC)        │                     │   iroh (QUIC)        │
└──────────────────────┘                     └──────────────────────┘
             ╲                                     ╱
              ╲──────►  relay (fallback)  ◄───────╱
                 forwards encrypted bytes only
```

Sending a file works like this:

1. The sender hashes the files with BLAKE3 while streaming them from disk (shown as "preparing").
2. The sender asks the receiver to accept a transfer, including file names and sizes.
3. The receiver checks free space and accepts, either automatically or after the user confirms.
4. Data is downloaded into a hidden `.gigapair-partial/` folder inside the destination folder, and each block is verified as it arrives.
5. Once the root hash matches, the file is renamed into place. It never appears under its final name while incomplete.

Gigapair is built on [iroh](https://github.com/n0-computer/iroh) for connectivity (QUIC, hole punching, relays). Transfers use a small protocol of its own on top of iroh: files are split into 4 MiB blocks, each checked against a BLAKE3 block list that the offer pins with a hash, and the whole file is checked against its BLAKE3 root hash (the same value `b3sum` prints) before it is moved into place. See [docs/transfer-protocol.md](docs/transfer-protocol.md). The UI uses [GPUI](https://www.gpui.rs/) with [GPUI Kit](https://gpui-kit.com).

## Privacy and relays

- **No telemetry.** Nothing leaves your computer except the connection to your contact and, when needed, the relay. Logs stay local; you can export them to report a bug.
- **Relays can't read your files.** When a direct connection isn't possible, traffic goes through a relay (by default, the public relays run by n0). A relay only forwards end-to-end encrypted data. It **can** see both devices' IP addresses, when a transfer happens and how much data is sent.
- **You stay in control.** The first time a relay is used, Gigapair tells you. In settings you can use your own relay or enable **direct only**, which fails instead of using an intermediary. History records whether each transfer was direct or relayed.

## Installation

Download from the [latest release](https://github.com/dopazo/gigapair/releases/latest). Every package contains the desktop app (`gigapair-app`), the CLI (`gigapair`, alias `gpair`) and the background service (`gigapaird`), installed side by side.

| Platform | How |
| --- | --- |
| Windows | `gigapair-x86_64-pc-windows-msvc.msi`: installs to Program Files, adds a **Gigapair** entry to the Start menu and puts the CLI on the `PATH`. Or, in PowerShell: `powershell -ExecutionPolicy Bypass -c "irm https://github.com/dopazo/gigapair/releases/latest/download/gigapair-installer.ps1 \| iex"` |
| macOS (Apple silicon and Intel) | `curl --proto '=https' --tlsv1.2 -LsSf https://github.com/dopazo/gigapair/releases/latest/download/gigapair-installer.sh \| sh`, then run `gigapair-app` |
| Linux (x86_64, glibc 2.35+) | Same shell installer, then run `gigapair-app` |
| Any | The `.zip`/`.tar.xz` archives: unpack and keep the four programs in the same folder |

The shell and PowerShell installers put the programs in `~/.cargo/bin` (Windows: `%USERPROFILE%\.cargo\bin`) and add it to the `PATH`. They never update themselves: Gigapair contacts nothing but your contacts and the relay.

Not available yet: a macOS `.app` bundle (DMG), AppImage, `.deb`, winget, Scoop, Homebrew and `cargo install`. They are planned for later releases.

Every release includes SHA-256 checksums (`sha256.sum` and one `.sha256` per file) and [GitHub build attestations](https://docs.github.com/en/actions/security-for-github-actions/using-artifact-attestations). To verify a download:

```sh
gh attestation verify <downloaded-file> --repo dopazo/gigapair
```

On Linux the desktop app needs the usual desktop libraries (Vulkan driver, `libxkbcommon`, `fontconfig`), present on any desktop install.

### Unsigned binaries

v1 binaries are not code-signed, so your operating system will warn you the first time you run them:

- **Windows (SmartScreen):** click **More info**, then **Run anyway**.
- **macOS (Gatekeeper):** open **System Settings → Privacy & Security** and click **Open Anyway**.
- **Linux:** no signing warning.

The shell and PowerShell installers download with `curl`/`irm`, which don't mark files as downloaded from the internet, so they usually avoid these warnings.

## Usage

### Desktop app

1. **Pair:** click **Add contact**. Send the code (or a photo of the QR) to the other person by any channel. They paste it in **Add contact**. Check that the 4-word fingerprint matches on both screens and name the contact.
2. **Send:** drag files or folders onto the contact.
3. **Receive:** accept the incoming request, or turn on **Auto-accept** for that contact.

Closing the window keeps Gigapair in the system tray (the notification area on Windows, the panel on Linux), showing the overall progress. Click the tray icon to bring the window back; quit from its menu. Quitting the app doesn't stop transfers: the background service keeps them going. To make the close button quit instead, turn off **Keep Gigapair in the tray** in Settings (`gigapair config set close_to_tray off`). Launching the app again while it's in the tray shows the existing window. `gigapair-app --background` starts it in the tray without opening the window.

On Linux the tray icon uses the StatusNotifierItem standard: KDE, COSMIC, Xfce, Cinnamon and most panels show it; GNOME needs the "AppIndicator and KStatusNotifierItem Support" extension. Without a tray, closing the window quits the app.

### CLI

The CLI binary is `gigapair` (optional short alias: `gpair`).

| Command | Description |
| --- | --- |
| `gigapair pair` | Show a pairing code as text and as a QR in the terminal |
| `gigapair pair <code>` | Add a contact from another device's code |
| `gigapair contacts` | List contacts; subcommands `rename`, `remove` and `trust <contact> --auto-accept on\|off` |
| `gigapair send <contact> <paths...>` | Send files or folders and show progress |
| `gigapair requests` | List pending incoming requests |
| `gigapair accept <id>` / `reject <id>` | Accept or reject a request |
| `gigapair status [--watch]` | Active transfers, speed and connection type |
| `gigapair history [--clear]` | Sent and received transfers |
| `gigapair pause\|resume\|cancel\|retry <id>` | Control a transfer (ids can be shortened) |
| `gigapair config get\|set <key> <value>` | Read or change settings |
| `gigapair service install\|uninstall\|start\|stop\|status` | Manage the background service |

### Headless server

To set up a machine that only receives:

```sh
gigapair service install
gigapair pair <code-from-your-desktop>
gigapair contacts trust <contact> --auto-accept on
```

Example `config.toml`:

```toml
download_dir = "/srv/incoming"
keep_awake = true

[[contacts]]
name = "workstation"
node_id = "<node id>"
auto_accept = true
```

The service uses each platform's native mechanism:

| Platform | Mechanism |
| --- | --- |
| Linux | systemd user unit (with `loginctl enable-linger` to run without a login) |
| macOS | LaunchAgent in `~/Library/LaunchAgents` |
| Windows | Start at login (`HKCU\...\Run`, no administrator rights needed) |

System-wide variants (a Windows Service, a Linux system unit with a dedicated user) are planned with the installers.

## File handling

- Existing files are never overwritten. A name clash becomes `name (1).ext`.
- Folder structure (including empty folders) and modification times are preserved.
- Symbolic links are skipped with a warning.
- File names that are invalid on the receiving system (for example `:` on Windows) are adjusted. Long paths are supported on Windows.

## Where data is stored

| Data | Windows | macOS | Linux |
| --- | --- | --- | --- |
| Config (`config.toml`) | `%APPDATA%\gigapair\Gigapair\config\` | `~/Library/Application Support/dev.gigapair.Gigapair/` | `~/.config/gigapair/` |
| Database (contacts, queue, history) | `%LOCALAPPDATA%\gigapair\Gigapair\data\` | `~/Library/Application Support/dev.gigapair.Gigapair/` | `~/.local/share/gigapair/` |
| Logs | `%LOCALAPPDATA%\gigapair\Gigapair\data\logs\` | `~/Library/Logs/Gigapair/` | `~/.local/state/gigapair/` |
| Node private key | Credential Manager | Keychain | Secret Service (GNOME Keyring, KWallet) |
| Default downloads | `%USERPROFILE%\Downloads\Gigapair\` | `~/Downloads/Gigapair/` | `~/Downloads/Gigapair/` |
| In-progress data | `.gigapair-partial\` in the destination folder | same | same |

If no keyring is available (headless servers), the key is stored in a file with `0600` permissions in the data folder; set `key_storage = "file"` to choose this explicitly.

### Logs

The service and the desktop app each write one file per day to the logs folder (`gigapaird.YYYY-MM-DD.log` and `gigapair-app.YYYY-MM-DD.log`), keeping the last 7 days. Crashes are recorded there too, with a backtrace. `gigapair logs` prints the folder and its newest files, and **Settings → Logs folder → Open** opens it. Logs never leave the computer and never contain keys or pairing codes: attach them when reporting a problem.

For more detail, set `GIGAPAIR_LOG` before starting the service or the app, for example `GIGAPAIR_LOG=debug` (PowerShell: `$env:GIGAPAIR_LOG = "debug"`), or `info,gigapair_core=debug` for only Gigapair's own messages. A service that is already running keeps its level until it is restarted. `gigapair-app --verbose` and `gigapaird --verbose` also print the log to the terminal.

Setting the `GIGAPAIR_HOME` environment variable (or passing `--home <dir>` to `gigapair` and `gigapaird`) keeps everything under one folder instead. This is how you run two instances on one computer.

## Roadmap

| Stage | Deliverable | Done when |
| --- | --- | --- |
| 1. Core + CLI ✅ (implemented; real-network acceptance test pending) | `gigapair-core`, `gigapaird` and CLI with pair, send, accept, status and resume | 50 GB between Windows and macOS on different networks, with two forced interruptions, identical hash |
| 2. Minimal UI ✅ (implemented) | GPUI window with contacts, drag and drop and progress | A full transfer without touching the terminal |
| 3. Robustness ✅ (implemented; reboot test pending) | Keep-awake, persistence, queue, history, notifications, tray | Reboot mid-transfer and resume automatically |
| 4. Distribution 🟡 (release workflow in place: MSI, shell/PowerShell installers, archives, checksums, attestations; DMG/AppImage/`.deb` and system-wide service later) | Releases (MSI, DMG, AppImage, `.deb`), checksums, attestations, service install | Clean install on all three OSes and a headless server receiving |

## Building from source

Requires Rust 1.90 or newer ([rustup](https://rustup.rs/)).

- **Windows:** install the [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with the "Desktop development with C++" workload (MSVC and the Windows SDK).
- **macOS:** Xcode command line tools (`xcode-select --install`).
- **Linux (Debian/Ubuntu):** `sudo apt install libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libvulkan-dev libfontconfig-dev libxcb1-dev libx11-xcb-dev libssl-dev pkg-config`

```sh
git clone https://github.com/dopazo/gigapair
cd gigapair
cargo build --release
```

This produces `gigapair-app`, `gigapair` (and `gpair`) and `gigapaird` in `target/release/`. Keep them together: the app and the CLI start `gigapaird` from the same folder when it isn't running.

```sh
cargo run --release -p gigapair              # desktop app
cargo run --release --bin gigapair -- status # CLI
cargo run --release --bin gigapaird          # service in the foreground
```

### Trying it on one computer

Run two instances with separate data folders and pair them with each other:

```sh
target/release/gigapair --home ./demo-a pair            # shows a code, waits
target/release/gigapair --home ./demo-b pair <code>     # in a second terminal
target/release/gigapair --home ./demo-a send <name-of-b> <some file>
target/release/gigapair --home ./demo-b requests
target/release/gigapair --home ./demo-b accept <id>
```

For the app, set `GIGAPAIR_HOME` before launching it (PowerShell: `$env:GIGAPAIR_HOME = "C:\demo-a"; .\target\release\gigapair-app.exe`).

### Tests

```sh
cargo test --workspace
# Larger transfer and throughput check (generated data, nothing committed):
GIGAPAIR_TEST_MB=4096 cargo test --release -p gigapair-core --test transfer throughput -- --ignored --nocapture
```

The workspace contains:

| Crate | Purpose |
| --- | --- |
| `crates/gigapair-core` | Transfer logic and protocol, no UI dependencies |
| `crates/gigapair` | The executables (`gigapair-app`, `gigapair`, `gpair`, `gigapaird`) as thin wrappers, so a release ships them together |
| `crates/gigapaird` | Background service, including system service installation |
| `crates/gigapair-cli` | CLI client |
| `crates/gigapair-app` | GPUI desktop client; starts the service if it isn't running |

## License

Licensed under the [MIT License](LICENSE).
