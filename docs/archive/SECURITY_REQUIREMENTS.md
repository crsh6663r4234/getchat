# Security migration requirements — protocol v2 proposal

Baseline: da5ee92 / 4.0.44, inspected 2026-11-04. This is a source implementation
contract, not evidence of deployment or achieved confidentiality.

## Scope and claim boundary

Protect private content against the delivery service, network observers and
removed members for future traffic. Endpoint compromise, a current authorized
recipient copying content, traffic analysis, infrastructure availability and
administrator rollback of all trusted endpoint state remain explicit limits.
The Windows Server 2012 R2 VDS remains unchanged. No paid infrastructure,
certificate purchase, history deletion or production migration is part of this
source work. Existing production key rotation/deployment is not verified.

## Content and metadata policy

| Class | Required v2 handling | Visible routing / limitation |
|---|---|---|
| Text, edits, replies, reactions, pins | Authenticated MLS application envelope | Conversation/device IDs, sequence, size and delivery timing |
| Files, thumbnails, private filenames | Fresh per-object AEAD keys; encrypted MLS manifest | Opaque object ID, ciphertext length and transfer timing |
| Private group properties, participant roles | Authenticated versioned group policy | Server membership needed for admission and quotas |
| Watch Party URL, playback state, notes | MLS content; bounded authenticated commands | Loading a third-party URL exposes viewer IP to that host |
| Audio/screen frames | Separate purpose-bound frame E2EE keys distributed by MLS | SFU/TURN addresses, track timing, bandwidth and room routing |
| Profile/avatar | Explicit public-directory opt-in or private encrypted profile | Public discoverability cannot also imply server-hidden data |
| Presence | Minimum authorized audience; no broadcast to unrelated accounts | Server observes connections and admission |
| Notifications | Generic activity by default | No external plaintext previews |
| Local archives/exports | Separate opt-in archive keys and disclosed retention | Archive restoration is not forward-secret session restoration |

Current legacy paths do not meet this policy. A new protocol cutover must not be
advertised until every class is implemented and the acceptance matrix passes.
Never replace missing encrypted content with a legacy plaintext send.

## Decisions and invariants

1. Preserve the supported OpenMLS cipher suite; do not invent TreeKEM. MLS message
   ratchets differ from membership/security epoch transitions.
2. One ordered crypto owner per conversation; persist sender state and immutable
   ciphertext together before network publication. Message-ID retries reuse the
   exact ciphertext; mismatched scope/content fails. Legacy cache entries without
   a ciphertext record cannot safely regenerate a retry with that ID.
3. Identity recovery enrolls a fresh device/leaf. Three-of-five recovery may unlock
   identity authorization and a separate archive key, never live MLS storage keys.
   Recovery shares require an independently anchored manifest identity.
4. SQLCipher must be a real linked cipher library, proven with cipher-version,
   wrong-key and database/WAL/temp inspection. Existing encrypted fields do not
   satisfy full-database encryption. No live-data migration during development.
5. Ed25519 proofs bind canonical requests to enrolled keys; they are not binary
   attestation. Authorization, replay state and malicious-member validation remain
   mandatory. Clients validate encrypted MLS semantics; server never gets secrets.
6. Redis counters/replay admission require atomicity, bounded memory and explicit
   persistence/failover assumptions. Unavailable sensitive admission fails closed.
7. All media paths require authenticated relays with verified selected candidates.
   Missing relay configuration must surface a clear unavailable state.
8. Durable removal events deny admission, invoke authenticated SFU removal for all
   device identities, retry/reconcile and rotate media keys. Token expiry alone
   does not terminate an existing participant.
9. Authenticode and updater signatures use independent trust purposes. No paid
   signer is assumed. Signed metadata freshness, sequence, platform/channel and
   compromise recovery are required before strengthened update-security claims.

## Dependencies and cutover gates

Implementation can proceed locally with temporary test data. Protocol rollout
depends on device enrollment, compatibility negotiation and UI locked-state support.
Storage integration depends on Windows-native SQLCipher build compatibility and
reviewed recovery/key-wrapping design. Distributed admission depends on an actual
Redis deployment and failure policy. Relay/frame confidentiality depends on real
LiveKit/TURN SDK capability and free-tier capacity verification. Production signing
depends on existing approved credentials/infrastructure; absence is a release
blocker, never permission to buy a service or label unsigned binaries production.

Do not delete historical plaintext until a separately scoped migration has backup,
dry-run, integrity checks, rollback and retention authorization. SSD, snapshots,
recipient copies and backups prevent a universal erasure claim.

## Evidence rules

Record exact revision, command, platform, result and test scope in
security-evidence.json. Distinguish proposed, implemented, locally tested,
multi-device tested and deployed. Do not mark mock tests as deployment evidence.
Independent crypto/design review and penetration review remain release gates;
multi-agent review alone is not independent external certification.
