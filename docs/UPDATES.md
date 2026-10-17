# GhostChat updates

The installed Windows app checks for updates when it starts, then every six hours. Users can also check from **Settings → Updates**. When a signed release is available, the app offers to download and install it, then restarts.

Updates use Tauri's signed updater bundles. Keep the private signing key private and backed up: existing installations trust the public key embedded in the app, so replacing or losing the private key breaks future updates.

## First GitHub release

1. Create a GitHub repository and push this project to it.
2. Add a repository Actions secret named `TAURI_SIGNING_PRIVATE_KEY`. Its value is the full contents of the local `crates/desktop/secrets/ghostchat-updater.key` file. Do not commit or share that file. The matching public key is already embedded in `crates/desktop/tauri.conf.json`.
3. Push a version tag matching the versions in the workspace and `crates/desktop/tauri.conf.json`, for example `v0.2.0`.
4. The release workflow builds and signs the Windows MSI, creates a GitHub Release, and uploads Tauri's `latest.json` updater feed.
5. Install that first updater-enabled MSI once. Existing 0.1.x installations do not contain the updater and cannot update themselves.

For later releases, increment the version in the root `Cargo.toml` and `crates/desktop/tauri.conf.json`, then push the matching `vX.Y.Z` tag. Keep the same signing key.

Automatic in-app updates apply to the installed MSI. Portable executable builds must be downloaded again from the latest release.

The repository does not yet exist, so the workflow cannot publish until it is created and the signing secret is added.
