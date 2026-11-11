# Encrypted planner (polls and events) in encrypted groups

Date: 2026-11-11. Status: decided by the controller; the owner asked for this work ("devam").

## Problem

In an encrypted (MLS) group, chat messages are end-to-end encrypted, but polls and events (the planner) are stored on the server as plaintext: titles, options, descriptions, votes and RSVPs. The server can read them. The README and FEATURES say so today.

## Goal

For groups whose chat is encrypted (`conversation_crypto_locks` row present), the server stores planner items as opaque ciphertext and never sees titles, options, votes or RSVP answers. Non-encrypted groups keep the current behaviour unchanged.

## Design

1. Key. Each encrypted group derives a planner key from its current MLS epoch with the MLS exporter (`export_secret`, label `"ghostchat planner v1"`, context = conversation id). Every member at the same epoch derives the same key. The key changes when the group changes epoch, so after a removal the removed member cannot read new items (old items stay readable by members who held the older epoch only if they kept its key; accepted limitation, document it).
   - Old items: when the epoch moves, items written in earlier epochs must still be readable. Store with each ciphertext the epoch number and the key id; keep a bounded local cache of recent epoch planner keys per group on each device. Items from epochs a device never had stay unreadable there (documented, same class as past-epoch messages).
2. Encryption. AES-256-GCM, random 96-bit nonce per write, AAD = conversation id + item kind + item id + epoch.
3. Server. The planner tables keep their shape but title/description/options/ends/location/vote/rsvp payloads for encrypted groups are stored in a new `payload_ciphertext` column (BLOB) and the plaintext columns are NULL. Votes and RSVPs are stored as ciphertext per voter and tallied on clients. The server still needs `creator_id`, timestamps and item ids (existing account-deletion behaviour stays). `closes_at` for polls becomes a plaintext field only if needed for enforcement (keep plaintext `closes_at` so the server can refuse late votes; document that it leaks timing only).
4. Routes. Unchanged paths. For encrypted groups the request body carries `ciphertext`, `epoch` and `key_id` instead of plaintext fields; the server validates shape and size only (it cannot validate content). Non-encrypted groups keep the current request shape.
5. Clients. Desktop UI encrypts before posting and decrypts after fetching; the MLS export runs in the Rust crypto module (new Tauri command `crypto_planner_key(conversation_id, epoch)`), never in JS. Titles are not shown to other members until they decrypt; a locked item shows "encrypted item, cannot open on this device".
6. Migration. A new server migration adds the ciphertext column and makes the plaintext columns nullable. Existing items in encrypted groups are NOT rewritten automatically (they were plaintext); the migration leaves them and the UI labels them "stored before encryption". New items in encrypted groups are always ciphertext.
7. Compatibility. The server keeps accepting plaintext for non-encrypted groups. An older client against an encrypted group must get a clear "update GhostChat" error, not silent plaintext writes (server refuses plaintext writes to encrypted groups).

## Non-goals

No change to chat message encryption. No encryption of group membership metadata. No attempt to hide the number of votes or who voted from the server (it sees counts of ciphertext rows; documented).

## Tests

- Rust: export key equal across members at the same epoch and different after a commit; round trip; wrong AAD fails; a removed member's device cannot derive the new key.
- Server route tests: encrypted group rejects plaintext planner writes; accepts ciphertext; non-encrypted group unchanged; old-client plaintext to encrypted group refused with a specific code; account deletion keeps items with anonymous creator (existing test).
- Node: UI encrypts/decrypts through a fake crypto bridge; locked item rendering.
- Windows manual: two PCs, encrypted group, create a poll and an event on one, vote/RSVP on the other, confirm the server database contains no titles (check the sqlite file with `strings`).

## Documents to update

README and FEATURES.md (remove the "planner stored as plaintext" statement for encrypted groups), SECURITY.md, CRYPTO_EVENT_RELAY.md.
