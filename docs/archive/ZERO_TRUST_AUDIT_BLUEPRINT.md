# GhostChat security audit and implementation blueprint

Date: 2026-11-04. Inspected revision: `da5ee92` (4.0.44).
Scope: static source/configuration review and protocol-design review. This is
not a penetration test, deployed-server inspection, cryptographic proof or
certification. No application code, credentials, messages or database files were
modified during this audit. Claims below distinguish existing code from targets.

## Decision

The submitted specification is not an achieved security guarantee. The current
application must not be labelled “zero doubt”, “bulletproof” or enterprise-E2EE
on the basis of this review. Implementation can proceed after the contradictions
below are resolved in the security policy. No honest architecture eliminates
compromised endpoints, traffic analysis, operator outages or all denial of service.

## Evidence and gaps

| Control | Current source evidence | Result |
|---|---|---|
| MLS | `crates/desktop/src/crypto.rs`: OpenMLS 0.9.2, `encrypt_text`, membership commits | Present for configured encrypted chats; not universal |
| Cipher suite | `crypto.rs`: X25519/AES-128-GCM/SHA-256/Ed25519 MLS suite | Valid distinct suite; do not describe it as AES-256-GCM MLS |
| Plaintext policy | `crates/server/src/lib.rs`: `send_message` rejects plaintext only when conversation crypto lock exists | Mandatory application-wide E2EE absent |
| Attachments | `crypto_attachments.rs`: chunk AEAD, per-file keys and nonce prefix/index, manifest | Encrypted path present; legacy/non-encrypted paths remain |
| Metadata | Server stores/routs identities, membership, presence; Watch Party state is relayed as JSON | All-metadata E2EE claim false |
| Local persistence | `EncryptedJsonCodec`: AES-256-GCM entities, HMAC lookup references; Windows credential storage key | Field encryption present; not SQLCipher full-database encryption |
| Key recovery | `main.rs`: OS credential storage and account recovery functions | No 3-of-5 encrypted-state recovery found |
| Erasure | `zeroize`/`Zeroizing` used in local crypto code | Useful partial memory hygiene, not global erasure proof |
| Epoch policy | `encrypt_text` uses `create_message`; membership paths merge commits | No explicit self-update commit on every message |
| Request authentication | Backend `authenticate`: hashed bearer-session lookup | No universal device-signature middleware |
| Rate limiting | `allow_rate`: process-local mutex/hash-map/deques | Lost at restart, not shared across workers; Redis absent |
| IP policy | `voice-client.js`: relay-only preference otherwise `all`; SFU wrappers separate | Universal enforced relay-only absent |
| Media revocation | Backend calls ListParticipants and RemoveParticipant after group changes | Best effort, not guaranteed instantaneous; 10-minute join tokens |
| Media E2EE | JS LiveKit rooms do not configure application frame encryption | SFU can access media; TLS alone is insufficient |
| Supply chain | GitHub workflow uses locked builds and Tauri updater signing secret | No Authenticode signing step/configuration found; actions use tags |
| Update replay | Tauri updater public key and version checks present | No complete signed-metadata expiry/high-water rollback policy found |
| Other OS keyrings | Non-Windows storage path returns unsupported error | SecretService support absent |

Line anchors can shift; inspect the named functions before editing. Library
presence proves neither RFC interoperability nor end-to-end integration safety.

## Corrected guarantees and threat boundary

1. Confidential application content must be encrypted before network delivery.
   Specify content classes: text, attachments/manifests, reactions, replies,
   profile fields, Watch Party URLs/state, and private group properties.
   Public bootstrap keys, protocol headers, routing IDs and transport addressing
   cannot all be hidden inside an already established MLS epoch. Publish a
   metadata exposure inventory instead of claiming zero metadata leakage.
2. MLS uses a message secret tree within epochs, and TreeKEM updates group key
   agreement across commits. Sending application ciphertext is not itself an
   epoch transition. If per-message commits remain a hard requirement, serialize
   each update+message pair and prove recovery under concurrent commits. Prefer
   the RFC message ratchet plus explicit membership/security-triggered updates.
3. Backups that preserve old ratchet state also preserve decryption capability.
   Identity recovery, archive recovery and active MLS state recovery are separate
   capabilities. Recover identity and optional encrypted archives, then create a
   new device leaf and rejoin; never resume a stale sender ratchet from backup.
4. A valid Ed25519 signature proves possession of a key, not an unmodified app.
   Validate all signed inputs, authenticate device-to-account bindings, and apply
   authorization independently. Do not name signatures “remote attestation”.
5. Relay-only hides direct peer network candidates, not IP addresses from TURN,
   SFU, hosting provider or destination sites. It does not encrypt SFU content.
6. Token expiry governs joining, not an existing media session's lifetime.
   Removal is an authenticated RoomService RPC, not a membership webhook.
   Cloud/self-hosted token revocation behavior must be tested separately.
7. EV/AuthentiCode establishes publisher identity; it cannot guarantee absence of
   SmartScreen warnings. Updater signatures do not alone prevent replay/freeze,
   signing-key compromise, vulnerable signed releases or privileged local rollback.

## Target architecture

### Client cryptographic boundary

Keep private signing keys, MLS processing, plaintext serialization, attachment
encryption and durable outbox operations in Rust. The WebView is a rendering
boundary with minimal plaintext lifetime, no arbitrary remote script execution
and scoped Tauri commands. A compromised authorized endpoint can still access
content it legitimately receives; no client-signature scheme changes this.

Use an audited compatible MLS implementation, pinned dependencies and official
interop vectors. Keep sender/device identity bindings and roster authorization
within a versioned application policy authenticated alongside the MLS group.
The server's Owner/Admin/Member SQL field alone is not cryptographic authorization.

Conversation state machine:
`UNINITIALIZED -> INVITED -> VERIFIED -> ACTIVE -> REKEYING -> ACTIVE`.
Revocation/removal leads to `REMOVED`; missing keys or conflicting state leads to
`LOCKED`. Only `ACTIVE` can publish application data. Setup failure, unknown
device, storage unlock failure or epoch mismatch must never choose plaintext.

For each conversation use one ordered cryptographic worker. Atomically persist
the new sender state and ciphertext outbox before transmission. A retry sends
the same ciphertext bytes for the same message ID, not a second encryption.
Use bounded future-epoch buffering; commit delivery precedes dependent data.
Duplicate, forked, malformed and removed-member commits must be rejected or
quarantined without replacing the current trusted state. Preserve proof material
needed to diagnose forks without logging private content.

### Recovery and encrypted storage

Generate independent random 256-bit database DEK and recovery wrapping key R.
Split R with an audited 3-of-5 Shamir implementation and independent randomness.
Each share envelope includes version, backup-set ID, share index, threshold,
creation generation and an authenticated binding to the recovery manifest.
Authenticate shares; plain Shamir alone does not detect forged contributions.
No two copies in the same trust/failure domain count as independent shares.

Ideal Shamir property for a uniform secret: any set of fewer than three shares
reveals no information about R. This does not prove safe distribution, peer
availability, implementation correctness, or resistance to three colluding peers.
No invented polynomial/field implementation. Test every 3-of-5 combination,
duplicate indexes, mixed generations and corrupted shares.

For the normal storage unlock path define:

`P = Argon2id(passphrase, random_salt, v=0x13, m=65536 KiB, t=3, p=4, out=32)`

`KEK = HKDF-SHA256(OS_secret || P, salt=wrap_salt, info=app/account/device/version)`

AEAD-wrap the random DEK under KEK; a separate recovery envelope wraps the DEK or
archive key under R. OS secret is retrieved from the OS vault, not stored beside
the database. Persist salts and KDF parameters, not raw KEK/DEK. Define separately
whether passphrase is additionally required for recovery: it must not be assumed
implicitly. Keyring loss must be recoverable without requiring that lost keyring.

Argon2 settings above are a baseline, not an absolute strength guarantee. Limit
concurrent derivations, benchmark intended devices and enforce safe upper bounds
when loading attacker-controlled KDF parameters. Store versioned parameters.

Use genuine SQLCipher AES-256-CBC + per-page authentication, not SQLite with a
`PRAGMA key` that the linked library ignores. Validate cipher version and wrong-key
failure in CI. Encrypt MLS state, drafts, profiles, search data and outbox; audit
WAL, journal, temp files, attachments and crash dumps separately. SQLCipher's own
passphrase KDF is distinct from the external Argon2/wrapping scheme.

Recovery imports archives into a fresh device identity and re-verifies/rejoins
groups. Include archive generation and account binding; detect old/inconsistent
backup sets using trusted peer state. Offline recovery cannot guarantee global
freshness without an external trusted checkpoint. Old recovery sets remain a
risk until replaced keys are no longer capable of decrypting retained archives.

`zeroize` protects owned memory buffers best-effort. Audit copies, library caches,
WebView strings and serialization errors. Process termination, swap, snapshots,
SSD remnants and backups are outside a simple zeroize guarantee. Long-lived
master-key encryption of historical MLS snapshots must not defeat intended FS.

### Backend, signatures and resource control

Introduce `src/auth/device_proof.rs`, `src/auth/nonce_store.rs`,
`src/auth/authorization.rs`, `src/ratelimit.rs` and a versioned policy module.
First authenticate the account/device binding; then authorize the action.

Sign a domain-separated deterministic envelope containing protocol version,
deployment audience, device/session ID, method, canonical path/query, body digest,
timestamp and unique request ID. Specify canonicalization and test vectors; a
naive JSON string reserialization is not a wire specification. Reject duplicate
nonce/request IDs atomically across all workers; define expiry and clock skew.
Bind proof to session to prevent moving signatures between accounts/servers.

Do not buffer a 5 GB upload to verify its signature. Authenticate a bounded signed
manifest and use indexed AEAD chunks with object/conversation IDs and lengths in
AAD. Reject altered, duplicated, reordered, missing and appended chunks. Verify
the final manifest before presenting a completed file. Define nonce uniqueness
per key, chunk count limits and resumable-upload state transitions.

WebSocket upgrade authenticates once, then frames carry bounded sequence numbers
and device/session binding. Recheck expiry/revocation while connected. Bootstrap
endpoints require an explicit signature-exception policy; before enrollment a
trusted device signing key does not yet exist. Verify MLS objects on clients;
the delivery service cannot certify encrypted application semantics by inspecting
opaque payloads. Reject malformed framing before expensive crypto work.

Redis sliding windows must use an atomic server-side operation, bounded keys,
expiry and explicit per-account/device/IP/room/upload/global policies. Avoid
claiming a counter equals an exact sliding log. In Redis Cluster every multi-key
atomic operation must use a compatible hash slot. Use bounded scripts and memory
limits; avoid eviction silently bypassing critical authentication limits.

Define restart/failover consistency: Redis persistence and replication are not
magically lossless. On limiter failure fail closed for token issuance and other
sensitive actions; use explicitly bounded degraded behavior where availability
is required. Enforce proxy body/header/connection/time limits before expensive
verification, Argon2 concurrency limits, upload quotas, WS frame/queue bounds and
idle timeouts. A Redis cluster does not stop volumetric upstream DDoS or botnets
with many accounts; edge/network protection and capacity remain separate.

### Media security and revocation

Enforce relay-only at every client media constructor, including direct calls,
group LiveKit rooms and GhostShare rooms. Validate TURN configuration before
starting media; never silently switch to `all` or public direct capture paths.
Use expiring authenticated TURN credentials and test selected candidates using
actual stats, packet traces, reconnect and network change cases.

Require application frame E2EE for audio/screen tracks if SFU confidentiality is
part of the product promise. Distribute distinct purpose-bound media keys through
authenticated MLS state; rotate on membership changes. The SFU keeps routing
metadata. LiveKit frame-encryption and native SDK interoperability must be tested;
enabling an SFU or TURN is not the implementation of this control.

Issue least-privilege 30-second initial join tokens if operational tests permit.
The issuer validates current device/session/room membership on every request.
Record a revocation generation, deny future issuance immediately after membership
commit, and persist a transactional outbox for disconnect RPCs. Use stable opaque
session identities or enumerate every active session of the removed account.

Call RoomService RemoveParticipant with the service-supported token revocation
parameters where applicable. Retry idempotently, verify disconnection, monitor
pending revocations and reconcile periodically. Self-hosted reusable tokens and
refreshed Cloud tokens require separate tests. If SFU removal fails, rotate media
E2EE keys to deny future content; fail closed for new media publication if policy
cannot be established. Already received content cannot be revoked retroactively.
Document measured revocation latency and outage behavior, never “instant always”.

### Legacy migration and deletion

No automatic destructive purge is authorized by this audit. Prepare a separate
reviewable migration, inventory and user-visible retention decision. Migration
and deletion are different operations: encrypting an old copy cannot undo a
server/operator having already read it.

A raw hash of a short plaintext message is dictionary-testable. Prefer opaque
random tombstones; if keyed audit linkage is necessary, use a separately scoped
HMAC and document who can access the key. The proposed marker can label the row,
but is not evidence of secure deletion. Include attachment blobs, previews,
indexes, logs, exports, WAL/free pages, replicas, object versions and backups.

Create a new encrypted database, copy under a single migration transaction/state
machine, validate record counts/content and recovery, then atomically switch the
active pointer. Crash at every stage must leave one known-good state. Remove old
copies only according to an approved retention/deletion policy; SQL DELETE or
VACUUM alone cannot prove erasure from SSDs or prior backups. Track migration
generation and reject old clients writing plaintext after the cutover.

### Build, signing and updater

Pin GitHub Actions to reviewed commit SHAs; isolate untrusted PR builds from
release secrets. Use protected tags/environments, least-privilege tokens, locked
dependencies, vulnerability/license review, SBOM and verifiable provenance.
Authenticode-sign installers and relevant binaries using protected hardware/cloud
keys, trusted timestamping and CI signature/publisher validation. This has cost
and organizational prerequisites; it conflicts with promising every deployment
control is free. Never place the signing private key in source or build logs.

Keep Tauri updater artifact verification separate from Authenticode. Add signed,
expiring release metadata with channel/version/artifact hash/length/platform and
minimum supported version. Maintain an authenticated highest-seen version and
metadata sequence, define key rotation/offline root recovery, reject rollback
and expired metadata, and specify safe interrupted-install recovery. A fully
privileged endpoint attacker can roll back local storage unless an external
trusted state/hardware policy prevents it; state that boundary explicitly.

### STRIDE residuals

| Threat | Required controls | Residual boundary |
|---|---|---|
| Spoofing | Verified device bindings, proofs, replay protection | Stolen endpoint key signs valid requests |
| Tampering | MLS verification, chunk AEAD, DB HMAC, update verification | Malicious authorized member can send harmful content |
| Repudiation | Authenticated protocol transcripts and scoped audit | MLS is not automatically a legal non-repudiation system |
| Disclosure | Mandatory content E2EE, DB encryption, media E2EE | Routing/timing/size metadata, authorized endpoints, backups |
| DoS | Bounded parsing/crypto/queues, Redis, edge quotas | Upstream flood, compromised account fleets, provider outage |
| Privilege | Server RBAC + authenticated group policy and roster checks | MLS membership alone does not define Owner/Admin semantics |

## Work order and release gates

1. **P0 containment/policy:** rotate the LiveKit secret previously disclosed in
   conversation, invalidate affected access as supported, define trusted devices,
   content/metadata/media guarantees and unsupported deployment risks. Never put
   the secret in this report. Verify changes on the actual VDS; source review
   cannot establish its current environment or whether rotation occurred.
2. **P0 mandatory E2EE:** inventory all plaintext endpoints and ancillary content;
   build fail-closed state machine and ciphertext outbox; prevent old-client
   downgrade before claiming confidential-by-default operation.
3. **P1 recovery/storage:** implement and independently review share handling,
   SQLCipher build integration, wrapped keys, archive versus live-state recovery,
   migration staging and crash/rollback behavior. Do not purge history first.
4. **P1 authentication/abuse:** device proof and replay store; bounded Redis
   limiter; revocation of existing HTTP/WS sessions; load/failover tests.
5. **P1 media:** mandatory relay policy everywhere, application frame E2EE,
   transactional membership revocation and real-network validation.
6. **P1 deployment:** Authenticode, protected release workflow, signed metadata
   anti-rollback and key rotation. Separate paid certificate decisions from the
   already selected free-tier media plan.
7. **Release gate:** independent cryptographic/design review and penetration test;
   no unresolved critical/high findings, documented residual risks and reproducible
   test evidence for every guarantee. Source unit tests alone cannot satisfy it.

Acceptance matrix must cover: malicious delivery server; wrong/missing keys;
nonce replay across workers; concurrent MLS commits; crash after encryption before
send; replay after restore; every valid 3-share subset; mixed/corrupt shares;
missing keyring; wrong passphrase; SQL/WAL/temp inspection; network transition
without direct candidates; SFU partition during revocation; removed-user rejoin;
frame E2EE across old/new clients; expired or downgraded signed releases; signature
key rollover; Redis restart, memory pressure, failover and unavailable cluster.

Record exact build IDs, test fixtures, timestamps, expected invariants and observed
results. A failed confidentiality precondition must produce an explicit locked
state, not “best effort” sending. Formal claims must state model assumptions and
be reviewed for correspondence to executable code; an algorithm list is not proof.

## Primary references

- MLS protocol, key schedule, security considerations: https://www.rfc-editor.org/rfc/rfc9420.html
- Argon2id parameters and vectors: https://www.rfc-editor.org/rfc/rfc9106.html
- SQLCipher encryption/authentication: https://www.zetetic.net/sqlcipher/design/
- LiveKit token lifetime, refresh and revocation: https://docs.livekit.io/frontends/reference/tokens-grants/
- Tauri updater signing: https://v2.tauri.app/plugin/updater/
- SmartScreen/EV behavior: https://learn.microsoft.com/windows/apps/package-and-deploy/smartscreen-reputation
