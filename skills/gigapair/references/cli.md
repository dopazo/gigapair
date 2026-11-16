# Gigapair CLI reference

`gigapair` and `gpair` are the same program. Every command accepts `--help`, and `--home <dir>` (see [Two instances on one machine](#two-instances-on-one-machine)).

## Contents

- [Commands](#commands)
- [Naming contacts and transfers](#naming-contacts-and-transfers)
- [Reading the output](#reading-the-output)
- [Transfer states](#transfer-states)
- [Patterns for agents](#patterns-for-agents)
- [Settings](#settings)
- [Two instances on one machine](#two-instances-on-one-machine)

## Commands

| Command | What it does |
| --- | --- |
| `gigapair pair [--name <name>] [--yes]` | Show this device's pairing code (text and QR) and wait up to 10 minutes for the other device to use it |
| `gigapair pair <code> [--name <name>] [--yes]` | Add a contact from the other device's code |
| `gigapair contacts [list]` | List contacts: name, online or offline, auto-accept, node id |
| `gigapair contacts rename <contact> <name>` | Rename a contact |
| `gigapair contacts remove <contact>` | Remove a contact: its transfers are cancelled and the other device is told to forget this one |
| `gigapair contacts trust <contact> --auto-accept on\|off` | Accept this contact's transfers without asking |
| `gigapair send <contact> <paths...> [--no-wait]` | Send files and folders. Without `--no-wait`, shows progress until the transfer ends |
| `gigapair requests` | Incoming transfers waiting for an answer |
| `gigapair accept <id> [--to <folder>]` | Accept, into the download folder or `--to` another one (created if missing) |
| `gigapair reject <id>` | Reject a request |
| `gigapair status [--watch]` | Device summary plus active transfers. `--watch` redraws until Ctrl+C (don't use it from an agent) |
| `gigapair history [--limit <n>] [--clear]` | Finished transfers, newest first (50 by default). `--clear` deletes them |
| `gigapair pause <id>` / `resume <id>` | Pause or resume an active transfer |
| `gigapair cancel <id>` | Cancel a transfer; the other side is told |
| `gigapair retry <id>` | Send a failed or cancelled transfer again, as a new transfer with a new id. Sender side only |
| `gigapair config get [<key>]` | Show one setting, or all |
| `gigapair config set <key> <value>` | Change a setting; `""` resets an optional one |
| `gigapair service install` / `uninstall` | Start the service at login (and now) / stop doing so |
| `gigapair service start` / `stop` / `status` | Start, stop or check the service |
| `gigapair logs` | The logs folder and its newest files |
| `gigapair reset [--yes]` | Delete everything Gigapair keeps on this device: contacts, identity, history, settings, logs and the start at login; contacts are told to forget it. Received files stay. Irreversible: every contact must pair again. Without a terminal it needs `--yes` |
| `gigapair --version` | The version |

`service` and `logs` work without a running service; every other command connects to it, starting it if needed.

`pair` details:

- `--name` names the new contact. Without it, an interactive terminal asks; otherwise the other device's own name is used.
- `--yes` skips the "Does the other device show the same words? [y/N]" question. Without a terminal and without `--yes`, `pair` fails with `can't ask for confirmation without a terminal; pass --yes after checking the words`. See rule 4 in SKILL.md before using it.
- Codes look like `gp1-abcd-efgh-…`. Case, spaces and missing dashes are tolerated. A code works once and expires after 10 minutes.

`send` details:

- Relative paths are resolved against the current folder. Symlinks inside folders are skipped with a `warning:` on stderr; empty folders are kept.
- The first line names the transfer id: `Sending <label> (<size>) to <contact> [<id>]`.
- Without `--no-wait` it exits 0 when the transfer completes and 1 with the reason if it fails, is rejected or is cancelled. Ctrl+C only stops watching; the transfer goes on.

## Naming contacts and transfers

A **contact** argument matches, in order: the exact node id or the name in any case; otherwise a unique match on the start of the name or the first 4+ characters of the node id. `studio` finds "Studio Mac". If several contacts match, the error lists them with the start of their node ids, and nothing is sent.

Names are unique (ignoring case): pairing with a second device of the same name stores it as `name (2)`, and `contacts rename` refuses a name another contact already has.

A **transfer id** is hex. `status`, `requests` and `history` print the first 8 characters; any prefix that matches only one transfer works.

## Reading the output

`gigapair contacts`:

```text
NAME        STATUS    AUTO-ACCEPT  NODE ID
Studio Mac  online    off          8f3c…
NAS         offline   on           b21d…
```

`status`, `requests` and `history` print one line per transfer:

```text
633efa0e  → Studio Mac  Season 2 raw footage/  Transferring  620.0 MB/3.2 GB (19.4%)  6.1 MB/s  ETA 6m 58s  [direct]
5a861ebe  ← NAS  photo-library-2025.zip  Awaiting your answer  0 B/860.0 MB (0.0%)
```

Fields: short id, direction (`→` sending to, `←` receiving from) and contact, label (the first item; a folder ends in `/`, and several items read like `cut-a.mov + 2 more`), state, bytes done/total (percent), then speed, ETA and connection when known. The connection is `[direct]`, `[via relay (<relay>)]` or `[via relay]`. Indented lines below a transfer carry a note, the error of a failed one, or a hint that both devices look to be on the same network.

`gigapair status` starts with a summary:

```text
Device:      workstation
Node id:     8f3c…
Relay:       https://… (direct only)
Downloads:   /home/me/Downloads/Gigapair
Keep awake:  on, preventing sleep now
Requests:    2 waiting (`gigapair requests`)
```

`(direct only)` and the `Requests:` line appear only when they apply. `No active transfers.` means nothing is running.

`gigapair service status`:

```text
Starts at login: yes (/home/me/.config/systemd/user/gigapaird.service)
Running:         yes
```

## Transfer states

| State (as printed) | Meaning | Where it shows |
| --- | --- | --- |
| Preparing | The sender is scanning and hashing the files | `status` |
| Waiting for acceptance | Sent; the receiver hasn't answered yet | `status` (sender) |
| Awaiting your answer | Incoming; needs `accept` or `reject` | `status`, `requests` (receiver) |
| Queued | Accepted, waiting for a free download slot (`max_parallel_downloads`) | `status` |
| Transferring | Data is moving | `status` |
| Paused | Paused on this device; `resume` continues. A pause from the other device shows as the note `Paused by <name>` | `status` |
| Verifying | Final hash check | `status` |
| Completed | Every file matched its BLAKE3 hash and is in place | `history` |
| Rejected / Cancelled / Failed | Ended without completing; the error line says why | `history` |
## Patterns for agents

**Send and check the outcome.**

```sh
gigapair send nas /data/export --no-wait   # note the id, e.g. 633efa0e
gigapair status                            # repeat now and then; the line shows progress and ETA
gigapair history --limit 10                # once 633efa0e is no longer in status
```

Pick the polling interval from the ETA; a large transfer needs no checks every few seconds. A transfer stuck in `Waiting for acceptance` needs the human on the other device (or auto-accept there).

**Receive into a given folder.** `gigapair requests`, then `gigapair accept <id> --to <absolute or relative folder>`. Free space is checked before accepting; files land in `.gigapair-partial/` inside the destination and are renamed into place when verified. Existing files are never overwritten: clashes become `name (1).ext`.

**Wait for completion in one blocking command** (only when the user wants that and the transfer is small): `gigapair send <contact> <paths>` without `--no-wait`; its exit code tells success from failure. Its output is a single line rewritten with carriage returns, so read only the last part.

## Settings

`gigapair config get` lists them; `(not set)` means the default applies. Booleans take `on`/`off` (also `true`/`false`, `yes`/`no`, `1`/`0`).

| Key | Values | Default | Notes |
| --- | --- | --- | --- |
| `device_name` | text | the host name | The name contacts see |
| `download_dir` | absolute path | `Downloads/Gigapair` | Created if missing |
| `keep_awake` | on/off | on | Keep the computer awake while transfers run |
| `notifications` | on/off | on | Desktop notifications |
| `close_to_tray` | on/off | on | Desktop app only |
| `theme` | `system`, `light`, `dark` | `system` | Desktop app only |
| `direct_only` | on/off | off | File data never goes through a relay; such transfers wait or fail instead |
| `relay_url` | full URL, e.g. `https://relay.example.com` | public relays | Restart the service to apply |
| `discovery` | on/off | on | Publish this device's relay URL (not its IP) so contacts find it after a network change. Restart to apply |
| `upload_limit_kib` | KiB/s, `0` for none | none | `20000` ≈ 20 MB/s |
| `max_parallel_downloads` | 1–8 | 1 | Incoming transfers downloading at once. Lowering it needs a restart |
| `key_storage` | `auto`, `keyring`, `file` | `auto` | Where the node key lives. Restart to apply |

Restart the service with `gigapair service stop` then `gigapair service start`. Transfers resume afterwards.

## Two instances on one machine

`--home <dir>` (or the `GIGAPAIR_HOME` environment variable) keeps all data, the socket and the logs under one folder, so a second instance doesn't touch the first:

```sh
gigapair --home /tmp/gp-test pair
```

Pass the same `--home` to every command for that instance. `gigapaird --home <dir>` runs its service in the foreground.
