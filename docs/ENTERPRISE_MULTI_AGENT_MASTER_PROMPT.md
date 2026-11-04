# GhostChat enterprise security engineering — autonomous execution prompt

Prepared 2026-11-04 against `da5ee92`, GhostChat 4.0.44. This file is a proposed
execution contract, not a claim that its controls have been implemented.

---

## Mission and delivery standard

You are the lead engineer coordinating the GhostChat security migration. Implement
and verify the architecture below in the existing repository. Read repository
instructions and `docs/ZERO_TRUST_AUDIT_BLUEPRINT.md` first; verify actual HEAD,
working-tree changes, dependencies and runtime capabilities before planning edits.
Treat the audit as evidence to check, not as a substitute for code inspection.

Deliver reviewable changes, reproducible test results and an explicit security
case. Do not describe the result as mathematically invulnerable, “11/11”, zero-risk
or production-certified. Name properties, assumptions, evidence and residual risks.
An algorithm list or passing unit tests is not a proof or independent audit.

Follow existing user authorization and execution-environment permission rules.
Preserve unrelated changes and data. Do not buy certificates/services, add billing,
upgrade a free plan, change the VDS operating system or provision new paid hosts.
Required paid/external prerequisites become named release blockers. Never quietly
weaken a security requirement to work around a missing prerequisite.

Source-code retirement of plaintext paths is required. Deleting user history,
backups, credentials, production databases or deployed storage is a separate,
destructive operation: prepare a precise migration/deletion plan and obtain any
required authorization before executing it. Never use a broad recursive delete.
Do not reproduce disclosed secrets in source, prompts, logs, fixtures or reports.

## Baseline repository map

- `crates/desktop/src/crypto.rs`: existing MLS implementation and encrypted codec.
- `crates/desktop/src/crypto_attachments.rs`: chunked attachment encryption.
- `crates/desktop/src/main.rs`: Tauri commands, OS credential and updater integration.
- `crates/server/src/lib.rs`: HTTP/WS authentication, authorization, rate limiting,
  persistence and LiveKit token/removal operations.
- `crates/desktop/ui/voice-client.js`: direct-call peer connection.
- `crates/desktop/ui/group-voice-client.js`, `ghost-share-client.js`: LiveKit media.
- `crates/desktop/ui/watch-party.js`, `main.js`: shared playback and UI integration.
- `.github/workflows/release.yml`, `crates/desktop/tauri.conf.json`: release/update policy.

The requested `src/crypto/mls.rs` does not presently exist at repository root.
Refactor existing code into sensible crate-local modules only after mapping call
sites. Do not create a parallel unused implementation to satisfy a filename.

## Team, ownership and scheduling

Lead responsibilities: scope, threat model, interface contracts, work assignment,
integration, independent adversarial review, release gates and truthful reporting.
Workers must not approve their own security-sensitive changes as final evidence.

| Specialist role | Ownership and output |
|---|---|
| agent_crypto_core | MLS state machine, device/group policy, ciphertext outbox, isolated recovery protocol |
| agent_secure_storage | SQLCipher integration, wrapped keys, OS vault abstraction, migration/recovery storage |
| agent_backend_network | Request proofs, authorization, replay protection, Redis resource limits |
| agent_webrtc_sfu | Relay enforcement, media E2EE, session revocation/reconnect behavior |
| agent_secops_ci | Protected builds, Authenticode, signed release metadata, anti-rollback |

When limited to four concurrent agents, keep lead plus at most three workers.
Use waves; do not assume five specialists can run concurrently. Initial wave:
Crypto, Backend, Storage. Next wave: Media, SecOps, independent reviewer.
Change wave order as dependencies require; use read-only research in parallel.

Before any edit, assign disjoint files or explicit worktrees. `crypto.rs`,
`main.rs`, server `lib.rs`, Cargo manifests/lock and release configuration are
shared integration surfaces: a single named owner edits each at any time.
The lead owns dependency/lock/version changes and merges worker patches. No
worker uses blanket staging, destructive reset, force push, production migration
or secret operations. Stop only the affected task when a prerequisite is missing;
continue independent authorized work without duplicating completed work.

Every task handoff must specify: base revision, owned files, inputs/outputs,
invariants, tests, dependencies and non-goals. Workers report exact changed files,
commands/results and limitations; source comments are not verification evidence.

## Non-negotiable semantic corrections

1. MLS message ratchets and epoch transitions are distinct. Use RFC 9420 message
   key evolution; commit for membership/security events under a documented update
   policy. Do not add an epoch commit per message without demonstrating why it is
   necessary and how concurrent commits/offline members/crashes are handled.
2. Mandatory E2EE applies to private application content. Public bootstrap values,
   routing headers and transport addressing require a documented exposure budget.
   Do not promise all network metadata disappears inside TreeKEM ciphertext.
3. Recovering identity cannot silently resurrect erased historical ratchet keys.
   Identity, optional archive and live-session recovery are separate protocols.
4. Signatures establish key possession, not binary attestation. A malicious client
   holding a legitimate key remains an adversarial protocol participant.
5. The server validates authorization, framing and state; clients validate MLS
   signatures/commits/transcripts. Never upload group secrets so the server can
   inspect or “approve” encrypted application semantics.
6. Membership removal commits a durable revocation event and calls the SFU's
   authenticated RoomService API. Calling this operation a webhook does not give
   atomic or immediate delivery. A notification webhook may report results.
7. Token expiry controls initial admission; it is not an active-session timeout.
   Account/device invalidation, SFU disconnection, rejoin rejection and media
   key rotation are separate controls with explicit outage behavior.
8. TURN-only prevents direct peer candidate exposure under a tested threat model;
   it does not hide the endpoint IP from relays/providers or arbitrary websites.
9. SQLCipher requires authenticated page encryption in addition to AES-CBC.
   `zeroize` does not prove deletion from storage, swap, snapshots or prior copies.
   Neither predictable hashes nor database overwrites prove SSD erasure.
10. Authenticode and Tauri update signatures are distinct. EV does not guarantee
    SmartScreen suppression. No synthetic downloads or undisclosed user telemetry
    to “build reputation”; handle publisher reputation through legitimate signed
    distribution and optional, explicitly configured operational reporting.

## Stage 0 — security contract and dependency map

Create versioned ADRs for: protected content classes; metadata exposure; identity
binding; recovery; epoch/update policy; distributed replay/limits; media revocation;
release trust roots. Record endpoint compromise, malicious delivery service,
malicious authorized members, rollback, provider outages and resource exhaustion.

Inventory every transmission/persistence path including reactions, edits, replies,
pins, profiles, presence, notifications, file names/previews, Watch Party sources,
exports, logs and media keys. Assign one cryptographic policy to each. Gate old
client versions by authenticated protocol policy, not a cosmetic version dialog.

Inventory deployment prerequisites without exposing secrets. The old VDS runtime,
free-tier media quotas and missing signing infrastructure cannot be hidden behind
an “enterprise ready” label. LiveKit credential rotation previously requested must
be verified operationally; never assume the user completed it.

Produce `SECURITY_REQUIREMENTS.md`, `SECURITY_INTERFACES.md`,
`SECURITY_ACCEPTANCE_MATRIX.md` and a machine-readable evidence manifest. Define
contracts before shared-file implementation. Avoid new dependencies until license,
maintenance, compatibility and build requirements have been inspected.

## Phase 1 — mandatory MLS and authenticated application policy

Crypto owner implements a Rust-owned conversation state machine:
`UNINITIALIZED -> INVITED -> VERIFIED -> ACTIVE -> REKEYING -> ACTIVE`, with
`LOCKED` and `REMOVED` terminal/recovery paths. Only ACTIVE publishes application
content. Missing keys, unverified identity change, replay, epoch mismatch or vault
failure must never invoke a plaintext fallback.

Use the existing compatible OpenMLS stack, pinned and checked against official
vectors/interoperability tests. Preserve its declared cipher suite honestly; the
existing AES-128-GCM MLS suite and AES-256-GCM attachment/storage layers are
different constructions. Do not hand-write TreeKEM or require the server to decrypt.

Specify an authenticated application-policy object binding group ID, device roster,
Owner/Admin/Member roles, policy version and change authority. MLS membership alone
does not assign application administrator powers. Reconcile server RBAC with this
policy; reject role/roster disagreements and unauthorized commits at endpoints.

Each group has one ordered cryptographic worker. Encrypt once, then atomically
persist updated MLS sender state and an immutable ciphertext outbox entry before
network send. Retries resend identical bytes for the same message ID. A conflicting
reuse of a message ID is an error. Commit state/outbox delivery and acknowledgments
must survive interruption at every boundary without nonce/key reuse.

Define concurrent-commit ordering, parent epoch binding, bounded pending proposals,
fork detection/quarantine, duplicate delivery, welcome handling, key-package
single-use semantics and offline catch-up. Authenticate the group chain and reject
rollback against trusted checkpoints. Do not claim an untrusted local counter is
rollback-resistant against an administrator restoring the whole endpoint.

Encrypt attachment manifests and private auxiliary events under the content
policy. Use fresh per-file keys; indexed AEAD chunks bind object, conversation,
version, size and index through AAD. Define exact nonce construction, key lifetime,
chunk bounds, truncation/append detection, resume behavior and final verification.
Keep large plaintext out of WebView memory where possible. Notifications must not
send plaintext content to external services by default.

Gate: malformed/duplicate/out-of-order/future commits; concurrent writers; removed
member; changed device identity; crash after ratchet persistence before send;
duplicate retry; attachment tamper/reorder/truncation; every plaintext route fails
closed for the new protocol. Include multi-device real MLS exchange, not only mocks.

## Phase 2 — isolated recovery, SQLCipher and migration

Define independent recovery domains and cryptographic keys:

- Identity recovery restores authorization to enroll a new device; it does not
  resume an old MLS leaf or silently impersonate its live ratchet state.
- Archive recovery, if enabled, restores explicitly retained encrypted history.
  Its retention/backup key policy is disclosed separately from forward secrecy.
- Live-session recovery uses a new verified device leaf and fresh group admission;
  stale snapshots cannot restart an old sender state or restore erased epochs.

Use an audited 3-of-5 Shamir implementation for a random identity-recovery wrapping
key, not a password or the evolving MLS state. Version and authenticate shares,
backup-set ID, generation and index against an authenticated recovery manifest.
The recovery key may wrap identity-restoration material and a separate archive
key; it MUST NOT wrap the live MLS database DEK or a backup containing live MLS
ratchets. Archive exports use an explicit allowlist excluding epoch/group/exporter
secrets, sender/receiver ratchets, active device private keys, consumed KeyPackages
and live protocol outbox state. Anchor the recovery manifest's verification-key
fingerprint outside the replaceable recovery kit. A MAC keyed only by the
reconstructed recovery key does not independently authenticate a replacement kit.
Keep a local-vault passphrase distinct from passwordless account login.
Define peer authentication, share delivery, revocation/re-sharing, corruption,
duplicates, peer unavailability and three-peer collusion. Test all ten valid
three-share subsets. “Two shares did not recover in a test” is not a secrecy proof;
rely on the reviewed construction and explicitly stated distribution assumptions.

SQLCipher integration must use the actual cipher-enabled native library, tested
on supported Windows builds. `PRAGMA key` on ordinary SQLite is not encryption.
Use full page encryption/authentication plus encrypted attachments/temp policy.
Generate random per-vault DEKs; use domain-separated KEKs to wrap them.

Baseline password KDF: Argon2id v0x13, memory 65536 KiB, t=3, p=4, output 32 bytes,
random salt >=16 bytes. Combine KDF output with independent OS-vault secret using
a documented HKDF construction; AEAD-wrap DEKs with account/device/version-bound
AAD. Store KDF parameters/salts/envelopes, not raw keys. Decide explicitly whether
identity recovery requires an additional passphrase. Recovery cannot require a
lost OS secret that it is supposed to replace.

Support Windows Credential Manager; add a fail-closed SecretService implementation
only if Linux is in the release support matrix. Namespace aliases by deployment,
account/device and purpose. Benchmark KDF memory and cap concurrent derivations;
bound imported parameters against resource exhaustion. Zeroize owned secret buffers
and audit accidental copies, panic paths, library lifetime and WebView exposure.

Implement a separate resumable migration tool with inventory, dry run, backup
policy, staged encrypted copy, integrity/count checks, recovery verification,
atomic activation and crash recovery. No irreversible purge before validation.
Retire plaintext source paths after protocol cutover; do not delete installed
users' historical data just because a code path was retired.

Replace legacy row references with opaque tombstones if required. If audit linkage
needs HMAC, use a separately controlled key; never expose a plain hash of short
messages as proof of deletion. Cover WAL, journals, caches, previews, search,
exports, replicas, object versions and backups. SSD/backup erasure is an explicit
residual unless storage-layer guarantees and key destruction justify the claim.

Gate: wrong/missing vault key, wrong passphrase, all recovery combinations,
mixed generations, corrupt shares, rollback checkpoint mismatch, key rotation,
database/WAL/temp inspection, crash at each migration boundary, and restored
identity unable to resurrect deleted live-session keys.

## Phase 3 — device proofs, server state and Redis

Backend owner extracts focused crate modules, proposed names:
`crates/server/src/auth/device_proof.rs`, `auth/authorization.rs`,
`auth/replay.rs`, `ratelimit.rs`; integrate with existing Axum handlers.

Define a domain-separated canonical signed request format and test vectors.
It includes version, deployment audience, device/session ID, method, canonical
target, content digest, timestamp and unique request ID. Reject ambiguous path,
query, duplicate headers and body transformations. State exact signed bytes;
never rely on incidental JSON object ordering. Device key binding requires a
verified enrollment/replacement protocol. Define bootstrap exceptions explicitly.

Authenticate session/key, atomically reject replay across all workers, then
authorize operation against current membership, role and state generation.
Signatures do not exempt clients from bounds/schema/state checks. Bound parsing
before expensive crypto; resource limits apply before account creation/auth too.
Specify WS upgrade and per-frame proof/sequence handling, expiry, revocation,
queue bounds and reconnect behavior. Sensitive errors must not become key oracles.

Large uploads use a signed bounded manifest plus authenticated chunks; do not
buffer a 5 GB request merely to compute/verify one signature. Retry identity,
idempotency, billing/quota counters and upload-session expiry are explicit.

Implement bounded Redis sliding-window policies with atomic scripts/functions,
expiry and compatible cluster hash slots. Specify whether the algorithm is an
exact log or approximate counter and its error bound. Rate limits include account,
device, source network, room, upload bytes and global expensive operations.
Prevent unbounded attacker-created limiter keys. Trust forwarded addresses only
from configured proxies; never accept spoofed headers from arbitrary clients.

Define Redis restart/persistence/replication/failover loss assumptions. Sensitive
admission fails closed on replay-store/limiter failure; other degraded paths have
explicit finite budgets. Restart survival is tested, not inferred from “Redis”.
Upstream DDoS, botnet identities and provider availability remain separate from
application quotas. Include KDF/signature verification concurrency bounds.

Gate: valid signature with wrong role; legitimate malicious client; nonce replay
across workers; changed body/target/audience; revoked open WS; Redis unavailable,
failover, restart and memory pressure; spoofed proxy headers; oversized/malformed
payloads; unauthorized MLS envelope mutation; bounded CPU/memory under load.

## Phase 4 — media confidentiality, relay policy and revocation

Media owner enforces `iceTransportPolicy: "relay"` and corresponding LiveKit
configuration on all direct/group/GhostShare/reconnect paths. Reject startup when
authenticated TURN cannot meet the policy; no silent `all` or P2P fallback.
Retain source selection consent and cleanup. Test actual selected candidates and
packet flows under network changes, not only the presence of a config string.

TURN hides direct peer candidates, not IPs from infrastructure operators. Watch
Party links can expose IP addresses to the video host; peer-controlled URLs and
direct resource loads require an explicit policy, consent, allowlist or trusted
relay/proxy. Do not claim network anonymity while allowing arbitrary remote loads.

If confidentiality against the SFU is required, implement compatible application
frame E2EE for audio and screen tracks. Distribute independent purpose-bound media
keys through authenticated MLS state; rotate on membership/device changes. Verify
browser/native SDK interoperability and behavior when a recipient lacks keys.
SFU/TURN transport encryption alone is not media E2EE. Routing metadata persists.

For removal/revocation:
1. Authenticate and authorize the membership change; commit a revocation generation
   and disconnect event in the same durable transaction/outbox as server state.
2. Deny new token issuance and device/session requests immediately after commit.
3. Invoke authenticated `RoomService/RemoveParticipant` for all active identities
   of the removed account/device. Use service-specific token revocation parameters
   where supported; webhook delivery is not the enforcement primitive.
4. Invalidate the cooperative client's state/streams; do not depend on it for
   enforcement against a malicious client.
5. Rotate media E2EE keys for remaining members; verify old devices cannot decrypt
   newly emitted frames. Already delivered content cannot be revoked.
6. Retry idempotently, verify SFU removal, reconcile missed events, surface failure
   and define fail-closed publication/admission when revocation cannot be enforced.

A 30-second token is an initial admission lifetime target, not a disconnection
guarantee. Handle clock skew, refresh and reconnect. Test Cloud and self-hosted
token-reuse behavior separately. Specify measured p95/p99 revocation objectives
and a bound under stated healthy-network assumptions; declare partition behavior.

Gate: removed-user rejoin using original/refreshed token; multiple devices;
concurrent join/removal; SFU timeout/partition; queued outbox after backend restart;
key rotation; old encrypted frame replay; ICE fallback; missing TURN; reconnect;
third-party media load IP policy. No complete native-capture rewrite is implied
by this security phase; that is a separate media implementation track.

## Phase 5 — supply chain, Authenticode and anti-rollback

SecOps owner pins reviewed actions to commit SHAs, isolates untrusted PR jobs from
release secrets and uses least-privilege workflow permissions/protected tags and
environments. Generate SBOM, reproducible locked-build records, dependency review
and verifiable provenance. Record toolchains and source/artifact hashes.

If approved certificate/signing infrastructure exists, sign installers and shipped
executables with Authenticode and trusted timestamps; validate identity, chain,
timestamp and artifact coverage before upload. Use protected non-exportable keys
where supported. Never put keys in git or untrusted jobs. Without prerequisites,
produce explicitly development-only artifacts; production signing remains blocked.
Do not purchase EV certificates or claim they bypass SmartScreen automatically.

Keep Tauri updater verification enabled. Add an explicit signed-metadata trust
model: deployment/channel/platform/version, artifact hash/length, monotonic metadata
sequence, expiry, minimum supported version, delegated keys and rotation/recovery.
Use a reviewed update framework/design where feasible rather than inventing
cryptography. Define offline root authority and signing-key compromise response.

Enforce highest-seen release/metadata policy and reject downgrade/replay/freeze
under documented trust assumptions. A local administrator restoring both software
and state is outside a simple file-based rollback guarantee. Protect local state
and/or validate an external trusted checkpoint if that adversary is in scope.
Interrupted update must recover without installing unverified bytes or wiping keys.

Gate: wrong/compromised signing key, valid old artifact, expired signed metadata,
clock skew/rollback, wrong channel/platform, truncated package, missing signature,
interrupted installation, certificate expiry/timestamp validation, key rollover,
untrusted PR secret access and CI provenance verification. No unsigned production
release on signing failure. No user telemetry without explicit policy/consent.

## Integration, evidence and release decision

The lead maintains a requirement-to-code-to-test map. Review every encryption,
authorization and state transition change independently before merging.
Unit tests, cross-process integration, multi-device MLS/media tests, fuzzing and
crash/fault injection must match the requirement; mocks alone cannot prove real
network, platform, keystore or provider behavior. Avoid tests that merely assert
the same constant exists in implementation and fixture.

For formal claims, state the model and prove scoped invariants (for example
single-use outbox transition or replay rejection) using appropriate modeling and
review. Separate proof of an abstract state machine from correspondence to Rust,
SQL, Redis scripts and actual deployments. Standard cryptographic assumptions
remain assumptions, not newly established mathematical facts.

STRIDE review records controls and residuals for each boundary. Authentication
does not replace authorization; AEAD does not stop an authorized malicious sender;
MLS transcripts do not automatically provide legal non-repudiation; Redis does
not guarantee availability; encrypted archives change historical confidentiality.

Release only when each required gate has verifiable evidence, independent
cryptographic/design and penetration review is complete, and no unresolved
critical/high finding remains. Missing external prerequisites block only their
claimed production capability; they must not be marked complete or silently
removed from the acceptance matrix.

At every handoff report:
`implemented / tested locally / tested across real devices / deployed / blocked`.
State exact evidence and remaining risks. Never equate a git push with successful
CI, signed release availability, installed VDS configuration or validated service.

Final deliverables: code, protocol/ADR documents, migration dry-run report,
recovery drill, STRIDE/residual-risk register, test evidence manifest, SBOM/build
provenance, signing/revocation runbooks, rollback/recovery runbooks and release
go/no-go decision. Keep all secret values out of these artifacts.

## Reference anchors to verify before implementation

- https://www.rfc-editor.org/rfc/rfc9420.html
- https://www.rfc-editor.org/rfc/rfc9106.html
- https://www.zetetic.net/sqlcipher/design/
- https://docs.livekit.io/frontends/reference/tokens-grants/
- https://v2.tauri.app/plugin/updater/
- https://learn.microsoft.com/windows/apps/package-and-deploy/smartscreen-reputation

---

End of proposed execution prompt. Do not treat its presence in a repository as
authorization to execute it: execution must be requested by the user.
