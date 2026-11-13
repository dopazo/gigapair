# Development

## Building from source

Requires Rust 1.95 or newer ([rustup](https://rustup.rs/)).

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

## Trying it on one computer

Run two instances with separate data folders and pair them with each other:

```sh
target/release/gigapair --home ./demo-a pair            # shows a code, waits
target/release/gigapair --home ./demo-b pair <code>     # in a second terminal
target/release/gigapair --home ./demo-a send <name-of-b> <some file>
target/release/gigapair --home ./demo-b requests
target/release/gigapair --home ./demo-b accept <id>
```

For the app, set `GIGAPAIR_HOME` before launching it (PowerShell: `$env:GIGAPAIR_HOME = "C:\demo-a"; .\target\release\gigapair-app.exe`).

## Tests

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

Releases: see [releasing.md](releasing.md). The visual design (mark, colour, type, window) is in [design/gigapair-visual.html](design/gigapair-visual.html).
