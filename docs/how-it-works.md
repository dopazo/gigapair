# How Gigapair works

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

Gigapair is built on [iroh](https://github.com/n0-computer/iroh) for connectivity (QUIC, hole punching, relays). Transfers use a small protocol of its own on top of iroh: files are split into 4 MiB blocks, each checked against a BLAKE3 block list that the offer pins with a hash, and the whole file is checked against its BLAKE3 root hash (the same value `b3sum` prints) before it is moved into place. See [transfer-protocol.md](transfer-protocol.md). The UI uses [GPUI](https://www.gpui.rs/) with [GPUI Kit](https://gpui-kit.com).

## Privacy and relays

- **No telemetry.** Nothing leaves your computer except the connection to your contact and, when needed, the relay. Logs stay local; you can export them to report a bug.
- **Relays can't read your files.** When a direct connection isn't possible, traffic goes through a relay (by default, the public relays run by n0). A relay only forwards end-to-end encrypted data. It **can** see both devices' IP addresses, when a transfer happens and how much data is sent.
- **You stay in control.** The first time a relay is used, Gigapair tells you. In settings you can use your own relay or enable **direct only**, which fails instead of using an intermediary. History records whether each transfer was direct or relayed.

## File handling

- Existing files are never overwritten. A name clash becomes `name (1).ext`.
- Folder structure (including empty folders) and modification times are preserved.
- Symbolic links are skipped with a warning.
- File names that are invalid on the receiving system (for example `:` on Windows) are adjusted. Long paths are supported on Windows.
