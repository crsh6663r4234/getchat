# GhostChat updates

The installed Windows app checks for updates when it starts, then every six hours. Users can also check from **Settings → Updates**. When a signed release is available, the app offers to download and install it, then restarts.

Updates use Tauri's signed updater bundles. Keep the private signing key private and backed up: existing installations trust the public key embedded in the app, so replacing or losing the private key breaks future updates.

## Before updating

Copy this note into the release text of every release until no 4.0.53 or older installation is left:

> **Before updating.** Desktop app 4.0.48 to 4.0.53: the automatic update cannot install (the app closes and does not start again). Download `GhostChat_4.0.54_x64_en-US.msi` from the 4.0.54 release and install it once by hand, with GhostChat closed; later updates install automatically. Standalone server 4.0.53 and older: replace `ghostchat-server-windows-x64.exe` by hand once; from 4.0.54 on the server updates itself. Back up the server database and each PC's `%APPDATA%\app.ghostchat.desktop\` folder first.

The release workflow adds the same note to the generated release notes. [TROUBLESHOOTING.md](TROUBLESHOOTING.md#automatic-update-closes-the-app-and-nothing-is-installed) explains the cause.

## Back up before you update

Updates cannot be undone by installing an older version: server migrations 0025 and later have no reverse migrations, an older server binary refuses to start on a database a newer one migrated, and an older desktop build cannot read the account-bound encrypted groups and text cache a newer one wrote. Restoring a backup taken before the update is the only way back. See "Upgrades are one-way" in [GHOSTCHAT_4_0_RELEASE_CANDIDATE.md](GHOSTCHAT_4_0_RELEASE_CANDIDATE.md) for why and for the restore steps.

Builds from 4.0.54 on take these backups automatically, next to the database they copy:

- The server, and the desktop app's built-in server, write `<database>.pre-<first pending migration>.bak` (for example `ghostchat.sqlite.pre-0033.bak`) before applying a migration numbered 0025 or higher to a database that already holds data. They stop instead of migrating if the backup cannot be written, and keep the three newest of these backups per database.
- The desktop app copies its local database once to `ghostchat.sqlite.pre-account-binding.bak` in `%APPDATA%\app.ghostchat.desktop\` at startup, before anything is migrated, when the profile was written by a build before the account binding. Both desktop backups of one upgrade hold the state before it. If the copy fails, encrypted chats stay paused with a message until a restart writes it.

4.0.53 and earlier shipped without these backups: an installation already on 4.0.51, 4.0.52 or 4.0.53 has passed migrations 0025-0032 and gets automatic backups only for later migrations. Before updating, still copy the server database (stop the service or use a SQLite-aware backup) and, on each PC, the `%APPDATA%\app.ghostchat.desktop\` folder with GhostChat closed.

## The standalone server updates itself

Builds from 4.0.54 on of the standalone Windows server, `ghostchat-server-windows-x64.exe`, update themselves; the desktop app's built-in server never does (the desktop app has its own updater). At startup and every 24 hours the server reads `latest.json` of the latest GitHub release. Only a strictly higher version is installed, never an older or equal one. The server then:

1. downloads `ghostchat-server-windows-x64.exe` and its `.sig` next to the running exe (as `<exe>.new` and `<exe>.new.sig`) and checks the minisign signature with the updater public key built into the server, the same key the desktop updater uses for the MSI. A file whose signature does not match is deleted and nothing changes. The `.sha256` file is not trusted on its own.
2. writes a copy of the database, `<GHOSTCHAT_DATABASE>.pre-update.bak` (for example `ghostchat.sqlite.pre-update.bak`), with SQLite `VACUUM INTO`, replacing the copy of the previous update. If it cannot be written, the update stops.
3. stops listening (requests in flight get up to 10 seconds), renames itself to `<exe>.old`, moves the new exe in place and starts it with the same arguments, environment and working directory.
4. waits up to two minutes for the new server to answer `GET /health` with the new version. Then the old process exits, and the new one deletes `<exe>.old` once Windows releases it. If the new server does not answer in time, it is stopped, the old exe is put back, the old server listens again, and that version is not tried again until the server restarts.

Every step is logged. Turn it off with `GHOSTCHAT_AUTO_UPDATE=false` (also `0`, `no` or `off`) in the server's environment; then replace the exe by hand as before.

The update also applies any new database migrations, with the automatic `pre-<migration>.bak` backups described above.

Servers on 4.0.53 and older (including 4.0.52 and 4.0.53) do not contain the self-updater: replace `ghostchat-server-windows-x64.exe` by hand once with 4.0.54 or newer. From then on the server keeps itself current.

## GitHub releases

The source and release workflow are published at [crsh6663r4234/getchat](https://github.com/crsh6663r4234/getchat).

1. In the repository's **Settings → Secrets and variables → Actions**, add a repository secret named `TAURI_SIGNING_PRIVATE_KEY`. Its value is the full contents of the local `crates/desktop/secrets/ghostchat-updater.key` file. Do not commit or share that file. The matching public key is already embedded in `crates/desktop/tauri.conf.json`.
2. Increase the version in the workspace `Cargo.toml`, `Cargo.lock`, and `crates/desktop/tauri.conf.json`, then push the change to `main`. A matching `vX.Y.Z` release is created automatically.
3. The release workflow builds and signs the Windows MSI, creates a GitHub Release, uploads Tauri's `latest.json` updater feed, and attaches a standalone Windows x64 server with its SHA-256 checksum and its updater signature (`ghostchat-server-windows-x64.exe.sig`, made with the same key as the MSI's), which the server's self-updater requires.
4. Install the updater-enabled MSI. Existing 0.1.x installations do not contain the updater and cannot update themselves.

For later releases, increment the version in the root `Cargo.toml`, `Cargo.lock`, and `crates/desktop/tauri.conf.json` before pushing to `main`. Keep the same signing key.

Automatic in-app updates apply to the installed MSI. Portable executable builds must be downloaded again from the latest release.
