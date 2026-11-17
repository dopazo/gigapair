# Releasing

Releases are built by [dist](https://opensource.axo.dev/cargo-dist/) (cargo-dist) in GitHub Actions (`.github/workflows/release.yml`) when a version tag is pushed. Configuration: `dist-workspace.toml`.

## What a release contains

For each target (Windows x86_64, macOS arm64 and x86_64, Linux x86_64 glibc):

- an archive (`.zip` on Windows, `.tar.xz` elsewhere) with `gigapair-app`, `gigapair`, `gpair` and `gigapaird`, plus `README.md` and `LICENSE`;
- on Windows, an MSI (`crates/gigapair/wix/main.wxs`) that installs to Program Files, adds the CLI to the `PATH` and a **Gigapair** Start menu entry for the app. Installing any MSI replaces the installed one, including a rebuild with the same version (`AllowSameVersionUpgrades`); installing an older version is refused, but Windows Installer compares only the first three numbers of the version, so the betas of one version and its final release all count as the same version and replace each other in any order. It also adds a Windows Firewall exception for `gigapaird.exe` (all profiles, removed on uninstall), so the first run shows no firewall prompt and contacts can connect directly. If Gigapair is running, the installer ends the app and then the service itself (they have no files-in-use dialog to answer; the app goes first because it restarts a service that stops); a transfer interrupted this way resumes when the service starts again. The executables carry the app icon (`crates/gigapair/build.rs`, `assets/gigapair.ico`);
- on macOS, a DMG per architecture (`gigapair-<target>.dmg`) with `Gigapair.app` and a shortcut to Applications. It is built by `scripts/macos-bundle.sh`, called from the custom job `.github/workflows/build-dmg.yml` (`global-artifacts-jobs` in `dist-workspace.toml`), which repackages the macOS archives: the app holds the four programs in `Contents/MacOS` and an icon made from `gigapair.ico`. It is signed ad hoc (needed on Apple silicon) but not with a Developer ID or notarized, so the first launch needs **Open Anyway**. Each DMG has its own `.sha256` and attestation;
- on Linux, a `.deb` and an `.rpm` (`gigapair-x86_64-unknown-linux-gnu.deb` / `.rpm`) built with [nfpm](https://nfpm.goreleaser.com) by `scripts/linux-packages.sh` from the Linux archive, in the custom job `.github/workflows/build-linux-packages.yml`. They install the four programs in `/usr/bin`, the menu entry and the icon (`packaging/linux/`). Removing the package runs `prerm.sh`, which stops Gigapair and deletes every user's `~/.config/systemd/user/gigapaird.service`; it never touches contacts, keys or files. The deb version is `0.1.0~beta.6`, so a beta sorts before the final release. Each package has a `.sha256` and an attestation;
- shell (`gigapair-installer.sh`) and PowerShell (`gigapair-installer.ps1`) installers, without a self-updater;
- SHA-256 checksums (`sha256.sum` and one `.sha256` per file) and GitHub build attestations.

Only the `gigapair` package (`crates/gigapair`) is released: it holds every executable as a thin wrapper, because dist packages each crate separately and the app and the CLI need `gigapaird` next to them.

## Cutting a release

1. Make sure `main` is green.
2. Bump `version` in the root `Cargo.toml` (`[workspace.package]`), run `cargo build` so `Cargo.lock` follows, and update the version in the install commands of `README.md` and `docs/usage.md` (they name it while there are only pre-releases; from the first stable release they can use `releases/latest/download/`). Merge that change to `main`.
3. Tag the merge commit and push the tag:

   ```sh
   git tag v0.1.0
   git push origin v0.1.0
   ```

4. Watch the **Release** workflow in the Actions tab. It builds every target, then creates the GitHub Release with all files and the attestations. A pre-release tag such as `v0.2.0-beta.1` is published as a pre-release.
5. Smoke-test the downloads: install the MSI on Windows, the DMG on macOS (drag to Applications, **Open Anyway**) and the `.deb` or `.rpm` on Linux (check the menu entry, then `apt remove` / `dnf remove`), pair two devices and send a file. Also update over a running install: with the previous release installed and the app and service running (and a transfer going, if you can), install the new one, then check that `gigapair --version` and `gigapaird --version` both print the new version (on Windows, both in `C:\Program Files\gigapair\bin\`) and that `gigapair status` works without a restart or reboot. Other packages (deb, rpm, DMG, shell installer) don't stop a running service: the first `gigapair` command or app launch after the update restarts it if it is older (`ipc::connect_or_spawn`), and prints a line saying so.

Pull requests only run `dist plan` (a dry run), so a broken configuration shows up before tagging.

## Changing the configuration

Edit `dist-workspace.toml` and run `dist generate` (install with `cargo install cargo-dist --version 0.34.0 --locked`) to regenerate the workflow. `main.wxs` is edited by hand (Start menu shortcut), so `allow-dirty = ["msi"]` keeps dist from overwriting it; edit it directly, keeping the existing GUIDs.

## Not yet

AppImage, Flatpak, an AUR package, a signed apt/dnf repository, winget, Scoop, Homebrew, publishing to crates.io, code signing, the macOS/Linux app icon, the Windows Service / Linux system unit for servers, and an automatic "new release available" notice (today the check is manual: Settings → Check for updates, `gigapair check-update`).
