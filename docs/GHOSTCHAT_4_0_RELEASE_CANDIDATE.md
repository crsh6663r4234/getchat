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

## Required before public release

- Run a two-device MLS interoperability and restart/replay pass.
- Validate LiveKit over trusted TLS with two real devices, reconnects, device changes, screen refresh, and removed-member behavior.
- Validate installer upgrade, signed update download, migration rollback, and SmartScreen behavior on a clean Windows machine.
- Verify `ghostchat://invite/<code>` from a browser with the installed app closed and already running; confirm sign-in gating, explicit join confirmation, expired/revoked links, and that the invite-code endpoint cannot add members to MLS-locked groups.
- Review server environment, TLS proxy, TURN/LiveKit secrets, attachment storage, database backup, and report retention policy.
- Obtain an independent protocol/security review before making a production end-to-end-encryption claim.

Until those checks are complete, distribute the candidate only for controlled review and keep the stable release channel available.
