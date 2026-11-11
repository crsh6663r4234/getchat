# GhostChat 4.0 — major release plan

GhostChat 4.0 is the next major product release. The stable public channel remains 0.3.8. The unpublished local review candidate is version 4.0.0 so its installer and updater metadata match the planned major release; no GitHub release has been created or published. Production release still depends on the validation gates below.

## Release goals

1. **Private conversations:** end-to-end encrypted direct and group messages, encrypted attachments, per-device identity, key verification, recovery and device replacement flows. The server must relay ciphertext and must not have keys that decrypt content.
2. **Complete groups:** group profile and membership management, invite links, roles/permissions, message replies/reactions/pins, and group voice/screen-sharing sessions.
3. **Reliable desktop calling:** clearer call state, reconnect handling, device controls, TURN relay guidance, screen-share source selection/refresh, and diagnostics that explain network failures.
4. **Safer daily use:** tray/minimize behavior, startup preference, notification controls, block/report tools, account/device management, and useful data export/backup behavior.

## Delivery sequence

### 4.0 foundation — in progress

- Add an authenticated, membership-checked relay for opaque encrypted conversation events.
- Keep existing plaintext messaging behavior explicit and unchanged while the encrypted client is being built.
- Record privacy limits and decide the protocol/library before shipping any encryption claim.

### 4.0 alpha — encrypted messaging

- Prototype MLS 1.0 for direct chats and groups, with client-side state and keys kept on each device.
- The desktop now pins OpenMLS 0.9.2 with its SQLx storage provider (SQLx 0.8.6, Rust 1.91 minimum). Persisted MLS values use a custom authenticated AES-256-GCM codec, and the codec key is held in Windows Credential Manager. This still needs Windows upgrade/recovery checks and protocol interoperability review before release. The desktop uses OpenMLS's RustCrypto primitive provider with SQLx-backed persistent storage; it does not use the in-memory provider for its persistent MLS state.
- Add authenticated device registration, KeyPackage publishing/claiming, group lifecycle messages, encrypted message rendering and encrypted attachment upload/download.
- **In progress:** the desktop now creates and verifies one-time KeyPackages, creates direct or group MLS state, relays targeted Welcome events, and encrypts/decrypts new direct or group text. Setup messages are kept in an encrypted local outbox with the MLS state for retries. Group setup requires verifying each member's device code, then verifying the group code with members. The server locks both direct chats and groups when encryption setup starts, requires the `e2e1:` envelope with a SQLite trigger to close plaintext-insert races, and disables edits and plaintext uploads after the lock. New encrypted attachments are chunk-encrypted on Windows before upload; their key and display metadata travel inside an MLS-encrypted message, while the server stores generic metadata and ciphertext only. The opaque message payload is not independently validated as MLS by the server. Group setup revokes outstanding invitations. After setup, owners/admins can add accepted friends with a staged MLS Commit and per-member Welcome, and can remove non-owner members with a staged MLS Commit addressed only to remaining members. A non-owner can now submit an MLS self-Remove proposal; the unique group owner commits it, and the server atomically removes the leaver and stores the Commit only for remaining members. The desktop keeps pending operations encrypted beside MLS state and merges owner commits only after server confirmation. Clients verify new KeyPackages and resulting rosters. This self-leave path is experimental and still needs two-device interoperability, replay/restart, owner-offline, and security review. Stable multi-device membership, recovery/device-loss, deployment, interoperability, and independent security review remain pending. Existing message history and legacy attachments remain plaintext.
- [x] Key-change UX: when an authenticated MLS commit changes a previously verified identity or group roster, stop consuming subsequent events until the user compares the old and current security codes and explicitly approves the change.
- [x] Persist incoming MLS event cursors in the same SQLite transaction as MLS state and the decrypted local message cache; migrate the prior cursor on first upgrade. Real restart/replay validation remains pending.
- [ ] Device recovery/replacement with portable encrypted MLS state; server operators must not be able to silently substitute identities.
- [x] Until a portable encrypted MLS-state backup exists, show an explicit Settings warning that the account recovery key does not restore this device's encrypted chat state and that device replacement is not supported.
- [x] Search the 50 newest messages per conversation locally without sending the search phrase to the server. A complete encrypted-history local index remains pending until storage and recovery are dependable.

### 4.0 beta — group and calls

- Group creation, group naming, friend invites, member removal/leave, owner transfer, owner/admin/member roles, limited-use seven-day invite codes and `ghostchat://invite/…` app links, message replies, shared reactions and pins are implemented in the desktop and server. Per-conversation unread counts, persisted read positions, direct-chat seen status, and ephemeral typing indicators are also implemented. Invite-code links remain disabled for MLS-locked groups.
- [x] Add the LiveKit SFU client path for group voice and screen sharing, with a short-lived server-signed room token, member-only authorization, muted-on-join microphone, device controls, local per-participant mute, speaking indicators, and multiple screen-share tiles. This path is not end-to-end encrypted.
- [x] Ask LiveKit to evict all active sessions for an account when that member leaves or is removed; this is best-effort if the SFU API is unavailable. Self-hosted LiveKit does not revoke previously issued tokens, so verify reconnect behavior and the short token lifetime on the deployed build.
- [ ] Deploy LiveKit with trusted TLS, public media ports, and a private API secret; validate two-device join/reconnect, device changes, screen refresh, removed members, mixed app versions, and media relay behavior before enabling group calls broadly.

### 4.0 release candidate

- [x] Add a Settings view to list registered MLS encryption devices and revoke that device ID's ability to claim unclaimed KeyPackages. The UI explicitly warns that existing app sessions and account access remain active, already-claimed packages stay valid, and group membership is unchanged.
- [x] Add user block and report controls with server-side enforcement for friendship/search, direct messages/files, and one-to-one call signaling; reports contain only the explicitly entered reason/details and do not copy chat content.
- The optional Windows sign-in launch setting and tray behavior are implemented. Finish the remaining account/device lifecycle and data export/backup UX.
- Review database migration and rollback behavior, signed update flow and server upgrade instructions.
- Run protocol interoperability and security review, server/client integration checks, Windows installer upgrade checks, and real two-device call/share checks. Keep the existing stable release channel available until the candidate passes.

## Protocol and privacy decision

MLS is the leading protocol candidate because IETF RFC 9420 is a Standards Track protocol for asynchronous groups from two to thousands of members and specifies forward secrecy and post-compromise security. OpenMLS is the leading Rust implementation candidate, not an automatic security endorsement. Its upstream documentation describes its command-line client as a proof of concept; GhostChat must integrate the library API and review its current status rather than reuse that CLI. Track open upstream issues before each version pin; current reports include a memory-provider key-collision bug and nondeterministic ordering of valid Update/Remove proposals.

No custom key agreement or home-grown encryption will be introduced. The current stable release still stores readable message bodies and attachment bytes. The unreleased 4.0 direct/group text flow encrypts new messages after participants verify identities; the relay API alone is not end-to-end encryption. New file attachments in a verified encrypted chat use independent AES-256-GCM chunks, with the file key and manifest carried by an MLS-encrypted message; the server still sees conversation, uploader, timing, and encrypted byte count. Previously uploaded attachments remain plaintext. No production-wide security claim should be made before recovery, interoperability, and independent protocol review are complete.

## Current implementation status

- [x] Define major-release scope and staged acceptance goals.
- [x] Opaque event relay API and persistence (server foundation; not encryption by itself).
- [x] Device registration/revocation and one-time public KeyPackage upload/claim endpoints.
- [x] Initial MLS device identity and encrypted local storage bootstrap.
- [x] Experimental direct-chat MLS group setup and text encrypt/decrypt path in the unreleased desktop client; explicit peer fingerprint confirmation is required.
- [x] Group text conversations with friend invites, membership management, owner transfer and server-enforced roles.
- [x] Message replies and persisted, per-user emoji reactions.
- [x] Persisted shared message pins.
- [x] 5 GB photo/video/file attachment upload and download support (still plaintext at rest and in transit only protected by HTTPS).
- [x] Experimental MLS group bootstrap and text encrypt/decrypt path for one device per account; group membership is server-locked after setup, plaintext sends and new file uploads are blocked.
- [ ] Complete MLS group lifecycle: encrypted additions, non-owner removals, and owner-committed non-owner self-leave now have experimental staged commit paths; device replacement, stable multi-device support, restart/replay checks, interoperability review, and independent security review remain.
- [x] Experimental encrypted attachment upload/download in MLS chats: AES-256-GCM 1 MiB chunks, integrity-checked decrypt-to-file, and small allowlisted media previews. The current file picker/save flow is Windows-only; large-file and two-device behavior still needs release validation.
- [x] Local search across the 50 newest messages per conversation; the search phrase stays on the device.
- [x] List registered MLS encryption devices and revoke their future KeyPackage access, with the current local device protected from accidental self-revocation.
- [x] Revoke all other account sessions from Settings while keeping the current session active; active revoked WebSocket sessions are closed.
- [x] Block/unblock users, remove friendship and pending requests, hide blocked pairs from search, and reject new direct messaging, file uploads, and call setup; preserve old history and shared groups.
- [x] Submit bounded, rate-limited abuse reports without attaching messages; expose reports and block list in account export.
- [x] Add an optional per-Windows-user launch-at-sign-in setting and keep the app available from its tray icon when the window closes.
- [ ] Full-history encrypted search index and recovery-safe index rebuild.
- [ ] MLS recovery/device replacement, replay/restart checks, interoperability review, and independent security review.
- [x] Unread counts, persisted read positions, and ephemeral typing indicators.
- [x] Limited-use group invite codes with expiry and administrator revocation.
- [x] Copyable Windows app links for standard group invite codes; app-open and app-already-running handling queues the code for an authenticated user's explicit join action.
- [x] Group voice and screen-sharing client/server integration (deployment and real-network validation remain pending).
- [x] Add copyable `ghostchat://invite/…` links for regular groups with current-instance forwarding, startup queueing, and an explicit join action; encrypted groups retain the verified MLS membership flow.
- [ ] Validate invite links in the installed app and complete release validation for group voice/screen sharing.
- [ ] Upgrade, recovery and release-candidate validation.

## Next independent goal after 4.0

After this release goal is complete, start a separate end-to-end design audit and rebuild using the workflow in [GHOSTCHAT_DESIGN_REBUILD_PLAN.md](GHOSTCHAT_DESIGN_REBUILD_PLAN.md). Figma will hold the UI system, components, and prototypes; Higgsfield will support original visual exploration and optional brand artwork. This is queued after the 4.0 work so a full visual overhaul does not get mixed into the release candidate.
