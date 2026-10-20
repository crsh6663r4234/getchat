# GhostChat updates

The installed Windows app checks for updates when it starts, then every six hours. Users can also check from **Settings → Updates**. When a signed release is available, the app offers to download and install it, then restarts.

Updates use Tauri's signed updater bundles. Keep the private signing key private and backed up: existing installations trust the public key embedded in the app, so replacing or losing the private key breaks future updates.

## GitHub releases

The source and release workflow are published at [crsh6663r4234/getchat](https://github.com/crsh6663r4234/getchat).

1. In the repository's **Settings → Secrets and variables → Actions**, add a repository secret named `TAURI_SIGNING_PRIVATE_KEY`. Its value is the full contents of the local `crates/desktop/secrets/ghostchat-updater.key` file. Do not commit or share that file. The matching public key is already embedded in `crates/desktop/tauri.conf.json`.
2. Increase the version in the workspace `Cargo.toml`, `Cargo.lock`, and `crates/desktop/tauri.conf.json`, then push the change to `main`. A matching `vX.Y.Z` release is created automatically.
3. The release workflow builds and signs the Windows MSI, creates a GitHub Release, uploads Tauri's `latest.json` updater feed, and attaches a standalone Windows x64 server with its SHA-256 checksum.
4. Install the updater-enabled MSI. Existing 0.1.x installations do not contain the updater and cannot update themselves.

For later releases, increment the version in the root `Cargo.toml`, `Cargo.lock`, and `crates/desktop/tauri.conf.json` before pushing to `main`. Keep the same signing key.

Automatic in-app updates apply to the installed MSI. Portable executable builds must be downloaded again from the latest release.
