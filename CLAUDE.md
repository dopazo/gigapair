# CLAUDE.md

Guidance for Claude Code when working in this repository.

## Project

Gigapair is a peer-to-peer app for moving very large files (50–200+ GB) between paired devices: end-to-end encrypted, BLAKE3-verified and resumable. Rust workspace, GPUI desktop UI, CLI, and a background service.

- Full product spec: `spec_gigapair.md` (in Spanish). It is the source of truth for scope, requirements (RF-xx / RNF-xx) and stage plan. Cite requirement IDs in commits and PRs when relevant.
- Current stage: **pre-code**. Stage 1 is `gigapair-core` + `gigapaird` + CLI. Don't start UI work before stage 1 passes its acceptance test unless asked.

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
| Transfer | iroh-blobs (collections, BLAKE3 verified streaming, resume) |
| UI | gpui + gpui-component (pinned rev) |
| CLI | clap (derive) |
| IPC | interprocess |
| DB | rusqlite with `bundled` |
| Paths | directories |
| Keys | keyring |
| Sleep inhibit | keepawake |
| Notifications / tray | notify-rust, tray-icon |
| Release | dist (cargo-dist), release-plz |

Paths come from `directories` (qualifier `dev`, org `gigapair`, app `Gigapair`). Never store anything next to the executable.

## Commands

```sh
cargo build                      # build the workspace
cargo test --workspace           # all tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo run -p gigapaird           # run the service in the foreground
cargo run -p gigapair-cli -- <args>
cargo run -p gigapair-app
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
- **Logging:** `tracing`. No secrets, no full file contents.
- No `unwrap()`/`expect()` in non-test code paths that can fail at runtime.
- License: MIT (see `LICENSE`). New dependencies must be MIT-compatible (e.g. MIT, Apache-2.0, BSD); avoid copyleft (GPL, AGPL).

## Out of scope for v1

Don't build these unless asked: one-to-many/swarm transfers, cloud storage or store-and-forward, two-way folder sync, compression/transcoding, mobile apps, auto-update (only "new release available" notice), code signing.
