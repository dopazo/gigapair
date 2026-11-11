# Transfer protocol and storage

This document describes how Gigapair moves data between two devices and why it
does not use iroh-blobs (spec risk #4).

## Decision: our own block store on top of iroh

The spec asked stage 1 to check whether iroh-blobs could keep in-progress data
in `.gigapair-partial/` on the destination disk and finish with a rename
instead of a copy (RNF-12). iroh-blobs (0.104) keeps blob data inside its own
store directory, managed by its own database, and exports by copying out of
that store or by handing the file over to the store's management. Getting
"write next to the destination, rename at the end" would mean one store per
destination folder, plus tracking which store owns which partial file. Its API
also changes often (it is pre-1.0).

The CLAUDE.md fallback applies: write files directly. Gigapair uses iroh only
for connections (QUIC with TLS 1.3, Ed25519 identities, hole punching,
relays) and implements a small transfer protocol on top. This gives:

- **No double disk usage.** Data is written into
  `<destination>/.gigapair-partial/<transfer id>/<n>.part` and moved into place
  with a rename that refuses to replace an existing file (`renameat2` with
  `RENAME_NOREPLACE` on Linux, `renamex_np` with `RENAME_EXCL` on macOS,
  `MoveFileExW` without `MOVEFILE_REPLACE_EXISTING` on Windows). Nothing is
  ever copied.
- **Full control of resume and integrity**, described below.
- **Fewer moving parts** to keep in step with upstream.

## Integrity (RF-06, RNF-06)

The sender hashes each file once, streaming it from disk (the "Preparing"
state):

- the **root hash**: plain BLAKE3 of the whole file (the same value `b3sum`
  prints);
- the **block list**: BLAKE3 of every 4 MiB block, concatenated (32 bytes per
  block, so 1.6 MB for a 200 GB file).

The offer carries, for every file, its size, modification time, root hash and
the BLAKE3 of the block list. The receiver:

1. downloads the block list and checks it against the hash in the offer;
2. checks every block against the list as it arrives, before writing it;
3. re-hashes the finished file and compares it with the root hash
   ("Verifying"). Only then is the file moved to its final name and marked
   complete.

A block that fails its check is downloaded again (up to three attempts in a
row before the transfer is marked failed).

## Resume (RF-05, RNF-07)

The receiver pulls. For each file it asks for data from its first missing
block to the end. About once a second it calls `fsync` on the partial file and
records the number of verified blocks in SQLite (WAL, `synchronous = FULL`).

After a network drop, a killed service or a reboot, the receiver starts from
the checkpoint. It also re-reads the blocks already on disk past the
checkpoint and keeps the ones that match the block list, so a crash between
checkpoints doesn't cost a resend either. Either way, verified blocks are
never sent again.

The sender checks that each source file still has the size and modification
time it had when it was offered; if not, the transfer fails with a message
asking to send it again.

## Wire format

- ALPN `gigapair/1`; every request uses its own bidirectional QUIC stream.
- Messages are length-prefixed (`u32`, big endian) JSON envelopes
  `{"v": <protocol version>, "body": ...}`. A version mismatch is answered with
  `incompatible_version` before anything else, so mismatched versions fail
  with a clear message instead of mid-transfer.
- Requests: `pair`, `ping`, `unpair`, `offer`, `decision`, `notice` (paused,
  resumed, cancelled, completed, failed), `get_hashes` (answered with the raw
  block list) and `get` (answered with the raw file bytes from a block to the
  end of the file).
- Only paired contacts may use anything but `pair` and `unpair`; everything
  else is answered with `not_paired`.

## Pairing (RF-01)

A pairing code (`gp1-…`, base32) carries the creator's endpoint id, its relay
URL (or direct addresses when no relay is available) and a 16-byte one-time
secret that expires after 10 minutes. The device that pastes the code connects
and presents the secret. Both devices then show four words derived from both
ids and the secret; if they match, the code wasn't swapped in transit. Each
side saves the contact only after its user confirms.

## Finding contacts again

- With discovery on (default), iroh publishes **only the relay URL** of this
  device to n0's DNS (pkarr), never its IP addresses, so contacts can find it
  after a network change.
- Each contact's last known addresses are stored locally and refreshed from
  pings (every 45 s) and incoming connections.
- The service reuses its UDP port across restarts, so stored direct addresses
  stay valid on a LAN even without relays.
