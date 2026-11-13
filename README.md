<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/images/logo-dark.svg">
    <img src="docs/images/logo-light.svg" alt="Gigapair" width="300">
  </picture>
</p>

<p align="center"><b>Move huge files between the devices you trust. Pair once, send anytime.</b></p>

Gigapair is a desktop app and CLI for sending very large files (tens to hundreds of GB, any type) directly between two computers: peer-to-peer, end-to-end encrypted, verified and resumable. No cloud storage, no size limits, no new code for every transfer.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/images/window-dark.png">
    <img src="docs/images/window-light.png" alt="The Gigapair window: a folder being sent to a contact, with its strip of verified blocks, and an incoming request waiting for an answer" width="820">
  </picture>
</p>

*Originally built to send talk recordings between the organizers of [builderia.lat](https://builderia.lat).*

> **Status:** beta. Windows, macOS and Linux builds are on the [releases page](https://github.com/dopazo/gigapair/releases); the first real-network acceptance tests are still in progress.

## Why Gigapair

- **Survives interruptions.** If the network drops, the app closes or a computer reboots, the transfer resumes where it stopped. Blocks already verified are never sent again.
- **Every byte checked.** Each 4 MiB block is verified with BLAKE3 as it arrives, and a file only gets its final name once its root hash matches.
- **No double disk space.** Data is written next to its destination and renamed into place: a 200 GB file needs 200 GB free, not 400 GB.
- **Doesn't stall when a computer sleeps.** Gigapair keeps the system awake only while a transfer runs.
- **Contacts, not codes.** Pair two devices once with a code or QR and a 4-word check, then drop files onto the contact. A headless server can receive on its own with auto-accept.
- **Works behind NAT and CGNAT.** Direct connection with hole punching, and an encrypted relay as a fallback. Every transfer shows whether it is **direct** or **via relay**, and which relay.

## Privacy

No telemetry, and no network traffic except to your contact and, when needed, a relay. A relay only forwards end-to-end encrypted data; it can see both IP addresses and how much is sent. You can run your own relay or allow **direct connections only**. Details in [How it works](docs/how-it-works.md#privacy-and-relays).

## Install

Download from the [latest release](https://github.com/dopazo/gigapair/releases/latest). Every package contains the desktop app (`gigapair-app`), the CLI (`gigapair`, alias `gpair`) and the background service (`gigapaird`).

| Platform | How |
| --- | --- |
| Windows 10/11 | `gigapair-x86_64-pc-windows-msvc.msi`: adds **Gigapair** to the Start menu, the CLI to the `PATH`, and a firewall exception for the service |
| macOS (Apple silicon and Intel) | `curl --proto '=https' --tlsv1.2 -LsSf https://github.com/dopazo/gigapair/releases/latest/download/gigapair-installer.sh \| sh`, then run `gigapair-app` |
| Linux (x86_64, glibc 2.35+) | Same shell installer, then run `gigapair-app` |

The binaries are not code-signed yet. On Windows, SmartScreen asks once: **More info → Run anyway**. On macOS: **System Settings → Privacy & Security → Open Anyway**. Every release ships SHA-256 checksums and [build attestations](https://docs.github.com/en/actions/security-for-github-actions/using-artifact-attestations) (`gh attestation verify <file> --repo dopazo/gigapair`).

## Use

**Desktop app**

1. **Pair:** click **Add contact** and send the code (or the QR) to the other person by any channel; they paste it under **Add contact**. Check that both screens show the same four words.
2. **Send:** drop files or folders onto the contact.
3. **Receive:** accept the request, or turn on **Accept transfers from this contact automatically**.

Closing the window keeps Gigapair in the tray, and transfers keep going in the background service.

**CLI**

The CLI does everything the app does, so a machine that only receives (a home server, a NAS, a render node) needs no desktop at all: pair it once, turn on auto-accept, and it takes files on its own. It also suits scripts and AI agents that move files for you: commands can run without prompts (`pair --yes`, `send --no-wait`), contacts and transfers accept short names, and `status` and `history` report what happened.

```sh
gigapair pair                         # show a pairing code (and a QR)
gigapair pair <code>                  # add a contact from the other device's code
gigapair send <contact> <paths...>    # send files or folders
gigapair status --watch               # progress, speed, direct or relay
```

All commands, headless servers, settings, where data is stored and logs: [docs/usage.md](docs/usage.md).

## More

- [How it works](docs/how-it-works.md): the service, the transfer steps, relays and file handling.
- [Transfer protocol](docs/transfer-protocol.md).
- [Building from source and tests](docs/development.md).
- [Releasing](docs/releasing.md).

Built on [iroh](https://github.com/n0-computer/iroh) (QUIC, hole punching, relays) and [GPUI](https://www.gpui.rs/) with [GPUI Kit](https://gpui-kit.com).

## License

[MIT](LICENSE). The bundled Archivo font is under the SIL Open Font License ([details](crates/gigapair-app/assets/fonts/README.md)).
