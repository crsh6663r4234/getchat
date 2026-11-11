# Security acceptance matrix

Observed 2026-11-04 (Europe/Istanbul), baseline `da5ee92a3ecaafd2d3d73a62d91f6854258f74a4`, app 4.0.44.
Scope: local source inspection and the explicitly recorded local tests. This is
not deployment evidence, penetration testing, independent review or certification.
Concurrent changes after this snapshot require re-evaluation; a baseline gap is
not automatically closed by a later patch. The C02 progress note below records
the lead's explicitly reported Phase 1 working-tree change/test separately from
baseline inspection. Machine-readable evidence and commands:
[`security-evidence.json`](security-evidence.json).

Status meanings: **PARTIAL** = useful implementation exists but requirement is
not satisfied; **MISSING** = required control not found in reviewed source;
**UNVERIFIED** = external/runtime claim lacks evidence. None means release-ready.

| ID / owner | Requirement and current evidence | Status | Required acceptance evidence / next action |
|---|---|---|---|
| C01 / Crypto | Mandatory content E2EE: `server::send_message` enforces MLS-shaped payloads only for crypto-locked conversations; auxiliary JSON state exists. | PARTIAL | Inventory text/files/replies/reactions/profiles/Watch Party; old-client plaintext/downgrade rejection and real-device MLS tests. Server framing checks cannot certify opaque MLS semantics. |
| C02 / Crypto | Phase 1 working-tree progress in `crypto.rs::encrypt_text`: immutable application ciphertext outbox with sender-state transaction; lead reports same-byte retry after DB reopen, conflict/corruption/legacy-cache rejection and injected outbox insertion rollback covered by T05. | PARTIAL | Only this local persistence/retry subproperty is tested. Universal ACTIVE-only state machine, ordered per-group worker, other crash/send/ack boundaries, forks/concurrent commits and real cross-device exchange remain unmet. |
| C03 / Storage | Existing encrypted entity codec and OS vault are distinct from full SQLCipher page authentication. | PARTIAL | Actual SQLCipher build/version and wrong-key failure; DB/WAL/temp inspection on supported Windows; no plaintext fallback. |
| C04 / Recovery | Account recovery exists; no reviewed authenticated 3-of-5 isolated identity/archive protocol found. | MISSING | All ten share combinations, corrupt/mixed-generation shares, external fingerprint anchor; recovered identity must not restore live ratchets. |
| B01 / Backend | `authenticate` looks up a hashed bearer session; no integrated canonical device-proof middleware found. | MISSING | Enrollment/account-device binding, canonical-byte vectors, changed body/target/audience and wrong-role tests. Signature proves key possession, not attestation. |
| B02 / Backend | No shared nonce/replay store found. | MISSING | Atomic replay rejection across workers, expiry/clock policy, failover/restart loss and session-revocation tests. |
| B03 / Backend | `allow_rate` uses process-local mutex/deques. Redis client/scripts/policy absent. | MISSING | Bounded atomic distributed limiter, algorithm definition, cluster hash slots, TTL/cardinality and memory-pressure/failover tests. |
| B04 / Backend | `websocket_session` rechecks bearer-session existence/expiry every 15s and handles revocation events. No signed frame sequence/device binding. | PARTIAL | Revoked live connection, frame replay/reordering, queue/frame/CPU bounds and reconnect tests across workers. Existing periodic check is not instantaneous revocation. |
| B05 / Backend | `relay_voice_signal` has participant membership/block/rate checks; Watch Party schema validator is covered by focused tests. | PARTIAL | Negative outsider/wrong-role integration tests and policy-version binding. Existing schema tests prove only payload validation. |
| B06 / Backend + Crypto | Encrypted attachment path exists; no universal device-signed resumable upload manifest contract found. | PARTIAL | Bounded-memory 5 GB upload, indexed chunk authentication/reorder/truncation and manifest completion/idempotency tests. |
| B07 / Backend + Ops | Proxy trust, global expensive-operation budget and distributed failure policy not established by reviewed source. | UNVERIFIED | Trusted-proxy spoof tests; KDF/signature concurrency and connection/header/body/time limits under load. Redis does not replace upstream DDoS controls. |
| M01 / Media | `loadIceConfiguration` chooses relay or all from a preference; direct fallback remains; LiveKit room constructors lack enforced relay configuration. | MISSING | All direct/group/GhostShare/reconnect paths fail closed without TURN; packet traces and selected candidates under network transitions. |
| M02 / Media | LiveKit Room constructors do not configure application frame E2EE. | MISSING | MLS-distributed purpose-bound keys; JS/native frame encryption interop, missing-key failure and membership rotation tests. TLS/SFU deployment is not E2EE. |
| M03 / Media + Backend | Group/share token issuers check membership and scope publication; join token TTL is 10 minutes. | PARTIAL | Device/session/revocation-generation admission; tested short initial token lifetime, concurrent join/removal and refreshed/original-token rejoin cases. `exp` does not disconnect an active session. |
| M04 / Media + Backend | `remove_livekit_participant` lists sessions and calls authenticated RoomService RemoveParticipant after membership commit; errors are logged. | PARTIAL | Durable transactional disconnect outbox, generation, idempotent retries/reconciliation, all-device enforcement, partition behavior and measured p95/p99 latency. RPC, not webhook. |
| M05 / Media + Privacy | Watch Party direct HTTPS media URLs are validated by client/server; hosts still see participant network addresses. | PARTIAL | Explicit third-party resource consent/allowlist/proxy policy and real URL-load tests. TURN-only is not anonymity. |
| M06 / Media | GhostShare recovery/replacement/leave and voice gate regression tests are local JavaScript mocks. | PARTIAL | Real two-device media, SFU reconnect, audio/screen stop, codec and frame-E2EE verification; mocks cannot prove these. |
| S01 / SecOps | Release workflow uses actions by mutable tags and broad job-level contents write; Rust builds use --locked. | PARTIAL | Reviewed SHA pinning, release job isolation, scoped permissions, environment/tag protection; verify actual repository protection separately. |
| S02 / SecOps | Tauri updater signing secret/configured public key present; no Authenticode invocation or publisher verification found. | MISSING | Approved protected signing infrastructure, timestamp/chain/publisher and artifact coverage checks. EV is not a SmartScreen guarantee; do not purchase prerequisites implicitly. |
| S03 / SecOps | `check_for_update`/`install_update` use Tauri updater checks; no signed metadata sequence/expiry/high-water protocol found. | MISSING | Wrong channel/platform, expired/frozen metadata, valid-old release, clock rollback and key rotation tests; privileged endpoint snapshot rollback boundary documented. |
| S04 / SecOps | No SBOM/provenance generation found in reviewed release workflow. | MISSING | SBOM, dependency/license review, exact toolchain/source/artifact hashes and verified build provenance. |
| O01 / Operations | Deployed VDS, LiveKit/TURN reachability, credential rotation and free-plan quotas not inspected in this task. | UNVERIFIED | Actual deployment checks with secrets redacted; record IDs and observed outcomes, not credentials. |
| O02 / Storage + Ops | No destructive history purge performed or authorized by this matrix. | UNVERIFIED | Separate dry-run inventory, validated staged migration, rollback/recovery drill and explicit retention/deletion authorization. SSD/backups erasure is not proved by SQL DELETE. |
| R01 / Independent reviewer | No independent cryptographic review or penetration-test result supplied. | UNVERIFIED | Independent review of security-sensitive changes and resolution of critical/high findings before production security claims. |

## Locally observed tests

| Evidence ID | Command | Result | Scope |
|---|---|---|---|
| T01 | `node scripts/test-watch-party.cjs` | PASS | Mocked client sync, bounds, peer scoping, replay/tie/failed-send handling. No real video-host/privacy or E2EE proof. |
| T02 | `node scripts/test-ghost-share.cjs` | PASS | Mocked SFU track lifecycle/recovery. No deployed SFU or packet-flow proof. |
| T03 | `node scripts/test-voice-gate.cjs` | PASS | Mocked detector placement/speech recovery. No hardware/multi-device quality proof. |
| T04 | `cargo test --locked --package ghostchat-server watch_party_ --lib` | PASS (2 tests) | Focused payload validation only; no HTTP/WS authorization or deployed-server proof. |
| T05 | `cargo test -p ghostchat-desktop two_account_direct_mls_survives_restart_and_deduplicates_replayed_events --locked --offline` | PASS (1 test, reported by lead) | Windows local real-OpenMLS test on Phase 1 working-tree patch: atomic outbox/sender state, retry after reopen, conflict/corruption/legacy-cache rejection and insert-failure rollback. No real-device/network or global state-machine proof. |

## Release decision

**NO-GO for the requested enterprise security guarantees.** Local regressions may
pass while required controls remain missing. This matrix does not authorize
production secret operations, paid provisioning, user-data deletion or a release.
At integration, update each row and manifest with separate implementation,
local-test, real-device and deployment evidence; never equate git push with CI,
installation, signed release availability or validated security.
