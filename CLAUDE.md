# CLAUDE.md

Guidance for Claude Code when working in this repository.

## Project

Gigapair is a peer-to-peer app for moving very large files (50–200+ GB) between paired devices: end-to-end encrypted, BLAKE3-verified and resumable. Rust workspace, GPUI desktop UI, CLI, and a background service.

- Full product spec: `spec_gigapair.md` (in Spanish). It is the source of truth for scope, requirements (RF-xx / RNF-xx) and stage plan. Cite requirement IDs in commits and PRs when relevant.
- Current stage: stages 1–3 implemented (core, service, CLI, GPUI app, tray); the stage 1 real-network acceptance test (50 GB Windows↔macOS with two cuts) is pending a manual run. Stage 4 (distribution): minimal release pipeline with dist (see `docs/releasing.md`); DMG/AppImage/`.deb`, other channels, the system-wide service and the update notice come later.

## Architecture

```
gigapair-app (GPUI) ─┐
                     ├─ local IPC (interprocess: Unix socket / named pipe) ─► gigapaird ─► gigapair-core ─► iroh / iroh-blobs
gigapair-cli (clap) ─┘
```

| Crate | Role | Rules |
| --- | --- | --- |
| `crates/gigapair-core` | Transfer logic, protocol, queue, persistence, path handling | No UI deps. No GPUI. Must be testable in-process |
| `crates/gigapaird` | Tokio service; owns the iroh node, queue, state, retries, events to clients; installs itself as a system service | The only process that touches the network and the DB |
| `crates/gigapair-cli` | `gigapair` binary (alias `gpair`) | Thin client: IPC only, no business logic |
| `crates/gigapair-app` | GPUI client; starts the service if not running | Thin client: IPC only, no business logic |
| `crates/gigapair` | The executables (`gigapair-app`, `gigapair`, `gpair`, `gigapaird`), each a few lines calling its crate's `main()` | The only package dist releases; keep logic in the library crates |

Invariants:

- UI and CLI must reach feature parity (RF-10). Add capabilities to the service/IPC API first, then expose them in both clients.
- Every peer↔peer and client↔service connection exchanges a **protocol version**. Incompatible versions produce a clear error up front, never a mid-transfer failure.
- Closing the UI must never stop a transfer.

## Non-negotiables

These come from the spec and are easy to break by accident:

- **Streaming only (RNF-02).** Never load a whole file into memory. Assume files of 200 GB.
- **No double disk usage (RNF-12).** Receive into `.gigapair-partial/` inside the destination folder (same filesystem), then atomic `rename` to the final name. Never copy at the end. If iroh-blobs can't export without copying, write files directly with a custom store.
- **Integrity (RNF-06).** A file is never marked complete unless the BLAKE3 root hash matches.
- **Never overwrite.** Name clashes become `name (1).ext`.
- **Symlinks** are skipped with a warning, not followed. Empty folders and mtimes are preserved. Sanitize names invalid on the receiver (e.g. `:` on Windows) and support long paths on Windows (`\\?\`).
- **Only paired contacts** can send. Default is manual accept; auto-accept is a per-contact opt-in. Check free space before accepting (RF-09).
- **Keep-awake** guard is held only while transfers are active, and released when the last one ends.
- **Relay transparency.** Every transfer exposes `direct` vs `relay` (with relay name) in the UI, `gigapair status` and history. Honor the "direct only" setting.
- **No telemetry.** No network traffic except to the contact and the relay. Logs are local only.
- **Secrets.** Node private key in the OS keyring (`keyring` crate); fallback to a `0600` file only in headless/server mode. Never log keys or pairing codes.
- **Persisted state (RNF-07).** Queue and transfer state live in SQLite and must survive process kill and reboot.

## Stack

Pin versions for anything pre-1.0 (GPUI, gpui-component, iroh, iroh-blobs) and upgrade deliberately; their APIs change often. Check current docs (context7) before writing code against iroh, iroh-blobs or GPUI rather than relying on memory.

| Concern | Crate |
| --- | --- |
| Async runtime | tokio |
| Network / identity | iroh (QUIC, Ed25519 node ID, hole punching, relay) |
| Transfer | own block protocol on iroh streams (see `docs/transfer-protocol.md`); iroh-blobs was evaluated and not used |
| UI | gpui-kit `=0.9.1` (re-exports gpui-pre 0.3.12 + gpui-component) |
| CLI | clap (derive) |
| IPC | interprocess |
| DB | rusqlite with `bundled` |
| Paths | directories |
| Keys | keyring |
| Sleep inhibit | keepawake |
| Notifications / tray | notify-rust; tray: tray-icon (Windows, macOS), ksni (Linux, StatusNotifierItem over D-Bus, no GTK) |
| Release | dist (cargo-dist) `0.34.0`: `dist-workspace.toml`, `.github/workflows/release.yml` (generated: edit the toml and run `dist generate`) |

Paths come from `directories` (qualifier `dev`, org `gigapair`, app `Gigapair`). Never store anything next to the executable.

## Commands

```sh
cargo build                      # build the workspace
cargo test --workspace           # all tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo run --bin gigapaird        # run the service in the foreground
cargo run --bin gigapair -- <args>
cargo run -p gigapair            # the desktop app (default-run)
dist plan                        # dry run of the release (cargo install cargo-dist --version 0.34.0 --locked)
```

Run `cargo fmt` and `cargo clippy` before considering a change done.

## Testing

- **Unit** (in `gigapair-core`): queue, persisted state, name sanitization and path handling. Cover Windows-invalid names and collision renaming.
- **Integration:** two nodes in the same process transferring generated multi-GB files and checking the hash. Generate test data on the fly; never commit large files.
- **Fault injection:** drop the connection, kill and restart the service mid-transfer; it must resume without corruption or re-sending verified blocks.
- CI runs on Windows, macOS and Linux for every PR. Don't write platform-specific code without `cfg` gating and a test on each target.

## Conventions

- **Language.** Code, comments, UI strings, CLI output, errors, README and docs are in **English**. Keep user-facing strings centralized so they can be translated later. The spec is in Spanish; translate its UI text, don't copy it.
- **Errors:** `thiserror` for library errors in `gigapair-core`, `anyhow` at binary edges. Error messages tell the user what to do next.
- **Logging:** `tracing`. No secrets, no full file contents. Both `gigapaird` and `gigapair-app` log to daily files via `gigapair_core::logging::init`; log state changes and errors that a bug report would need, not every progress tick.
- No `unwrap()`/`expect()` in non-test code paths that can fail at runtime.
- License: MIT (see `LICENSE`). New dependencies must be MIT-compatible (e.g. MIT, Apache-2.0, BSD); avoid copyleft (GPL, AGPL).

## Out of scope for v1

Don't build these unless asked: one-to-many/swarm transfers, cloud storage or store-and-forward, two-way folder sync, compression/transcoding, mobile apps, auto-update (only "new release available" notice), code signing.
Dont try to search for >200 gb files unless asked, you will test it when the time comes.

## Code map

- `gigapair-core`: `paths` (sanitizing, collisions, no-clobber rename, free space), `manifest` (scan + BLAKE3 hashing, 4 MiB blocks), `pairing` (codes, fingerprint), `wire` (peer protocol), `api` + `ipc` (client↔service), `db` (SQLite), `config`, `keys`, `dirs` (`GIGAPAIR_HOME`), `logging` (daily log files + panic hook, shared by the service and the app; level via `GIGAPAIR_LOG`), `service` (user-level autostart), `engine/` (`net`, `pair`, `send`, `recv`, `api`).
- `gigapaird`: IPC server, keep-awake thread (`awake.rs`), notifications (`notify.rs`), instance lock.
- `gigapair-cli`: `lib.rs` (commands), `output.rs`, `strings.rs` (all user text).
- `gigapair-app`: `shell.rs` (owns the service connection, tray and window lifecycle; quit mode is explicit), `backend.rs` (IPC on a Tokio thread), `app.rs` (the window), `tray/` (per-OS tray + `summarize`), `single.rs` (one app per user: a second launch shows the window), `icon.rs` (placeholder icon), `qr.rs`, `strings.rs`.
- `gigapair`: `src/bin/*.rs` (one wrapper per executable; `windows_subsystem` lives here), `wix/main.wxs` (MSI, hand-edited Start menu shortcut).
- Tests: `crates/gigapair-core/tests/transfer.rs` runs two engines over loopback (`NetworkMode::LocalOnly`), including restarts mid-transfer. `throughput` is `#[ignore]`d; size via `GIGAPAIR_TEST_MB`.

## Known gaps / next steps

1. Manual acceptance test of stage 1 across two real networks (one behind CGNAT) with relays; check `direct`/`relay` labelling and the first-relay notice.
2. Tray (RF-14): implemented in `gigapair-app` (`shell.rs`, `tray/`, `single.rs`). Verified on Linux only; the Windows code is cross-checked (`cargo check --target x86_64-pc-windows-gnu`) but not run, macOS is compiled in CI but untested. The icon is a placeholder drawn in `icon.rs` until a real app icon exists (also needed for the window and installers). Pending: Windows/macOS manual test, the stage 3 reboot test, real icon.
3. Distribution follow-ups: DMG/`.app` bundle, AppImage, `.deb`, winget/Scoop/Homebrew, crates.io; real icon; system-wide service variants (Windows Service, Linux system unit with dedicated user, `/etc/gigapair`); "new release available" notice (must respect no-telemetry: opt-in or clearly explained). The release pipeline has never run: the first tag is its test.
4. Relay: the first-use notice is emitted once per install; the app shows it as a banner, the CLI prints it during `send`.
5. No LAN discovery (iroh 1.4 has no mDNS); contacts rely on n0 DNS (relay URL only) plus stored addresses and a stable UDP port.
6. Ticket sends to non-contacts (RF-16) not implemented.
7. UI strings live in `strings.rs` per client; no i18n framework yet.
