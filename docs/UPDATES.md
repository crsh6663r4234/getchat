# GhostChat updates

The installed Windows app checks for updates when it starts, then every six hours. Users can also check from **Settings → Updates**. When a signed release is available, the app offers to download and install it, then restarts.

Updates use Tauri's signed updater bundles. Keep the private signing key private and backed up: existing installations trust the public key embedded in the app, so replacing or losing the private key breaks future updates.

## Before updating

Copy this note into the release text of every release until no 4.0.53 or older installation is left:

> **Before updating.** Desktop app 4.0.48 to 4.0.53: the automatic update cannot install (the app closes and does not start again). Download `GhostChat_4.0.54_x64_en-US.msi` from the 4.0.54 release and install it once by hand, with GhostChat closed; later updates install automatically. Standalone server 4.0.53 and older: replace `ghostchat-server-windows-x64.exe` by hand once; from 4.0.54 on the server updates itself. Back up the server database and each PC's `%APPDATA%\app.ghostchat.desktop\` folder first.

The release workflow adds the same note to the generated release notes. [TROUBLESHOOTING.md](TROUBLESHOOTING.md#automatic-update-closes-the-app-and-nothing-is-installed) explains the cause.

## Back up before you update

Updates cannot be undone by installing an older version: server migrations 0025 and later have no reverse migrations, an older server binary refuses to start on a database a newer one migrated, and an older desktop build cannot read the account-bound encrypted groups and text cache a newer one wrote. Restoring a backup taken before the update is the only way back:

- SQLx stops when the database records migrations the binary does not know, so an older server cannot open a newer database.
- An older desktop build cannot use a profile a newer one opened: the rebuilt text cache has columns it does not fill, and it cannot find MLS groups whose storage is bound to the account.
- Several migrations rebuild tables (0027, 0029) or change keys and constraints, and the desktop re-keys stored groups to the account. Undoing the group binding would make every group readable to every account on the profile again, which is the defect the binding fixed, so it is not offered.

Builds from 4.0.54 on take these backups automatically, next to the database they copy:

- The server, and the desktop app's built-in server, write `<database>.pre-<first pending migration>.bak` (for example `ghostchat.sqlite.pre-0033.bak`) before applying a migration numbered 0025 or higher to a database that already holds data. They stop instead of migrating if the backup cannot be written, and keep the three newest of these backups per database.
- The desktop app copies its local database once to `ghostchat.sqlite.pre-account-binding.bak` in `%APPDATA%\app.ghostchat.desktop\` at startup, before anything is migrated, when the profile was written by a build before the account binding. Both desktop backups of one upgrade hold the state before it. If the copy fails, encrypted chats stay paused with a message until a restart writes it. The copy runs before the window can respond, so on a large profile the first start after the update takes that long (wait; do not end GhostChat in Task Manager).

4.0.53 and earlier shipped without these backups: an installation already on 4.0.51, 4.0.52 or 4.0.53 has passed migrations 0025-0032 and gets automatic backups only for later migrations. Before updating, still copy the server database (stop the service or use a SQLite-aware backup) and, on each PC, the `%APPDATA%\app.ghostchat.desktop\` folder with GhostChat closed.

## The standalone server updates itself

Builds from 4.0.54 on of the standalone Windows server, `ghostchat-server-windows-x64.exe`, update themselves; the desktop app's built-in server never does (the desktop app has its own updater). At startup and every 24 hours the server reads `latest.json` of the latest GitHub release. Only a strictly higher version is installed, never an older or equal one. The server then:

1. downloads `ghostchat-server-windows-x64.exe` and its `.sig` next to the running exe (as `<exe>.new` and `<exe>.new.sig`) and checks the minisign signature with the updater public key built into the server, the same key the desktop updater uses for the MSI. The signed trusted comment must name `ghostchat-server-windows-x64.exe`, so another file signed with that key (such as the MSI) is not accepted as the server. A file whose signature does not match is deleted and nothing changes. The feed's version must be a plain `major.minor.patch` (no `v` prefix, no leading zeros); any other form is refused before anything is downloaded. The `.sha256` file is not trusted on its own. Leftover files under those names are deleted before the download is written. The folder of the server exe must not be writable by other accounts.
2. waits while a call is relayed through the server (its call rooms live in the running process and would drop), for at most 30 minutes; after that it updates anyway. Calls over TURN or LiveKit do not hold it up.
3. writes a copy of the database, `<GHOSTCHAT_DATABASE>.pre-update.bak` (for example `ghostchat.sqlite.pre-update.bak`), with SQLite `VACUUM INTO`, replacing the copy of the previous update. If it cannot be written, the update stops.
4. stops listening (requests in flight get up to 10 seconds), checks the signature of `<exe>.new` again and that it holds the very bytes verified at download (otherwise it deletes the download, listens again and changes nothing), renames itself to `<exe>.old`, moves the new exe in place, checks those bytes once more and starts it with the same arguments, environment and working directory. It also sets `GHOSTCHAT_EXPECT_VERSION` to the version it installed: the signature covers the file but not its version, so a validly signed exe of another version stops before it opens the database.
5. waits up to two minutes for the new server to answer `GET /health` with the new version. Then the old process exits, and the new one deletes `<exe>.old` once Windows releases it. If the new server does not answer in time, it is stopped, the old exe is put back, the old server listens again, and that version is not tried again until the server restarts.
6. if the new server failed after it had already applied a database migration, the old exe is not put back, because it refuses to start on a database a newer build migrated. The new exe stays in place, the previous one stays as `<exe>.old`, and the old process logs an error and exits with code 1 (a service manager then restarts the exe, which is the new version). To go back instead, stop the server, restore `<exe>.old` over the exe and `<GHOSTCHAT_DATABASE>.pre-update.bak` over the database (delete its `-wal` and `-shm` files first), then start it.

Every step is logged. Turn it off with `GHOSTCHAT_AUTO_UPDATE=false` (also `0`, `no` or `off`) in the server's environment; then replace the exe by hand as before.

The update also applies any new database migrations, with the automatic `pre-<migration>.bak` backups described above.

Servers on 4.0.53 and older (including 4.0.52 and 4.0.53) do not contain the self-updater: replace `ghostchat-server-windows-x64.exe` by hand once with 4.0.54 or newer. From then on the server keeps itself current.

## Going back to an older build

Stop the server (or quit GhostChat, including from the tray), keep the current database aside, delete its `-wal` and `-shm` files, copy the backup over the database file and start the older build. Data written since the backup is lost. On a PC, the `pre-<version>.bak` and `pre-account-binding.bak` files written by one upgrade hold the same pre-upgrade state: restore either one. The server keeps only the three newest `pre-<version>.bak` files per database; the single `pre-account-binding.bak` is never deleted automatically, so delete it once the upgrade is confirmed.

A server backup is as sensitive as the server database. The desktop copy keeps the field encryption of the original (MLS state and cached texts stay encrypted with the local storage key in Windows Credential Manager, `GhostChat.MlsStorageKey`), while ids, public keys and timestamps are in the clear in both. Unlike the live database, a backup keeps whatever is later deleted until you delete the file.

## GitHub releases

The source and release workflow are published at [crsh6663r4234/getchat](https://github.com/crsh6663r4234/getchat).

1. In the repository's **Settings → Secrets and variables → Actions**, add a repository secret named `TAURI_SIGNING_PRIVATE_KEY`. Its value is the full contents of the local `crates/desktop/secrets/ghostchat-updater.key` file. Do not commit or share that file. The matching public key is already embedded in `crates/desktop/tauri.conf.json`.
2. Increase the version in the workspace `Cargo.toml`, `Cargo.lock`, and `crates/desktop/tauri.conf.json`, then push the change to `main`. A matching `vX.Y.Z` release is created automatically.
3. The release workflow ("Release GhostChat") builds and signs the Windows MSI, creates and publishes a GitHub Release marked as latest, uploads Tauri's `latest.json` updater feed, and attaches a standalone Windows x64 server with its SHA-256 checksum and its updater signature (`ghostchat-server-windows-x64.exe.sig`, made with the same key as the MSI's), which the server's self-updater requires.

For later releases, increment the version in the root `Cargo.toml`, `Cargo.lock`, and `crates/desktop/tauri.conf.json` before pushing to `main`. Keep the same signing key.

Automatic in-app updates apply to the installed MSI, the only desktop download the release publishes.
