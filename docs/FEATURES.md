# Feature checklist

## Implemented

- Windows Tauri 2 desktop shell with a dark theme, optional light theme, adjustable text size, and keyboard navigation.
- Anonymous account creation with random UUID v4 identifiers, unique usernames, and a one-time 256-bit recovery key.
- Argon2-hashed recovery verifier, SHA-256 lookup digest for high-entropy recovery keys, 30-day sessions, and session rotation on recovery.
- Windows Credential Manager storage scoped separately to each configured server.
- Temporary recovery-key backup in a separate Windows Credential Manager entry until the user confirms saving it.
- Authenticated user lookup, user search by username or ID prefix, friend requests, request acceptance/rejection, and friend removal.
- Direct-message block and report controls. Blocking removes a friendship and pending requests, hides the pair from search, and stops new direct messages, file uploads, and one-to-one call signaling. Existing chat history remains available; shared group chats are unchanged; unblocking does not restore friendship. Reports store only the chosen category and optional details, never attach message contents, and are included in the reporter's account export. A moderation console/review workflow is not included.
- Authorized direct and group conversations, SQLite message persistence, bounded history pages, message search, edit/delete, timestamps, and duplicate-send protection.
- Group name and membership management, friend-only invites, member removal/leave, owner transfer, and owner/admin/member roles enforced by the server.
- Owner/admin-created group invite codes expire after seven days, cap at 100 uses, and can be revoked; joining requires an authenticated GhostChat account. Each code can be copied directly or shared as a `ghostchat://invite/…` app link; joining still requires an explicit action. Invite-code joins remain disabled for MLS-locked groups.
- Reply references, shared emoji reactions, and pinned messages on persisted conversations.
- Conversation lists show unread message counts. Opening a conversation records a monotonic read position; direct chats show “Seen” when the peer's read position passes your latest message. Active typing is relayed live to other members.
- Photo, video, and file attachments stored on disk with authenticated, expiring download links and a 5 GB per-file limit.
- Authenticated WebSocket delivery with recipient filtering, reconnect recovery, immediate friend-request and acceptance events, and session-revocation events.
- Settings can revoke every other account session while keeping the current session signed in; revoked active WebSocket sessions are closed.
- Windows desktop notifications for incoming calls, friend requests, accepted requests, and messages outside the active conversation; notification controls and a test action are available in Settings while the app is running.
- One-to-one WebRTC voice calls with authenticated offer/answer/ICE signaling, device selection, audio processing controls, push-to-talk, and optional coturn TURN credentials.
- In-call screen sharing through the operating system's display picker, with optional system audio sharing.
- Server-side account/session/message input checks, request-size limits, and process-local rate limits.
- Account JSON export and permanent account/data deletion with a confirmation step in the client.
- Editable display name, username and profile photo, plus a minimize-to-tray window with open and quit actions and an optional per-Windows-user launch-at-sign-in setting.
- A loopback-only local server embedded in the desktop process and a separately deployable server executable.
- Unreleased 4.0 alpha: optional MLS encryption for new direct or group text after devices publish KeyPackages and users verify identity codes. Local MLS state and cached sent text are encrypted at rest. The server stores ciphertext for those new messages, while earlier history stays plaintext. Search checks only the latest 50 messages per conversation locally and never sends the search phrase to the server. Group owners/admins can add accepted friends using targeted MLS Commit/Welcome events after verifying each new KeyPackage and remove non-owner members with a targeted MLS Commit. Non-owners can submit a self-Remove proposal for the unique owner to commit; the departing client remains in the group until the commit is confirmed. New attachments in MLS chats are chunk-encrypted on Windows; previous attachments remain plaintext. These encrypted paths are experimental and still need multi-device, restart/replay, interoperability, and release validation.
- Settings can list registered MLS encryption devices and revoke that device ID's ability to claim unclaimed KeyPackages. A separate session control can close other account sessions. Device revocation itself does not invalidate packages already claimed or remove a device from existing encrypted groups.

## Not implemented

- Communities/servers, channels, invite links, fine-grained permissions, a moderation console/review workflow, audit logs, and presence states.
- Group read-receipt details and offline push notifications.
- Production group-voice deployment and end-to-end call testing; the branch now includes optional LiveKit SFU integration, which requires server-only credentials, public media connectivity, and is not end-to-end encrypted. Group camera calls are not included.
- Stable multi-device operation, full-history encrypted search, device recovery/replacement, and key-change reset flows. MLS addition/removal/self-leave, direct/group text, and encrypted attachment paths are experimental and unreleased; they have not had interoperability or independent security review.
- Account-wide preferences sync and an external community directory.
- Automatic database backups and full audit logging.

The 4.0 work is still in progress. Group text/file sharing and desktop group management are available in this branch. Direct calls remain peer-to-peer; group voice and screen sharing use the optional LiveKit SFU integration and still need a configured media host and real multi-device validation. Newly sent text and new files in opted-in, verified direct and group chats use the experimental MLS path. Earlier messages/files and group media are not end-to-end encrypted. Encrypted group additions, removals, and owner-committed non-owner self-leave use experimental MLS updates and still need multi-device validation.
