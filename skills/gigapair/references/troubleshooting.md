# Troubleshooting Gigapair

Read the `error:` line first: most messages already say what to do, and those are not repeated here. This file covers the problems whose cause or fix is not obvious from the message.

## Contents

- [First checks](#first-checks)
- [Relay instead of direct, or slow](#relay-instead-of-direct-or-slow)
- [Transfers that don't move](#transfers-that-dont-move)
- [Service](#service)
- [Pairing](#pairing)
- [Install](#install)
- [Logs](#logs)
- [Still stuck](#still-stuck)

## First checks

```sh
gigapair --version          # the CLI exists, and which version
gigapair service status     # starts at login? running?
gigapair status             # device, relay, active transfers (starts the service if needed)
gigapair contacts           # is the other device online?
```

Both devices must have Gigapair running at the same time: nothing is stored in between. `offline` in `contacts` means the other device's service can't be reached right now; transfers wait and resume on their own when it can.

## Relay instead of direct, or slow

Each transfer shows `[direct]` or `[via relay (...)]` in `status` and `history`. Direct is the normal case. A relay only forwards encrypted data (it can't read the files, but sees both IP addresses, timing and volume) and is usually much slower, so a relayed transfer is worth fixing.

**Same place, different networks.** The most common cause when both devices are at the same site: they are connected to the same router but on networks that the router keeps apart. For example, one on the regular Wi-Fi and the other on its 5 GHz network ("Home" and "Home-5G"), one on the guest network, or one on Wi-Fi and the other on cable. Gigapair notices this and prints under the transfer:

```text
You and <name> seem to be behind the same router, but a direct connection could not be made. Make sure both devices are on the same network (the same Wi-Fi name, or both on cable) and send again.
```

Fix: connect both devices to the same network name, then cancel and send again (or `retry` it).

Other causes:

| Situation | Fix |
| --- | --- |
| Windows receiver installed without the MSI | No firewall exception, so contacts can't connect in directly. Install the MSI (see install.md) |
| Devices in different places, one behind CGNAT (common on mobile and some fiber connections) or a strict corporate firewall | Often unavoidable; the relay is the fallback. The user may run their own relay: `gigapair config set relay_url https://relay.example.com`, then restart the service |
| The user wants data never to go through a relay | `gigapair config set direct_only on`. Transfers that can't connect directly then fail with `No direct connection to <name> (direct only is on)` instead of relaying |
| Slow even with `[direct]` | Check the sender's `gigapair config get upload_limit_kib` (empty means unlimited), and the disks on both sides |
| Several incoming transfers, only one moves | By design: `max_parallel_downloads` is 1 by default (1–8) |
| The computer sleeps mid-transfer | `gigapair config get keep_awake` should be `true` |

## Transfers that don't move

| Situation | Cause and fix |
| --- | --- |
| Sender stays in `Waiting for acceptance` | The receiver hasn't answered. On the receiver: `gigapair requests`, then `accept` or `reject`; or turn on auto-accept there for this sender |
| `... Free up space or choose another folder` on accept | `gigapair accept <id> --to <folder on a disk with room>` |
| `this device is not paired with you; pair again to send files` | The other device removed this contact. Pair again |
| Note `Paused by <name>` | The other side paused it; it continues when they resume |
| Nothing received after a reboot (Windows, macOS) | The service starts at login, not at boot. Log in that user |

Interrupted transfers (a network cut, a restart, a reboot) resume by themselves where they stopped; verified data is not sent again.

## Service

| Message or symptom | Cause and fix |
| --- | --- |
| `the running Gigapair service (vX) is not compatible with this client; ...` | Gigapair was updated but the old service is still running. `gigapair service stop`, then `gigapair service start`. If `stop` fails with the same error, end the old process (`pkill -x gigapaird`, or `Stop-Process -Name gigapaird` in PowerShell), then `gigapair service start` |
| `the service did not start; check the log folder` | `gigapaird` exited at startup. Read the newest `gigapaird.*.log` (see [Logs](#logs)) |
| ``could not install the service: `systemctl ...` failed: ...`` | Linux without a systemd user manager for this user (a container, WSL without systemd, a `su` session). Log in as that user normally, or `loginctl enable-linger $USER` |
| `gigapaird was not found next to gigapair; ...` | The four programs aren't in the same folder. Reinstall |
| `<path>/config.toml is not valid: ...` | A hand edit broke the file. Fix the TOML; deleting the file resets settings but keeps contacts |

## Pairing

| Message or symptom | Cause and fix |
| --- | --- |
| `... Make sure Gigapair is running on the other device and the code is less than 10 minutes old` | The device that made the code can't be reached: its `pair` command or Add contact dialog was closed, the code expired, or a network blocks it. Keep it open on that device and use a fresh code |
| The four words differ between the devices | The code may have been intercepted, or the wrong code was used. Don't confirm; if already confirmed, `gigapair contacts remove <contact>` and pair again, sending the code by a channel the user trusts |
| `this pairing code was made by a newer version of Gigapair; ...` | Update this device |

## Install

| Symptom | Fix |
| --- | --- |
| `gigapair: command not found` right after installing | Open a new shell, or use the full path (install.md, "Check the install") |
| macOS: "Gigapair can't be opened" or "Apple could not verify..." | Not notarized: **System Settings → Privacy & Security → Open Anyway**, or `xattr -dr com.apple.quarantine /Applications/Gigapair.app` |
| Linux: `GLIBC_2.xx not found` | The distribution is older than glibc 2.35; there is no build for it yet |
| Linux: the app window doesn't open but the CLI works | A missing Vulkan driver, `libxkbcommon` or `fontconfig`. `gigapair-app --verbose` prints the error |
| Linux: no tray icon on GNOME | Install the "AppIndicator and KStatusNotifierItem Support" extension |

## Logs

`gigapair logs` prints the logs folder and its newest files: `gigapaird.YYYY-MM-DD.log` (service) and `gigapair-app.YYYY-MM-DD.log` (app), kept 7 days, with crashes and their backtraces. They never contain keys or pairing codes, so they are safe to attach to a bug report.

For more detail, run the service in the foreground with a higher log level, then go back to normal:

```sh
gigapair service stop
GIGAPAIR_LOG=debug gigapaird --verbose     # PowerShell: $env:GIGAPAIR_LOG = "debug"; gigapaird --verbose
# reproduce the problem, then Ctrl+C
gigapair service start
```

## Still stuck

Check the project's repository, https://github.com/dopazo/gigapair: its README and `docs/usage.md` for current behaviour, and its issues (https://github.com/dopazo/gigapair/issues) for known problems. If nothing matches, help the user open an issue with the version (`gigapair --version`), the OS, the exact error line and the relevant log lines.
