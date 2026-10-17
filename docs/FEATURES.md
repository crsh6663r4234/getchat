# Feature checklist

## Implemented and verified

- Windows Tauri 2 desktop shell with a dark theme, optional light theme, adjustable text size, and keyboard navigation.
- Anonymous account creation with random UUID v4 identifiers, unique usernames, and a one-time 256-bit recovery key.
- Argon2-hashed recovery verifier, SHA-256 lookup digest for high-entropy recovery keys, 30-day sessions, and session rotation on recovery.
- Windows Credential Manager storage scoped separately to each configured server.
- Temporary recovery-key backup in a separate Windows Credential Manager entry until the user confirms saving it.
- Authenticated user lookup, user search by username or ID prefix, friend requests, request acceptance/rejection, and friend removal.
- Authorized one-to-one conversations, SQLite persistence, bounded history pages, message search, edit/delete, timestamps, and duplicate-send protection.
- Authenticated WebSocket delivery with recipient filtering, reconnect support in the client, and immediate session-revocation events.
- Server-side account/session/message input checks, request-size limits, and process-local rate limits.
- Account JSON export and permanent account/data deletion with a confirmation step in the client.
- A loopback-only local server embedded in the desktop process and a separately deployable server executable.

## Not implemented

- Group direct messages, communities/servers, channels, roles, permissions, invites, moderation tools, audit logs, blocking, and presence states.
- Message replies, reactions, read receipts, typing indicators, offline push notifications, attachment transfer, image previews, and message pinning.
- Voice, video, screen sharing, device selection, Opus/WebRTC, TURN, and push-to-talk.
- End-to-end encryption. The server can read stored message content.
- Account avatar/profile customization, account-wide preferences sync, tray mode, startup behavior, and an external community directory.
- Automatic database backups and full audit logging.

The deliverable is a functional private one-to-one chat application and a reusable server foundation, not feature parity with Discord. Do not advertise the omitted items as available.
