# Using Gigapair

## Installation

Download from the [releases page](https://github.com/dopazo/gigapair/releases), or install from a terminal (below). Every package contains the desktop app (`gigapair-app`), the CLI (`gigapair`, alias `gpair`) and the background service (`gigapaird`), installed side by side.

| Platform | How |
| --- | --- |
| Windows | `gigapair-x86_64-pc-windows-msvc.msi`: installs to Program Files for all users, adds a **Gigapair** entry to the Start menu, puts the CLI on the `PATH` and adds a firewall exception for the service. Asks for administrator rights |
| macOS (Apple silicon and Intel) | The shell installer below, then run `gigapair-app` |
| Linux (x86_64, glibc 2.35+) | The shell installer below, then run `gigapair-app` |
| Any | The `.zip`/`.tar.xz` archives: unpack and keep the four programs in the same folder |

### From a terminal

macOS and Linux:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/dopazo/gigapair/releases/download/v0.1.0-beta.1/gigapair-installer.sh | sh
```

Windows (PowerShell): downloads the MSI and installs it with only a progress bar. Windows asks once for administrator rights (the UAC prompt, which you approve). The MSI is the recommended way, from a terminal too, because only the MSI adds the firewall exception:

```powershell
$msi = "$env:TEMP\gigapair.msi"; $log = "$env:TEMP\gigapair-install.log"
iwr https://github.com/dopazo/gigapair/releases/download/v0.1.0-beta.1/gigapair-x86_64-pc-windows-msvc.msi -OutFile $msi
$p = Start-Process msiexec -ArgumentList "/i `"$msi`" /passive /norestart /l*v `"$log`"" -Verb RunAs -Wait -PassThru
if ($p.ExitCode -in 0, 3010) { 'Gigapair installed.' } else { "Install failed (msiexec exit code $($p.ExitCode)). Log: $log" }
```

**Unattended (scripts, servers, AI agents).** Run from a prompt that already has administrator rights: nothing can approve a UAC prompt on your behalf, so a script that asks for one would wait forever. This version shows no window, and its output says what happened, so whoever runs it doesn't need to read the log:

```powershell
$v = 'v0.1.0-beta.1'
$msi = "$env:TEMP\gigapair.msi"; $log = "$env:TEMP\gigapair-install.log"
$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $admin) { 'Gigapair: run this from an administrator prompt (the MSI installs for all users).'; exit 1 }
iwr "https://github.com/dopazo/gigapair/releases/download/$v/gigapair-x86_64-pc-windows-msvc.msi" -OutFile $msi
$p = Start-Process msiexec -ArgumentList "/i `"$msi`" /qn /norestart /l*v `"$log`"" -Wait -PassThru
switch ($p.ExitCode) {
  0       { "Gigapair installed: $(& "$env:ProgramFiles\gigapair\bin\gigapair.exe" --version)" }
  3010    { 'Gigapair installed; restart Windows to finish.' }
  default {
    "Gigapair install failed: msiexec exit code $($p.ExitCode). Full log: $log"
    Select-String -Path $log -Pattern '^Error \d{4}\.' | Select-Object -Last 3 | ForEach-Object { $_.Line.Trim() }
    exit $p.ExitCode
  }
}
```

It ends with exit code 0 when Gigapair is installed, and otherwise with the code from `msiexec` (for example 1603, a fatal error; 1618, another installation is running), printing the installer's own error messages, such as:

```text
Gigapair install failed: msiexec exit code 1603. Full log: C:\Users\me\AppData\Local\Temp\gigapair-install.log
Error 1925. You do not have sufficient privileges to complete this installation for all users of the machine.  Log on as administrator and then retry this installation.
```

`exit` ends the PowerShell session: run it as a script (`powershell -File install.ps1`) or with `powershell -Command`, not pasted into a window you want to keep. A new terminal is needed to find `gigapair` on the `PATH`; the full path is `C:\Program Files\gigapair\bin\gigapair.exe`.

The commands name a version because so far there are only pre-releases, which GitHub never marks as *latest*; use the newest one from the [releases page](https://github.com/dopazo/gigapair/releases). From the first stable release, `releases/latest/download/...` will always point to the newest.

The shell installer puts the programs in your user folder, `~/.cargo/bin`, and adds it to your `PATH`; it needs no administrator rights. They never update themselves: Gigapair contacts nothing but your contacts and the relay.

**Windows without administrator rights.** If you can't install the MSI, a PowerShell installer installs for your user only, in `%USERPROFILE%\.cargo\bin`:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/dopazo/gigapair/releases/download/v0.1.0-beta.1/gigapair-installer.ps1 | iex"
```

It adds no Start menu entry (start the app with `gigapair-app`) and no firewall exception: Windows asks the first time the service runs, and allowing it needs an administrator. Without the exception, contacts can't open a connection to this computer directly, so more transfers go through a relay and may be slower. Prefer the MSI.

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

## Desktop app

1. **Pair:** click **Add contact**. Send the code (or a photo of the QR) to the other person by any channel. They paste it in **Add contact**. Check that the 4-word fingerprint matches on both screens and name the contact.
2. **Send:** drag files or folders onto the contact.
3. **Receive:** accept the incoming request, or turn on **Auto-accept** for that contact.

Closing the window keeps Gigapair in the system tray (the notification area on Windows, the panel on Linux), showing the overall progress. Click the tray icon to bring the window back; quit from its menu. Quitting the app doesn't stop transfers: the background service keeps them going. To make the close button quit instead, turn off **Keep Gigapair in the tray** in Settings (`gigapair config set close_to_tray off`). Launching the app again while it's in the tray shows the existing window. `gigapair-app --background` starts it in the tray without opening the window.

On Linux the tray icon uses the StatusNotifierItem standard: KDE, COSMIC, Xfce, Cinnamon and most panels show it; GNOME needs the "AppIndicator and KStatusNotifierItem Support" extension. Without a tray, closing the window quits the app.

## CLI

The CLI binary is `gigapair` (optional short alias: `gpair`). Every command accepts `--home <dir>`, to keep all data in another folder (see [Where data is stored](#where-data-is-stored)), and `--help`.

| Command | Description |
| --- | --- |
| `gigapair pair [--name <name>] [--yes]` | Show this device's pairing code as text and as a QR, and wait for the other device to use it |
| `gigapair pair <code> [--name <name>] [--yes]` | Add a contact from the other device's code |
| `gigapair contacts [list]` | List contacts, with online status and auto-accept |
| `gigapair contacts rename <contact> <name>` | Rename a contact |
| `gigapair contacts remove <contact>` | Remove a contact |
| `gigapair contacts trust <contact> --auto-accept on\|off` | Accept transfers from this contact without asking (e.g. a server) |
| `gigapair send <contact> <paths...> [--no-wait]` | Send files or folders and show the progress; `--no-wait` returns right away |
| `gigapair requests` | List incoming transfers waiting for an answer |
| `gigapair accept <id> [--to <folder>]` | Accept a request, into the download folder or `--to` another one |
| `gigapair reject <id>` | Reject a request |
| `gigapair status [--watch]` | Active transfers, speed and connection type; `--watch` refreshes until Ctrl+C |
| `gigapair history [--limit <n>] [--clear]` | Finished transfers (the last 50 by default); `--clear` deletes the history |
| `gigapair pause <id>` / `resume <id>` | Pause or resume a transfer |
| `gigapair cancel <id>` | Cancel a transfer |
| `gigapair retry <id>` | Send a failed or cancelled transfer again |
| `gigapair config get [<key>]` | Show one setting, or all of them |
| `gigapair config set <key> <value>` | Change a setting; an empty value resets an optional one |
| `gigapair service install` / `uninstall` | Start the service at login (and now) / stop starting it at login |
| `gigapair service start` / `stop` / `status` | Start, stop or check the service |
| `gigapair logs` | Show where the log files are (service and desktop app) |
| `gigapair --version` | Print the version |

In `pair`, `--name` names the new contact (asked if missing) and `--yes` skips the four-word confirmation, for scripts.

A contact can be named by its name (any case), the start of its name, or the start of its node id (4+ characters). A transfer can be named by the first characters of its id, as long as they match only one.

### Examples

**Pair two devices.** On the first device, show a code and wait:

```sh
gigapair pair
```

On the second, use that code. Both show the same four words; confirm only if they match:

```sh
gigapair pair gp1-… --name "Studio Mac"
```

**Send** a file, several files or a folder. Progress is shown until the transfer ends (Ctrl+C stops watching; the transfer keeps going):

```sh
gigapair send "Studio Mac" interview.mov
gigapair send studio cut-a.mov cut-b.mov notes.txt   # "studio" matches "Studio Mac"
gigapair send "Studio Mac" "D:\Footage\Season 2"
gigapair send "Studio Mac" big-archive.tar --no-wait # queue it and return
```

**Receive.** List what's waiting, then accept or reject by id:

```sh
gigapair requests
#   5a861ebe  ← Studio Mac  photo-library-2025.zip  Awaiting your answer  0 B/860.0 MB (0.0%)

gigapair accept 5a86                  # into the download folder
gigapair accept 5a86 --to D:\Incoming # somewhere else
gigapair reject 5a86
```

**Follow and control transfers:**

```sh
gigapair status --watch
#   633efa0e  → Studio Mac  Season 2 raw footage/  Transferring  620.0 MB/3.2 GB (19.4%)  6.1 MB/s  ETA 6m 58s  [direct]

gigapair pause 633e
gigapair resume 633e
gigapair cancel 633e
gigapair retry 633e                   # send a failed or cancelled transfer again
gigapair history --limit 10
```

**Manage contacts:**

```sh
gigapair contacts                     # list, with online status and auto-accept
gigapair contacts rename "Studio Mac" "Edit Suite"
gigapair contacts trust "Edit Suite" --auto-accept on
gigapair contacts remove "Edit Suite"
```

**Settings.** `config get` lists them all; an empty value resets an optional one:

```sh
gigapair config get
gigapair config set download_dir D:\Incoming
gigapair config set upload_limit_kib 20000    # about 20 MB/s; 0 is unlimited
gigapair config set direct_only true          # never use a relay
gigapair config set relay_url https://relay.example.com
gigapair config set relay_url ""              # back to the public relays
```

Settings: `device_name`, `download_dir`, `keep_awake`, `notifications`, `close_to_tray`, `direct_only`, `relay_url`, `discovery`, `upload_limit_kib`, `max_parallel_downloads` and `key_storage`. Some apply after the service restarts.

**Service and logs:**

```sh
gigapair service status
gigapair service install              # start gigapaird at login, and now
gigapair service stop
gigapair logs                         # where the log files are
```

### Other programs

| Program | Options |
| --- | --- |
| `gigapair-app` | `--background`: start in the tray without opening the window. `--verbose`: also log to the terminal |
| `gigapaird` | `--home <dir>`: keep all data in another folder. `--verbose`: also log to the terminal. `--version` |

The app and the CLI start `gigapaird` on their own when it isn't running, so it rarely needs to be started by hand.

## Headless server

A machine that only receives (a home server, a NAS, a render node) needs only the CLI and the service; the desktop app installed next to them is simply never started. Linux is the best fit today; see [Limitations](#server-limitations) for Windows and macOS.

**1. Install** with the shell installer (Linux, macOS) or the unattended MSI script (Windows), as in [From a terminal](#from-a-terminal):

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/dopazo/gigapair/releases/download/v0.1.0-beta.1/gigapair-installer.sh | sh
```

**2. Start the service at boot:**

```sh
gigapair service install
loginctl enable-linger $USER      # Linux: run without anyone logged in
```

**3. Pair it and let it receive on its own.** On your desktop, click **Add contact** (or run `gigapair pair`) and copy the code; then, on the server:

```sh
gigapair pair <code-from-your-desktop> --name "Workstation"
gigapair contacts trust Workstation --auto-accept on
gigapair config set download_dir /srv/incoming
```

Check that both sides show the same four words before confirming: that check is what proves no one intercepted the code.

**4. Check it:** `gigapair status` shows whether the service runs and what it is receiving, and `gigapair history` what arrived.

Without a desktop session there is usually no keyring, so the node key is stored in a file readable only by its user (see [Where data is stored](#where-data-is-stored)).

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

### Server limitations

| Limitation | Effect |
| --- | --- |
| Windows: the service starts when its user **logs in** (`HKCU\...\Run`), not as a Windows Service | After a reboot, nothing is received until that user signs in |
| macOS: a LaunchAgent runs only in a logged-in session | Same as on Windows |
| Linux builds are x86_64 with glibc 2.35+ only | No ARM (Raspberry Pi, most NAS), no musl (Alpine), no older distributions such as RHEL 8 |
| The service runs as a user, not as a system account | Run it as a dedicated user (for example `gigapair`) if the machine is shared |
| Windows without the MSI (PowerShell installer): no firewall exception | More transfers go through a relay; install the MSI instead |

A Windows Service, a Linux system unit with a dedicated user and settings in `/etc/gigapair`, and ARM Linux builds are planned.

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

## Logs

The service and the desktop app each write one file per day to the logs folder (`gigapaird.YYYY-MM-DD.log` and `gigapair-app.YYYY-MM-DD.log`), keeping the last 7 days. Crashes are recorded there too, with a backtrace. `gigapair logs` prints the folder and its newest files, and **Settings → Logs folder → Open** opens it. Logs never leave the computer and never contain keys or pairing codes: attach them when reporting a problem.

For more detail, set `GIGAPAIR_LOG` before starting the service or the app, for example `GIGAPAIR_LOG=debug` (PowerShell: `$env:GIGAPAIR_LOG = "debug"`), or `info,gigapair_core=debug` for only Gigapair's own messages. A service that is already running keeps its level until it is restarted. `gigapair-app --verbose` and `gigapaird --verbose` also print the log to the terminal.

Setting the `GIGAPAIR_HOME` environment variable (or passing `--home <dir>` to `gigapair` and `gigapaird`) keeps everything under one folder instead. This is how you run two instances on one computer.
