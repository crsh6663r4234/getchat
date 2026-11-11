# GhostChat updates

The installed Windows app checks for updates when it starts, then every six hours. Users can also check from **Settings → Updates**. When a signed release is available, the app offers to download and install it, then restarts.

Updates use Tauri's signed updater bundles. Keep the private signing key private and backed up: existing installations trust the public key embedded in the app, so replacing or losing the private key breaks future updates.

## Back up before you update

Updates cannot be undone by installing an older version: server migrations 0025 and later have no reverse migrations, an older server binary refuses to start on a database a newer one migrated, and an older desktop build cannot read the account-bound encrypted groups and text cache a newer one wrote. Restoring a backup taken before the update is the only way back. See "Upgrades are one-way" in [GHOSTCHAT_4_0_RELEASE_CANDIDATE.md](GHOSTCHAT_4_0_RELEASE_CANDIDATE.md) for why and for the restore steps.

Builds after 4.0.51 take these backups automatically, next to the database they copy:

- The server, and the desktop app's built-in server, write `<database>.pre-<first pending migration>.bak` (for example `ghostchat.sqlite.pre-0033.bak`) before applying a migration numbered 0025 or higher to a database that already holds data. They stop instead of migrating if the backup cannot be written, and keep the three newest of these backups per database.
- The desktop app copies its local database once to `ghostchat.sqlite.pre-account-binding.bak` in `%APPDATA%\app.ghostchat.desktop\` at startup, before anything is migrated, when the profile was written by a build before the account binding. Both desktop backups of one upgrade hold the state before it. If the copy fails, encrypted chats stay paused with a message until a restart writes it.

4.0.51 and earlier shipped without these backups: an installation already on 4.0.51 has passed migrations 0025-0032 and gets automatic backups only for later migrations. Before updating, still copy the server database (stop the service or use a SQLite-aware backup) and, on each PC, the `%APPDATA%\app.ghostchat.desktop\` folder with GhostChat closed.

## GitHub releases

The source and release workflow are published at [crsh6663r4234/getchat](https://github.com/crsh6663r4234/getchat).

1. In the repository's **Settings → Secrets and variables → Actions**, add a repository secret named `TAURI_SIGNING_PRIVATE_KEY`. Its value is the full contents of the local `crates/desktop/secrets/ghostchat-updater.key` file. Do not commit or share that file. The matching public key is already embedded in `crates/desktop/tauri.conf.json`.
2. Increase the version in the workspace `Cargo.toml`, `Cargo.lock`, and `crates/desktop/tauri.conf.json`, then push the change to `main`. A matching `vX.Y.Z` release is created automatically.
3. The release workflow builds and signs the Windows MSI, creates a GitHub Release, uploads Tauri's `latest.json` updater feed, and attaches a standalone Windows x64 server with its SHA-256 checksum.
4. Install the updater-enabled MSI. Existing 0.1.x installations do not contain the updater and cannot update themselves.

For later releases, increment the version in the root `Cargo.toml`, `Cargo.lock`, and `crates/desktop/tauri.conf.json` before pushing to `main`. Keep the same signing key.

Automatic in-app updates apply to the installed MSI. Portable executable builds must be downloaded again from the latest release.
