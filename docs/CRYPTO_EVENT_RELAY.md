# Opaque crypto-event relay (4.0 foundation)

This API is transport and persistence only. It does not encrypt, decrypt, authenticate protocol messages, or prove device identity. Do not send plaintext to it and assume the server cannot read it. The unreleased desktop uses it for experimental MLS setup events.

All requests require the normal `Authorization: Bearer <session>` header. The caller must belong to the conversation. By default the server stores the URL-safe base64 payload as supplied and forwards it to current conversation members. A sender may set `recipient_user_id` to deliver an opaque event only to a current member. The desktop uses targeted Welcome delivery during MLS setup and targeted Commit delivery for additions to an existing encrypted group. The client protocol must authenticate and validate every event.

## Publish

`POST /v1/conversations/{conversation_id}/crypto-events`

```json
{
  "client_event_id": "a-unique-client-generated-id",
  "recipient_user_id": null,
  "payload": "<URL-safe-base64-without-padding>"
}
```

`recipient_user_id` is optional. When supplied, it must identify a current member of the conversation; otherwise the event is rejected. Sync returns broadcast events and events addressed to the authenticated user. A targeted event is sent only to that user over WebSocket. The decoded payload must contain 1–65,536 bytes. Retries with the same `(conversation, sender, client_event_id)` and identical recipient/payload are idempotent; reusing an ID for different contents is rejected. Clients should use a fresh ID for every distinct protocol event.

## Sync

`GET /v1/conversations/{conversation_id}/crypto-events?after=0&limit=100`

Events are returned in ascending server sequence. Persist the largest processed `sequence` as the next `after` cursor. `limit` is clamped to 1–100. Reconnect recovery uses this endpoint; WebSocket delivery is only a low-latency hint.

The visible sequence can have gaps because targeted events for other recipients are filtered out. Clients must process the returned rows in ascending order and must not require contiguous sequence numbers. The 4.0 desktop stores each incoming event cursor in the same SQLite transaction as its MLS state and decrypted local cache entry; events sent by the current user advance the same local cursor separately because their MLS state was already updated when sent. On first upgrade, the prior local cursor is imported once.

New events are sent to authenticated WebSocket recipients as:

```json
{
  "type": "crypto_event",
  "event": {
    "sequence": 1,
    "id": "server-event-id",
    "conversation_id": "conversation-id",
    "sender_id": "user-id",
    "recipient_id": null,
    "payload": "<opaque-payload>",
    "created_at": "timestamp"
  }
}
```

The server learns conversation membership, sender, optional recipient, event size, ordering and timing. The server does not validate the opaque bytes as MLS. The unreleased desktop uses this API to deliver direct and group MLS setup events; encrypted text is stored as opaque ciphertext in the ordinary ordered message history. The client validates KeyPackages against the expected account/device credential and asks users to compare identity codes out of band. This is experimental: only one desktop MLS identity per account is supported, previous history and legacy attachments are not encrypted, device recovery is unavailable, and plaintext-to-encrypted migration is not implemented. New attachments in locked chats use a separate Windows client-side AES-GCM chunk stream; their manifest and key are sent inside an MLS-encrypted message.

## Encrypted conversation lock

- `GET /v1/conversations/{conversation_id}/crypto-lock` returns `locked`, `locked_by`, and `setup_complete` to a conversation member.
- `POST /v1/conversations/{conversation_id}/crypto-lock` accepts `{ "member_ids": ["..."] }`. For a direct conversation, the submitted list must exactly match its two current members. For a group, an owner or admin may lock the exact current roster of 2–16 members. A mismatch is rejected. Only the account that started an incomplete setup may resume it.
- After publishing a separate targeted MLS Welcome to every group member, the initiating account calls `POST /v1/conversations/{conversation_id}/crypto-lock/complete`. The Welcome carries the initial group state; the initiating client keeps its locally encrypted setup outbox in the same transaction as the MLS state so it can retry if the app closes during setup.
- Once setup starts, the server requires the `e2e1:` message envelope for both direct and group conversations. The HTTP API validates its base64 payload and a SQLite trigger enforces the envelope at insert time, so a plaintext send racing the lock cannot slip through. The server cannot prove that an opaque payload is valid MLS ciphertext; a modified client can still submit arbitrary bytes under that envelope. New attachment uploads and message edits are also rejected. Existing history and files remain available in their original plaintext form.
- In a locked group, outstanding invite codes are revoked and invite-code joins remain blocked. An owner or admin may add accepted friends only through an MLS membership operation: the client verifies each new device KeyPackage, stages an MLS Commit, and stores the encrypted pending update locally; the server checks the expected roster and atomically adds members plus targeted Commit/Welcome events; the client merges its pending commit only after the server confirms success. The stable operation ID makes retries safe after disconnects. Owners/admins can also remove non-owner members with a staged MLS Commit; the server removes the member and stores targeted Commit events for remaining members in one transaction. The removed user and committer are not sent that relay event. A non-owner may submit an authenticated MLS self-Remove proposal; the unique owner commits the pending proposal, after which the server removes the leaver and stores the Commit only for remaining members. The leaver keeps the group locally until the owner commit is confirmed, then removes local MLS state. The client verifies the resulting roster and asks remaining members to verify the changed group fingerprint before sending again. These membership flows remain experimental: the server cannot parse MLS commits, and owner-offline/restart behavior, device replacement, multi-device group state, two-device interoperability, and independent security review still need validation.

Self-leave requests use `POST /v1/conversations/{conversation_id}/crypto-membership/leave` with `operation_id`, the exact `expected_member_ids`, and the URL-safe-base64 MLS proposal in `proposal_payload`. Only one unresolved leave request is allowed per group so concurrent MLS proposals cannot race the owner. The unique owner client commits the pending MLS proposal and submits `POST /v1/conversations/{conversation_id}/crypto-membership/leave/commit/{user_id}` with the same operation ID, roster, and `commit_payload`; the server verifies the matching request exists, atomically removes the non-owner, records the idempotency key, and delivers the Commit only to the remaining members. Role changes and other membership changes are held until that request is resolved. If the owner is offline, the request stays pending and retries when the clients reconnect. An owner must transfer ownership before leaving.

## Device registration

- `GET /v1/me/crypto/devices` lists the signed-in account's device records.
- `POST /v1/me/crypto/devices` registers or renames a device:

```json
{ "id": "<device UUID>", "label": "Casper's PC" }
```

- `DELETE /v1/me/crypto/devices/{device_id}` revokes the current user's device and removes its unclaimed packages. It does not remove an already-added MLS member from existing groups; group removal is a separate owner/admin action using an MLS Commit.

## Public KeyPackages

- `POST /v1/me/crypto/devices/{device_id}/key-packages` uploads `{ "package_ref": "<MLS hash reference>", "payload": "<URL-safe-base64>" }`. The device must belong to the caller and be active. Each active device can keep at most 100 unclaimed packages. Packages expire from the relay after 84 days, matching the current OpenMLS default lifetime; clients still must validate the MLS package's actual lifetime.
- `POST /v1/users/{target_user_id}/crypto/key-package` claims one package from the target account's active devices. A claim is allowed for the caller's own account, a friend, or another member of a shared conversation. The JSON body is `{ "client_claim_id": "<stable UUID for this request>" }`. Reuse the same ID on retries; using it for a different target is rejected.

Only public KeyPackage bytes are uploaded; private key material stays on the device. The server does not parse or verify the package, bind its MLS credential to the account/device record, or provide key transparency. The unreleased desktop validates the credential against the requested account/device and shows a fingerprint for manual out-of-band comparison, but this does not prevent a malicious server from substituting keys without an independent authenticated key-transparency channel.
