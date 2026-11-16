# Installing, updating and removing Gigapair

Installing changes the user's machine: say what you will run and get their go-ahead. Steps that need administrator rights (sudo, the Windows UAC prompt) are approved by the user; an agent can't grant them itself.

## Contents

- [Which version](#which-version)
- [Install](#install)
- [Check the install](#check-the-install)
- [Unattended receiving server](#unattended-receiving-server)
- [Update and remove](#update-and-remove)
- [Where data lives](#where-data-lives)

## Which version

All releases so far are pre-releases, which GitHub never marks as *latest*, so `releases/latest/...` URLs don't work yet. Get the newest tag:

```sh
tag=$(curl -fsSL https://api.github.com/repos/dopazo/gigapair/releases | grep '"tag_name"' | head -n1 | cut -d'"' -f4)
```

```powershell
$tag = (Invoke-RestMethod https://api.github.com/repos/dopazo/gigapair/releases)[0].tag_name
```

If each of your commands runs in a fresh shell, set `tag` in the same command as the download, or write the literal tag (e.g. `v0.1.0-beta.4`) into the URL.

## Install

Use the method in this table. Each one lets contacts connect to this device directly; a direct connection is what makes transfers fast, and a relay is only a fallback.

| System | Method |
| --- | --- |
| Windows 10/11 | The MSI. It adds the firewall exception that direct connections need |
| macOS 11+ | The DMG |
| Debian, Ubuntu and derivatives | The `.deb` |
| Fedora, RHEL, openSUSE | The `.rpm` |
| Any other Linux (x86_64, glibc 2.35+), or no sudo | The shell installer, per user, into `~/.cargo/bin` |

There are no builds for ARM Linux or musl (Alpine) yet.

### Windows

The MSI installs to `C:\Program Files\gigapair\bin\` for all users, with a Start menu entry, the CLI on the `PATH` and the firewall exception. Windows shows a UAC prompt that the user approves:

```powershell
$msi = "$env:TEMP\gigapair.msi"; $log = "$env:TEMP\gigapair-install.log"
iwr "https://github.com/dopazo/gigapair/releases/download/$tag/gigapair-x86_64-pc-windows-msvc.msi" -OutFile $msi
$p = Start-Process msiexec -ArgumentList "/i `"$msi`" /passive /norestart /l*v `"$log`"" -Verb RunAs -Wait -PassThru
$p.ExitCode   # 0 installed; 3010 installed, restart needed; anything else failed
```

From a prompt that already runs as administrator (a server, a script), replace `/passive` with `/qn` and drop `-Verb RunAs`, so nothing waits for a prompt. If it fails, the reason is in the log: `Select-String -Path $log -Pattern '^Error \d{4}\.' | Select-Object -Last 3`. Exit code 1603 usually means missing administrator rights.

If the user can't approve administrator rights, first suggest that someone who can installs the MSI. As a last resort, a per-user PowerShell installer needs no administrator rights. Before using it, tell the user plainly: it can't add the firewall exception, so contacts often can't connect to this computer directly, and transfers may fall back to a relay, which is slower. Use it only if they accept that:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/dopazo/gigapair/releases/download/$tag/gigapair-installer.ps1 | iex"
```

It installs into `%USERPROFILE%\.cargo\bin`, with no Start menu entry (start the app with `gigapair-app`). Installing the MSI later fixes the connection.

SmartScreen may warn about unsigned binaries the first time: **More info → Run anyway**.

### macOS

Pick the DMG for the processor (`uname -m`: `arm64` → `gigapair-aarch64-apple-darwin.dmg`, `x86_64` → `gigapair-x86_64-apple-darwin.dmg`). Open it and drag **Gigapair** to Applications. The app is not notarized, so the first launch is blocked: open it once, then **System Settings → Privacy & Security → Open Anyway**, or run `xattr -dr com.apple.quarantine /Applications/Gigapair.app`.

The CLI is inside the app. To use it as `gigapair`:

```sh
sudo ln -s /Applications/Gigapair.app/Contents/MacOS/gigapair /usr/local/bin/gigapair
```

For a Mac used only from a terminal, the shell installer below works too.

### Linux

```sh
# Debian, Ubuntu:
curl -fLO "https://github.com/dopazo/gigapair/releases/download/$tag/gigapair-x86_64-unknown-linux-gnu.deb"
sudo apt install ./gigapair-x86_64-unknown-linux-gnu.deb

# Fedora, RHEL, openSUSE:
curl -fLO "https://github.com/dopazo/gigapair/releases/download/$tag/gigapair-x86_64-unknown-linux-gnu.rpm"
sudo dnf install ./gigapair-x86_64-unknown-linux-gnu.rpm

# Any distribution, per user, no sudo (also macOS):
curl --proto '=https' --tlsv1.2 -LsSf "https://github.com/dopazo/gigapair/releases/download/$tag/gigapair-installer.sh" | sh
```

The `./` makes apt/dnf install that file instead of searching a repository.

## Check the install

A shell opened before the install doesn't see the new `PATH`; use the full path or a new shell.

| Installed with | Full path |
| --- | --- |
| MSI | `C:\Program Files\gigapair\bin\gigapair.exe` |
| DMG | `/Applications/Gigapair.app/Contents/MacOS/gigapair` |
| `.deb` / `.rpm` | `/usr/bin/gigapair` |
| Shell installer | `~/.cargo/bin/gigapair` |
| PowerShell installer | `%USERPROFILE%\.cargo\bin\gigapair.exe` |

```sh
gigapair --version
gigapair status       # starts the service and prints this device's summary
```

## Unattended receiving server

A machine that only receives (home server, NAS, render node). Linux fits best: on Windows and macOS the service starts only when its user logs in.

```sh
gigapair service install                # start the service at login, and now
loginctl enable-linger "$USER"          # Linux: keep it running with nobody logged in

# On the user's desktop: "Add contact" in the app (or `gigapair pair`), copy the code. Then here:
gigapair pair <code> --name "Workstation" --yes
# Show the user the four printed words to compare with their desktop.

gigapair contacts trust Workstation --auto-accept on
gigapair config set download_dir /srv/incoming
```

Auto-accept lets that contact write to this disk without asking: set it only because the user asked for an unattended receiver. Without a desktop session there is usually no keyring, and the node key goes to a file readable only by its user.

## Update and remove

**Update:** install the newer version the same way. Contacts, history and settings are kept. A service that was running keeps the old version in memory until it restarts; the first `gigapair` command (or app launch) after the update notices and restarts it, printing `Restarted the Gigapair service: it was still running vX ...`. Interrupted transfers resume.

If instead it fails with `... is older than this program ... and restarting it didn't help`, an old `gigapaird` is still installed (a partial copy, or two installs on the `PATH`): install Gigapair again, and check that `gigapair --version` and `gigapaird --version` match.

**Remove:** first `gigapair service uninstall` and `gigapair service stop` (or, if the user also wants their data gone, `gigapair reset`, which does both and deletes the data; ask first, it can't be undone), then:

| Installed with | Remove with |
| --- | --- |
| MSI | **Settings → Apps → Installed apps → Gigapair → Uninstall** |
| DMG | Drag `Gigapair.app` to the Trash; delete `/usr/local/bin/gigapair` if you linked it |
| `.deb` / `.rpm` | `sudo apt remove gigapair` / `sudo dnf remove gigapair` |
| Shell or PowerShell installer | Delete `gigapair`, `gpair`, `gigapaird` and `gigapair-app` from `~/.cargo/bin` (`%USERPROFILE%\.cargo\bin`) |

Uninstalling keeps contacts, settings, this device's identity and received files (see [Where data lives](#where-data-lives)), so a reinstall or an update keeps working. To delete them, run `gigapair reset` before uninstalling, only if the user asks; pairing again is needed afterwards. Received files are never deleted by Gigapair.

## Where data lives

| Data | Windows | macOS | Linux |
| --- | --- | --- | --- |
| Config (`config.toml`) | `%APPDATA%\gigapair\Gigapair\config\` | `~/Library/Application Support/dev.gigapair.Gigapair/` | `~/.config/gigapair/` |
| Database (contacts, queue, history) | `%LOCALAPPDATA%\gigapair\Gigapair\data\` | `~/Library/Application Support/dev.gigapair.Gigapair/` | `~/.local/share/gigapair/` |
| Logs | `%LOCALAPPDATA%\gigapair\Gigapair\data\logs\` | `~/Library/Logs/Gigapair/` | `~/.local/state/gigapair/` |
| Node private key | Credential Manager | Keychain | Secret Service (GNOME Keyring, KWallet), or a `0600` file without one |
| Received files | `%USERPROFILE%\Downloads\Gigapair\` | `~/Downloads/Gigapair/` | `~/Downloads/Gigapair/` |
| Unfinished downloads | `.gigapair-partial\` inside the destination folder | same | same |

With `GIGAPAIR_HOME` or `--home`, everything lives under that folder instead.
