# GhostChat security interfaces — Stage 0 contract

Contract version: `security-interfaces/0.1-draft`.
Prepared: 2026-11-04. Source baseline: `da5ee92a3ecaafd2d3d73a62d91f6854258f74a4`
(GhostChat 4.0.44). Scope: source inspection and implementation preparation.

This document distinguishes **existing** interfaces from **proposed** interfaces.
It does not enable mandatory encryption, SQLCipher, recovery shares, device proofs
or media E2EE. Proposed types below are design contracts, not exported Rust types
or new deployed endpoints. The lead must ratify format fixtures, module ownership
and acceptance evidence before implementation. No production data was inspected.

Repository instructions: no `AGENTS.md` was found in the checkout or the checked
ancestor chain through `C:\`. The execution contract and
`ZERO_TRUST_AUDIT_BLUEPRINT.md` were read. This worker owns only this document;
shared Rust, JavaScript, manifests and migrations remain untouched.

## 1. Baseline boundaries and their actual semantics

This table describes committed baseline `da5ee92`. The current implementation
increment below supersedes its application-outbox observations; other baseline
limitations remain unless separately evidenced.

| Interface | Existing contract | Evidence and limit |
|---|---|---|
| Rust text encryption | `crypto_encrypt_text(user_id, conversation_id, client_message_id, content) -> {payload}`; UUID normalization; nonempty text up to 2500 UTF-8 bytes; URL-safe unpadded base64 of serialized MLS message | `crates/desktop/src/main.rs::crypto_encrypt_text`, `crypto.rs::encrypt_text`. Plaintext enters from WebView; returned ciphertext is not a durable Rust application outbox. |
| MLS suite | `MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519` | `crypto.rs::MLS_CIPHERSUITE`. Separate from AES-256-GCM used for stored entities/files. |
| Serialization | Process-global `CRYPTO_OPERATION_LOCK`; OpenMLS SQLx storage uses the same SQLite transaction as local cache writes | `crypto.rs::CRYPTO_OPERATION_LOCK`, `encrypt_text`. This is not a cross-process lock or per-conversation scheduler. |
| Incoming events | `crypto_process_event` accepts sender/account/conversation, sequence, expected roster and base64 payload; returns event result | `main.rs::crypto_process_event`, `crypto.rs::process_crypto_event`. MLS state, decrypted cache and event cursor are committed together; current sequence is delivery position, not independently authenticated global freshness. |
| Membership staging | Pending bootstrap and membership operations can be read/finalized after interruption | `crypto.rs::get_pending_bootstrap`, `get_pending_group_membership_update`, `finalize_group_membership_update`; tables `ghostchat_crypto_bootstrap`, `ghostchat_crypto_membership_outbox`. These are not an application-message ciphertext outbox. |
| Network text send | `POST /v1/conversations/:id/messages` accepts `{client_message_id, content, reply_to_id}`; encrypted UI sets `content="e2e1:" + payload` and `reply_to_id=null` | `crates/desktop/ui/main.js::sendMessage`, `crates/server/src/lib.rs::SendMessage`, `send_message`. Conversation crypto lock enforces envelope/base64 shape; it does not prove arbitrary bytes are valid MLS. |
| Duplicate text send | Backend inserts with `INSERT OR IGNORE`, then returns the row selected by conversation/sender/client message ID | `server/src/lib.rs::send_message`. It does not currently reject different ciphertext submitted under the same idempotency identity. |
| Retry persistence | UI stores `{clientMessageId,payload}` in a conversation-specific localStorage entry after Rust encryption returns, removes it after HTTP success | `ui/main.js::encryptedMessagePendingKey`, `retryPendingEncryptedMessage`, `sendMessage`. Crash between Rust commit and localStorage write loses the retry ciphertext; one entry is not a durable multi-item Rust queue. |
| Local codec | Entity encoding is `GC\x01 || random_nonce[12] || AES256GCM_ciphertext_and_tag`; AEAD AAD is constant `GC\x01`. Certain lookup references are `GCK\x01 || HMAC-SHA256(key, domain || serialized_lookup)` | `crypto.rs::EncryptedJsonCodec`. Field encryption and keyed lookup references are present; row/account/device binding and full-page encryption must not be inferred from the constant AAD. |
| Storage key | A random 32-byte key is represented in Windows Credential Manager under `GhostChat.MlsStorageKey`; cached in `STORAGE_KEY` | `main.rs::windows_credential::get_or_create_crypto_storage_key`, `crypto.rs::STORAGE_KEY`. Current alias is not purpose/account/device/deployment scoped. Missing key currently invokes key creation; migration must distinguish first creation from loss of an existing vault key. |
| Desktop database | Embedded local HTTP state and MLS storage share clones of the opened SQLite pool at app-data `ghostchat.sqlite` | `main.rs::app_database_path`, `.setup` database initialization. `Cargo.toml` enables SQLx SQLite; current code does not verify a SQLCipher-enabled library or wrong-key behavior. |
| Account recovery | OS credential commands store/get/delete account recovery tokens; account recovery restores server authorization | `main.rs::credential_*_recovery`, `windows_credential`; `docs/SECURITY.md`. No authenticated Shamir share/archive import interface exists. Recovery token does not currently restore local MLS keys. |

Historical baseline send gap: a repeated `encrypt_text` call compares any cached
plaintext for the same message ID but still invokes `group.create_message` again.
It neither looks up nor returns previously persisted application ciphertext.
Atomic sender-state persistence alone therefore does not satisfy encrypt-once
retry. This document requires a new Rust-owned outbox contract rather than merely
renaming the existing plaintext cache.

### Current implementation increment — local ciphertext durability

The lead's current `crypto.rs` worktree patch introduces
`ghostchat_crypto_application_outbox(client_message_id,user_id,conversation_id,
encrypted_payload,...)`. `encrypt_text` now stores protected immutable MLS payload
bytes in the same SQLite transaction as the updated sender state and local cache.
On retry it compares cached content/conversation, verifies outbox account and
conversation scope, and returns the saved payload instead of calling MLS again.
Changed content/scope, legacy cache-only records without safe retry ciphertext,
and undecryptable/corrupt outbox data fail closed.

The lead reports its extended
`two_account_direct_mls_survives_restart_and_deduplicates_replayed_events` test
passed for byte-identical retries across database reopen, scope/content rejection,
legacy/corrupt retry rejection and injected outbox-insert failure. Source review
confirmed the fault-injection trigger aborts the outbox insert; the transaction
rolls back both cache insertion and MLS sender advancement before a later valid
send. Attach the lead's exact command/output to the central evidence manifest;
this documentation worker did not independently execute that Rust test.

This is a narrow local durability fix, not completion of Phase 1. The UI still
uses its existing retry trigger; durable startup discovery, delivery leases and
acknowledgements, V2 deployment/device-scoped identity, authenticated content
policy, ancillary-content encryption and backend conflicting-duplicate rejection
remain separate work. The baseline crash gap no longer means ciphertext is
absent from disk; automatic rediscovery/transmission of that committed record is
still required. No production deployment or full recovery/migration is implied.

## 2. Proposed ownership and typed failure boundary

The Rust crypto owner exposes a small application service to the UI. Inputs use a
validated `DeviceContext` obtained from Rust vault/session enrollment; account and
device strings supplied by JavaScript are not independent authorization proofs.
The backend still authenticates and authorizes its own network boundary.

Proposed operations:

```text
prepare_application(context, conversation_id, client_message_id,
                    content_kind, bounded_plaintext) -> PreparedMessage
list_pending(context, conversation_id?, bounded_cursor) -> PendingPage
acknowledge_delivery(context, outbox_id, server_receipt) -> DeliveryStatus
apply_inbound(context, delivery_envelope) -> VerifiedContentOrStateChange
get_conversation_status(context, conversation_id) -> ConversationStatus
```

The durable sender, not the WebView, owns transmission/retry orchestration and
receipt validation. `PreparedMessage` may expose an opaque outbox ID and display
state; exposing ciphertext for transport never transfers retry authority to JS.

Required typed outcomes: `VaultLocked`, `DeviceUnverified`, `ConversationLocked`,
`MembershipChanged`, `EpochUnavailable`, `IdempotencyConflict`, `ReplayRejected`,
`PolicyMismatch`, `StorageUnavailable`, `DeliveryPending`, `RecoveryRequired`.
None causes plaintext retry or automatic creation of replacement live identities.
UI errors omit keys, plaintext, signed tokens and raw private protocol state.

## 3. Conversation and application-envelope contract

Proposed conversation state:
`UNINITIALIZED -> INVITED -> VERIFIED -> ACTIVE -> REKEYING -> ACTIVE`.
An authenticated removal leads to `REMOVED`; verification, fork, vault or state
failure leads to `LOCKED`. Only `ACTIVE` may prepare application ciphertext.
Recovering from `LOCKED` requires the explicit matching verification/rejoin path,
not UI acknowledgement of a generic error. A restored device never reuses the
old device's leaf/sender state.

Authenticated `GroupPolicyV2` binds: protocol version, deployment ID, conversation
and MLS group ID, policy generation, parent policy digest, device roster, account
bindings, application roles, permitted change authority and content policy.
It is distributed/authenticated with the group's MLS state. Endpoint checks
reconcile its resulting roster with the MLS roster; server RBAC alone is not
proof of the client's Owner/Admin authority. The MLS credential format presently
parsed by `crypto.rs::account_id_from_credential` is `ghostchat:v1:<account UUID>:<device UUID>`;
V2 must define enrollment and domain/deployment binding before replacing it.

`PrivateApplicationV2` is a versioned typed union for text, edits, reply content,
reactions, private pin data, attachment manifests, private profiles/group fields,
Watch Party state/URLs and call notes. Keep private kind-specific values inside
MLS application plaintext. Outer routing IDs, framing, epoch references and
length/timing exposures receive an explicit metadata policy. Do not silently
reuse current plaintext ancillary HTTP/WS payloads for V2.

Define canonical serialization and interoperability fixtures before implementing
V2; arbitrary `serde_json` reserialization is not a signed-byte specification.
Existing `e2e1:` and `ghostchat-file-v1:` remain explicitly legacy formats until
cutover policy retires their write paths. Application protocol generation and
minimum supported policy are authenticated independently of the displayed app
version. This draft does not reserve an unimplemented `gc2:` network endpoint.

Bounds are mandatory before parsing/allocating. Existing byte/character limits
are inconsistent (2500 UTF-8 bytes before MLS, 4000 characters after base64 on the
message endpoint, up to 3 MiB for incoming crypto events). Crypto/backend owners
must agree separate plaintext, ciphertext, Commit/Welcome and event-batch byte
caps and publish exact fixtures before changing these limits. Do not simply
raise the generic message limit to accommodate arbitrary MLS objects.

## 4. Atomic ciphertext outbox contract

Identity is `(deployment_id, account_id, device_id, conversation_id,
client_message_id)`. Scope every lookup/uniqueness check to this tuple. Current
`ghostchat_crypto_text_cache` message-ID-only lookup is not the V2 identity.

Logical `OutboxRecordV2` fields:

```text
outbox_id, identity_tuple, protocol_version, group_policy_generation,
parent_epoch, ordered_dependencies, immutable_ciphertext_bytes,
ciphertext_digest, encrypted_local_display_reference,
delivery_state, attempt_count, next_attempt_at, created_at,
server_message_id?, authenticated_receipt_reference?
```

The ciphertext digest is an unkeyed digest of already encrypted bytes for
integrity/idempotency, not a public dictionary-testable hash of short plaintext.
Detect conflicting plaintext reuse inside the encrypted vault without exporting
a plaintext digest to the server. `ordered_dependencies` names preceding commits
or policy transitions that must be delivered before dependent ciphertext.

Required transition semantics:

1. Under one ordered per-conversation crypto worker and one database transaction,
   validate ACTIVE/policy, reserve identity, create MLS ciphertext once, persist
   new sender state and immutable outbox bytes, then commit.
2. Existing identity with matching request returns the existing record/bytes.
   Different content, conversation binding or dependencies produces
   `IdempotencyConflict`; do not call MLS encryption again.
3. Only committed records are sendable. Network failure returns `DeliveryPending`
   and leaves exact bytes retryable. Crash before commit leaves no sendable state;
   crash after commit/before send is handled by outbox discovery on startup.
4. Treat `READY -> IN_FLIGHT` as a recoverable lease/attempt, not delivery proof.
   Restart reclaims the attempt. Use at-least-once byte-identical delivery with
   server idempotency; do not claim exactly-once network delivery.
5. The backend key includes authenticated sender/device/conversation/message ID.
   Duplicate matching ciphertext and immutable envelope returns the same receipt;
   mismatched bytes/envelope under that key returns a conflict, never success for
   silently ignored data. Transport/device proof is authorized independently.
6. A server acknowledgement proves service acceptance under its trust boundary,
   not recipient decryption. Recipient-verified/read status is a distinct
   authenticated private event. Bind receipt to request identity and ciphertext.
7. Mark acknowledged delivery atomically and retain/reclaim records under explicit
   policy. Never delete the only retry bytes on an ambiguous timeout.

Inbound processing atomically commits accepted MLS state, dedup identity, private
cache and cursor. Verify cryptographic group/policy identity independently of
server delivery sequence. Bounded future-epoch objects wait for dependencies;
fork/malformed/replayed data cannot replace trusted current state. A local counter
does not resist a privileged attacker restoring both the vault and executable.

## 5. Attachment contract and WebView boundary

Existing `EncryptedAttachmentManifest` contains `attachmentId`, `fileName`,
`contentType`, `sizeBytes`, key, `noncePrefix`, `chunkSize`, `chunkCount`.
`crypto_attachments.rs` uses a fresh random 32-byte AES-GCM file key, random 8-byte
nonce prefix plus big-endian u32 chunk index, 1 MiB chunks, a 5 GiB cap, and AAD
`ghostchat-file-v1:<attachment UUID>:<index>`. This AAD does **not** currently bind
all target V2 conversation/version/length fields. The manifest returned to JS
contains the file key (`main.rs::crypto_upload_encrypted_attachment`); migration
must remove that long-lived key handling from the rendering boundary.

Proposed `PrivateAttachmentManifestV2` binds version, object/conversation IDs,
complete plaintext length, chunk geometry/count, plaintext media metadata,
ciphertext integrity commitment and purpose-scoped key/nonce material inside
the encrypted application event. Rust owns upload/download encryption and only
returns bounded progress, opaque handles and user-selected destination results.
The public upload manifest exposes only the explicitly budgeted routing/quota
fields; it contains no raw file key, filename or private media metadata.

Specify exact length-prefixed binary AAD and nonce bytes with fixed endian rules
and fixtures, not formatted ambiguous strings. Each per-file key/nonce index
pair is used at most once; resume reuses identical encrypted chunk bytes or starts
a fresh key/object. Chunk indexes/count/size and total length are checked before
allocation. Reject reorder, duplicate substitution, truncation, append and final
manifest mismatch. Do not present a complete downloaded file before final checks.
Do not load 5 GiB plaintext into WebView or buffer a whole upload for signatures.

## 6. Vault and recovery key domains

| Key/material | Proposed scope | Recovery/export rule |
|---|---|---|
| Live vault DEK | Independent random 256-bit key per vault generation | Wrapped for normal local unlock only; never wrapped by threshold recovery R or included with recoverable live ratchet backups. |
| OS-vault secret | Independent 32 bytes, deployment/account/device/purpose alias | Retrieved from the OS vault. Loss locks the existing live vault; do not silently treat it as a new vault. |
| Local passphrase-derived P | Argon2id v0x13, 65536 KiB, t=3, p=4, output 32 bytes, random salt at least 16 bytes | A local unlock credential, distinct from anonymous account login. Not copied to backend, logs or WebView after derivation. |
| Recovery R | Independent random 256-bit wrapping key, authenticated 3-of-5 shares | Wraps only account restoration material and a separate archive key. Never live DEK, epoch/ratchet state or active device private keys. |
| Archive DEK | Independent key per retained-archive policy/generation | Can be wrapped under R; archives disclose retained historical plaintext to a successful authorized recovery. |
| Active device/MLS secrets | Device identity, epoch/group/exporter secrets, sender/receiver ratchets, consumed KeyPackages | Excluded from recovery archive allowlist. New identity/leaf and verified admission after recovery. |

Proposed normal unlock:

```text
P = Argon2id(local_passphrase, passphrase_salt, persisted_bounded_parameters)
KEK = HKDF-SHA256(IKM = OS_secret[32] || P[32], salt = wrap_salt,
                  info = canonical("GhostChat vault wrap v2", deployment,
                                   account, device, vault_generation))
wrapped_DEK = AEAD_Seal(KEK, unique_nonce, DEK, versioned_envelope_AAD)
```

`VaultEnvelopeV2` includes version, deployment/account/device/vault/generation
binding, KDF algorithm/parameters/salts, wrapping algorithm, nonce and ciphertext.
Persist envelopes/parameters, not raw KEK/DEK. Bound parameters before Argon2 to
prevent imported-envelope resource exhaustion; bound derivation concurrency.
Implement SQLCipher cipher-version/wrong-key tests separately: its own KDF is not
the external wrapping layer. OS-only automatic unlock, if selected later, is a
different explicit policy and must not inherit the two-factor local unlock claim.

`RecoveryManifestV2` binds recovery-set ID, account/deployment, generation, format,
3/5 threshold/count, allowed restored capabilities, archive envelope digest and
manifest verification key. Its verification-key fingerprint must be anchored in
a trusted channel/checkpoint outside the replaceable recovery kit.
`ShareEnvelopeV2` authenticates manifest digest, generation, unique index and share
bytes. Use reviewed Shamir and signature/envelope libraries; no custom field
arithmetic. A MAC based only on reconstructed R cannot authenticate an entirely
replaced kit. Confidential delivery to authenticated independent custodians is a
separate requirement from share integrity. Five copies in one trust domain are
not five independent custodians.

Proposed recovery API separates `inspect_recovery_set`, `collect_authenticated_share`,
`reconstruct_identity_authorization`, `import_archive` and `enroll_fresh_device`.
The result cannot be passed into an API named restore-live-MLS-state. A recovery
request never needs the lost OS secret. Whether recovery additionally requires a
passphrase must be explicitly ratified in the format/policy; do not silently
default it from normal vault unlock behavior.

All ten valid 3-share subsets must work; mixed generations/accounts/set IDs,
duplicate indexes, corrupt signatures/shares and wrong manifests must fail.
Fewer-than-threshold secrecy comes from the reviewed construction and assumptions,
not merely observed failed test reconstruction. Three colluding custodians and
offline lack of a trustworthy freshness checkpoint remain explicit residuals.
Archiving intentionally preserves history and does not preserve deletion-based
historical confidentiality for that retained copy. Never restore old sender state.

## 7. Storage migration, activation and deletion contracts

Use actual cipher-enabled SQLCipher with authenticated pages, not ordinary SQLite
accepting an ignored `PRAGMA key`. Keep the field codec labelled separately and
define whether V2 retains it as defense in depth. Its current process-lifetime
global key cache needs a reviewed vault lock/account-switch lifetime policy.

Proposed migration states: `INVENTORIED -> STAGED -> COPIED -> VERIFIED ->
ACTIVATED -> RETAINED_LEGACY` with `FAILED_RECOVERABLE` and separately authorized
`PURGE_PENDING -> PURGED_LOGICALLY`. A persisted migration record contains source
and destination vault IDs/generations, record counts, integrity checks and active
pointer transition. No step deletes the sole known-good copy. Restart can identify
which state is active, resume/roll back staging safely and reject old plaintext
writers after protocol cutover.

Inventory WAL/journal/free pages, search, previews, attachment temp files, logs,
exports, replicas/object versions, swap and backups. Encrypting a migrated copy
cannot undo prior server/operator access. SQL DELETE, VACUUM, overwriting a file
or deleting one DEK copy cannot establish physical erasure from SSDs/backups.
Opaque random tombstones can record logical removal; raw hashes of short private
messages are dictionary-testable and are not deletion evidence.

No production migration, history purge, credential deletion or key rotation is
authorized by this document. Such work requires exact target inventory, retention
decision and the execution authority applicable to that destructive operation.

## 8. Blocking decisions and handoffs

| ID | Gap/blocker at baseline | Required owner/output before claiming capability |
|---|---|---|
| INT-B01 | Baseline lacked application ciphertext durability; current patch addresses atomic payload persistence, safe same-byte retry and insert-failure rollback only | Crypto + storage + backend: retain local regression evidence; finish startup discovery, delivery/ack state, V2 identity scope and backend conflicting-duplicate handling. |
| INT-B02 | Encryption setup/trust state partly lives in UI; ancillary plaintext routes remain | Crypto + backend + lead: Rust state machine, authenticated GroupPolicy/content union, cutover and legacy-client rejection policy. |
| INT-B03 | Field encryption currently represented by codec; no verified SQLCipher build | Storage + lead: dependency/native-library review, supported OS build matrix, wrong-key/page/WAL/temp evidence. |
| INT-B04 | Recovery is account token only; no reviewed threshold/archive protocol | Crypto + storage + lead: reviewed Shamir/signature library selection, external trust anchor, additional-passphrase decision and recovery drill. |
| INT-B05 | Shared/global storage key alias and process-lifetime cache | Storage: per-vault namespacing, first-create versus missing-key detection, lock/zeroization lifetime and migration compatibility. |
| INT-B06 | V1 attachment AAD/JS key exposure differs from V2 target | Crypto + backend: canonical manifest/AAD/nonce fixtures, streaming/resume contract, Rust-only key ownership. |
| INT-B07 | Replay-resistant server sequence/freshness and device proof absent from these crypto APIs | Backend + lead: canonical signed requests, enrollment, distributed nonce/sequence store and authenticated policy generation; client signature is not attestation. |
| INT-B08 | SQL/OS snapshots can restore an entire old local world | Lead: trusted external checkpoint/hardware policy if privileged rollback is in scope; otherwise explicit limitation. |

Media frames, TURN-only enforcement, SFU removal and updater trust roots have
separate media/SecOps contracts. An MLS exporter-purpose key proposal must specify
group/policy generation and recipients before either owner consumes it. No raw
exporter/group secrets cross the Rust/UI boundary just to satisfy an SDK API.

Acceptance handoff must attach source build ID, exact commands, fixture IDs,
expected invariants and observed results. Required classes include MLS vectors and
multi-device exchange, malicious/forked commits, outbox crash boundaries, byte-
identical retries, all share subsets/corruption, archive isolation, wrong/missing
vault credentials, SQLCipher native build, disk artifact inspection and migration
crash injection. This Stage 0 document records contracts and blockers; it provides
no runtime, deployment, independent-audit or production security certification.
