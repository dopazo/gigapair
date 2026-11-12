# Releasing

Releases are built by [dist](https://opensource.axo.dev/cargo-dist/) (cargo-dist) in GitHub Actions (`.github/workflows/release.yml`) when a version tag is pushed. Configuration: `dist-workspace.toml`.

## What a release contains

For each target (Windows x86_64, macOS arm64 and x86_64, Linux x86_64 glibc):

- an archive (`.zip` on Windows, `.tar.xz` elsewhere) with `gigapair-app`, `gigapair`, `gpair` and `gigapaird`, plus `README.md` and `LICENSE`;
- on Windows, an MSI (`crates/gigapair/wix/main.wxs`) that installs to Program Files, adds the CLI to the `PATH` and a **Gigapair** Start menu entry for the app;
- shell (`gigapair-installer.sh`) and PowerShell (`gigapair-installer.ps1`) installers, without a self-updater;
- SHA-256 checksums (`sha256.sum` and one `.sha256` per file) and GitHub build attestations.

Only the `gigapair` package (`crates/gigapair`) is released: it holds every executable as a thin wrapper, because dist packages each crate separately and the app and the CLI need `gigapaird` next to them.

## Cutting a release

1. Make sure `main` is green.
2. Bump `version` in the root `Cargo.toml` (`[workspace.package]`), run `cargo build` so `Cargo.lock` follows, and merge that change to `main`.
3. Tag the merge commit and push the tag:

   ```sh
   git tag v0.1.0
   git push origin v0.1.0
   ```

4. Watch the **Release** workflow in the Actions tab. It builds every target, then creates the GitHub Release with all files and the attestations. A pre-release tag such as `v0.2.0-beta.1` is published as a pre-release.
5. Smoke-test the downloads: install the MSI on Windows and the shell installer on macOS and Linux, pair two devices and send a file.

Pull requests only run `dist plan` (a dry run), so a broken configuration shows up before tagging.

## Changing the configuration

Edit `dist-workspace.toml` and run `dist generate` (install with `cargo install cargo-dist --version 0.34.0 --locked`) to regenerate the workflow. `main.wxs` is edited by hand (Start menu shortcut), so `allow-dirty = ["msi"]` keeps dist from overwriting it; edit it directly, keeping the existing GUIDs.

## Not yet

DMG and a macOS `.app` bundle, AppImage, `.deb`, winget, Scoop, Homebrew, publishing to crates.io, code signing, a real app icon, the Windows Service / Linux system unit for servers and the "new release available" notice.
