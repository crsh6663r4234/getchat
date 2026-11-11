# GhostChat 4.0 release candidate

This document is the handoff checklist for the unreleased 4.0 candidate. The candidate is reviewable locally and has not been published. The release workflow now creates a GitHub draft rather than publishing automatically; a human must publish it after the release gates pass.

## Current evidence

- `cargo fmt --all` passes.
- `cargo test --locked --offline --workspace` passes: 1 two-account MLS integration test plus 3 server unit tests.
- The MLS integration test creates two isolated SQLite client stores, exchanges a Welcome, decrypts direct messages, reopens both stores, and rejects a replayed event. It does not replace a real two-Windows-device pass.
- `node --check crates/desktop/ui/main.js` passes.
- `node --check crates/desktop/ui/group-voice-client.js` passes after serializing screen-share start/stop/refresh operations.
- `git diff --check` passes.
- `cargo check --locked --offline --package ghostchat-desktop` passes with the deep-link and single-instance integrations.
- `node --check crates/desktop/ui/main.js` and `git diff --check` pass after the identity-change verification update.
- A clean release build passes with `cargo build --release --locked --offline --target-dir target/release-review-4.0-identity-guard --package ghostchat-server --package ghostchat-desktop`.
- The completed Windows build produced `ghostchat-desktop.exe` (23,979,008 bytes) and `ghostchat-server.exe` (9,921,536 bytes) in `target/release-review-4.0-identity-guard/release/`. This fresh output directory does not overwrite an app binary that may be running.
- MSI/signature/updater artifacts were not produced locally: `cargo-tauri` is not installed in this environment. The existing GitHub Actions workflow builds and signs the MSI, server executable, and updater metadata into a draft release; no release was published.
- Incoming MLS state, its decrypted-message cache entry, and the local event cursor now commit in the same SQLite transaction; restart/replay runtime validation is still required.
- MLS SQLite lookup references now use stable keyed digests while MLS entities keep randomized AES-GCM encryption; two-account tests cover the signature-key and KeyPackage lookups that randomized SQL keys broke.

## Candidate behavior covered

- New direct and group messages can use the MLS encrypted envelope after device setup and identity verification.
- Encrypted attachments use AES-256-GCM chunks; the server receives ciphertext and generic metadata.
- Group membership changes use staged MLS operations with roster checks and server-side authorization.
- Group voice and screen sharing use the configured LiveKit service; one-to-one calls use authenticated WebRTC signaling with optional TURN.
- Windows notifications, tray behavior, profile/avatar editing, Turkish UI, update checks, recovery-key backup, account export/delete, and session revocation are available.
- Encryption settings clearly warn that the account recovery key does not restore local MLS state and that device backup or transfer is not yet supported.
- Users can block or unblock peers and submit bounded reports. Blocking removes friendship and pending requests and prevents new direct messages, direct uploads, and one-to-one call signaling. Existing history and shared groups remain available.
- When a verified MLS identity or group roster changes, the client pauses later crypto events until the user compares old and current security codes through a trusted channel and approves the change.
- The client stores processed MLS event cursors with MLS state atomically and seeds the new cursor table from the prior client cursor on first upgrade.
- Owners/admins can copy a `ghostchat://invite/<code>` link for a regular group. The deep-link handler forwards URLs to the current app instance, validates the 43-character invite code, and queues it until an authenticated user can explicitly choose **Join group**. Encrypted groups continue to require the verified MLS membership flow.

## Upgrades are one-way: back up first

Back up before upgrading. Server migrations 0025 and later and the desktop app's account-bound local storage cannot be undone, so restoring a backup taken before the upgrade is the only way back to an older build:

- An older server binary refuses to start on a database that a newer one migrated: SQLx stops when the database records migrations the binary does not know.
- An older desktop build cannot use a profile a newer one opened: the rebuilt text cache has columns it does not fill, and it cannot find MLS groups whose storage is bound to the account.
- There are no reverse migrations. Several migrations rebuild tables (0027, 0029) or change keys and constraints, and the desktop re-keys stored groups to the account. Undoing the group binding would mean making every group readable to every account on the profile again, which is the defect the binding fixed, so it is not offered.

Automatic backups (builds after 4.0.51):

- **Server:** before it applies a migration numbered 0025 or higher to a database that already holds data, the server writes `<database>.pre-<first pending migration, four digits>.bak` next to the database (for example `ghostchat.sqlite.pre-0033.bak`) with SQLite `VACUUM INTO`. This applies to every future migration. An existing backup is never overwritten, new and in-memory databases get none, and if the backup cannot be written the server stops before migrating.
- **Desktop:** the app's local database `ghostchat.sqlite` in `%APPDATA%\app.ghostchat.desktop\` gets the same server backup when its built-in server migrates; if that backup cannot be written, GhostChat shows an error (in Turkish when Windows is set to Turkish) and closes without changing anything. In addition, at startup and before anything opens or migrates it, the app copies a database written by a build before the account binding once to `ghostchat.sqlite.pre-account-binding.bak` in the same folder. (A group still stored the old way on a profile whose cache was already rebuilt is copied the same way before its first adoption.) If the copy fails, the app starts but changes no encrypted data: encrypted chats stay paused with a message until a restart writes the copy, for example after freeing disk space.

**4.0.51 shipped without these backups.** Anyone who upgraded to 4.0.51 or earlier has already passed server migrations 0025-0032 and the desktop's account binding without an automatic copy; only their own manual backup can take them back. Later builds back up only the migrations that are still pending.

To go back: stop the server (or quit GhostChat, including from the tray), keep the current database aside, delete its `-wal` and `-shm` files, copy the backup over the database file, and start the older build. On a PC, both files written by one upgrade are taken at the same moment, before anything is migrated, and hold the same pre-upgrade state: restore either one (`pre-<version>.bak` exists only when server migrations were pending, `pre-account-binding.bak` only for a profile from before the account binding). After writing a new `pre-<version>.bak`, the server deletes that database's older ones so that only the three newest (by version) remain, never the one just written; copy one elsewhere if you need it longer. The single `pre-account-binding.bak` is never deleted automatically; delete it once the upgrade is confirmed.

Protection: a server backup is as sensitive as the server database (protect it as readable private-message data). The desktop copy keeps the field encryption of the original: MLS state and cached texts stay encrypted with the local storage key held in Windows Credential Manager (`GhostChat.MlsStorageKey`), while account, conversation and device ids, public keys and timestamps are stored in the clear in both. It sits in the same per-user application-data folder and needs no protection beyond the original's; it only opens on the same Windows account with that key. Unlike the live database, it keeps whatever is later deleted until you delete the file. `docs/SECURE_VAULT.md` describes a separate opt-in SQLCipher vault that does not cover `ghostchat.sqlite`.

## Required before public release

- Run a two-device MLS interoperability and restart/replay pass.
- Validate LiveKit over trusted TLS with two real devices, reconnects, device changes, screen refresh, and removed-member behavior.
- Validate installer upgrade, signed update download, restoring the automatic pre-upgrade backups with the previous build, and SmartScreen behavior on a clean Windows machine.
- Verify `ghostchat://invite/<code>` from a browser with the installed app closed and already running; confirm sign-in gating, explicit join confirmation, expired/revoked links, and that the invite-code endpoint cannot add members to MLS-locked groups.
- Review server environment, TLS proxy, TURN/LiveKit secrets, attachment storage, database backup, and report retention policy.
- Obtain an independent protocol/security review before making a production end-to-end-encryption claim.

Until those checks are complete, distribute the candidate only for controlled review and keep the stable release channel available.
