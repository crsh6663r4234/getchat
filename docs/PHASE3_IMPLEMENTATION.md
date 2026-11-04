# Phase 3 backend increment

Date: 2026-11-04. Modules: `crates/server/src/ratelimit.rs` and
`crates/server/src/device_proof.rs`. This document describes implemented library
primitives and integration obligations, not a claim that all HTTP/WS routes use
device signatures or that Redis failover has been validated.

## Dependencies and deployment

The integration owner adds `redis = 0.32.7` with `tokio-comp`, `script` and
`cluster-async`, and cached `ed25519-dalek = 2.2.0` with `std`. Keep the resolved
versions in Cargo.lock. Redis-rs supports async multiplexing and cluster routing;
its scripts reload after script-cache loss. Both crates retain their own license
terms and dependency/build review requirements.

- Set **one** of `GHOSTCHAT_REDIS_URL` or `GHOSTCHAT_REDIS_CLUSTER_URLS` (comma-separated
  cluster seed URLs, 1–16 entries). No service is provisioned by these modules.
- `GHOSTCHAT_REDIS_NAMESPACE` defaults to `ghostchat-v1`; 1–64 ASCII alphanumeric,
  underscore or hyphen characters. Use a different namespace per deployment.
- `GHOSTCHAT_REDIS_TIMEOUT_MS` defaults to 1500; accepted range 50–10000 ms.
- Redis URLs can contain credentials. Error messages deliberately do not echo URL
  strings, credential values or raw Redis errors. Do not print environment values.
- TLS URLs require an appropriate Redis-rs TLS/runtime Cargo feature. No implicit
  downgrade to non-TLS exists. Configure certificate validation for production.
- `RateLimiter::from_env() -> Result<Option<RateLimiter>, LimitError>` returns
  `None` when explicitly unconfigured. The caller must not interpret `None` or
  an error as secure permission. A separately named development mode is a product
  decision, not an enterprise security guarantee.

## Exact sliding log

`check(scope, subject, limit, window)` returns `Decision {allowed, remaining,
retry_after}` or a fail-closed `LimitError`. Scope is a server-selected constant,
not attacker input; subject must come from an authenticated identity or a trusted
proxy/source-address policy. Do not read arbitrary X-Forwarded-For as identity.
All workers must use the same limit/window for each scope.

One atomic Lua script uses Redis TIME, removes accepted events outside
`(now-window, now]`, checks count and inserts a server-generated UUID only when
allowed. Denied requests do not add events. Windows are 1 ms–24 hours, limits
1–10000 accepted events, and each scope registry is capped at 4096 active subjects.
Expired registry entries are removed atomically; new subjects fail closed when
the registry is full. Subject values are SHA-256 hashed in bounded keys. Hashing
is key-length/privacy hygiene, not anonymization against low-entropy guessing.

The event key and subject-registry key share a derived Redis hash tag, and the
cluster client routes commands. Key TTL and registry TTL are explicit. No SCAN,
unbounded script loop, user-generated script or silent in-memory fallback exists.
Thirty-two in-flight operations per process are allowed; saturation, timeout,
unavailable Redis or malformed replies return errors, not permission. Timed-out
operations may have committed and must be treated conservatively.

This is atomic admission, not a distributed transaction spanning independent
account/IP/global policies. The root may compose checks sequentially, accepting
that earlier accepted scopes can be charged if a later scope denies. Place a
bounded global/source admission limit before creating per-identity state. Active
session replay keys also require bounded enrollment/admission at integration.

Redis replication/persistence loss, administrative rollback, eviction and wall
clock discontinuities remain deployment concerns. Use noeviction and operational
capacity headroom for authentication state; define a restart/failover fence before
claiming replay protection survives state loss. The script clamps backwards time
to the subject's most recent admitted event, but that is not a global trusted clock
or proof of restart consistency. Real-cluster failover tests remain a release gate.

## Device proof v1 wire contract

`RequestProof` is a bounded JSON transport structure with unknown fields rejected:
`version, audience, account_id, device_id, session_id, method, target, body_sha256,
timestamp_ms, request_id, signature`. All identifiers are 1–128 allowed ASCII
characters; version is 1; method is an uppercase allowlisted HTTP method.
Request ID is canonical lowercase hyphenated UUID text. The body digest is
canonical unpadded base64url of 32 SHA-256 bytes; the signature is canonical
unpadded base64url of 64 Ed25519 bytes.

Signed bytes, in order:

1. Literal domain bytes `GhostChat.DeviceProof\0`.
2. Version as unsigned 16-bit big endian.
3. Audience, account ID, device ID, session ID, method and exact origin-form target;
   each UTF-8 value prefixed by its unsigned 32-bit big-endian byte length.
4. Raw SHA-256 digest (32 bytes).
5. Timestamp in Unix milliseconds as unsigned 64-bit big endian.
6. Raw UUID bytes (16 bytes).

JSON field order is not signed and has no effect. No signature algorithm is
invented: Ed25519-dalek strict verification is used. The proof establishes key
possession, not software attestation, user intent or operation authorization.

`target` is exact bounded ASCII origin-form, maximum 2048 bytes. No URL fragment,
absolute authority, control character, backslash, dot path segment, duplicate
path slash, encoded path delimiter/percent/dot or noncanonical percent encoding
is accepted. Query keys are unique ASCII alphanumeric/underscore/hyphen, sorted
lexicographically, each with an equals sign. This is a new strict protocol:
clients must emit these exact targets before signing. Middleware must read the
original request URI and reject normalization/duplicate-header ambiguity; it
must not infer targets from decoded handler parameters.

`verify(proof, TrustedBinding, RequestContext) -> VerifiedProof` accepts a
caller-supplied **already trusted** account/device/session/audience/public-key
binding. Revoked bindings fail. Actual method, target and exact-body digest must
match. Proof age is at most 90 s, future skew at most 30 s. Invalid shape,
binding, signature or timestamp is rejected with a generic message.

After successful verification, before executing a handler, the caller must invoke
`limiter.claim_replay(verified.replay_binding, verified.request_id,
verified.replay_ttl)`. The required retention is 121 s, covering accepted skew and
age. Claiming uses an atomic bounded per-session sorted set with expiry, detects
UUID aliases by normalization and rejects replay or capacity exhaustion. Reusing
a valid proof concurrently cannot be accepted twice by that Redis primary, but
Redis data loss/failover durability is a separate acceptance gate.

The module intentionally does not manufacture a device enrollment protocol, trust
keys supplied by the same untrusted request, or retrofit signature enforcement
onto passwordless bootstrap. The integration owner must provide authenticated
enrollment/replacement, duplicate proof-header rejection, bounded body parsing,
per-session admission, replay claim, current role/member checks and a versioned
bootstrap exception policy. Large uploads use a bounded signed manifest; never
buffer 5 GB merely to run this verifier. Existing WS needs its own device-bound
per-frame sequence/expiry/revocation integration. Opaque MLS semantics remain
client verification responsibility.

## Verification and remaining gates

Local unit tests cover deterministic byte encoding, changed target/body/audience/
session, revocation, expiration/future skew, signature tampering, URL aliases,
duplicate query keys, key slot compatibility, policy bounds and fail-closed
unavailable Redis. A real-service test is explicitly ignored by default:

```text
GHOSTCHAT_TEST_REDIS_URL=<isolated test Redis>
cargo test --locked -p ghostchat-server redis_exact_window_concurrent_workers_and_replay --lib -- --ignored
```

That test uses unique short-lived namespaces and no destructive flush. It tests
concurrent independent clients, exact allowance, expiry and replay. No Redis
service or cluster is installed, purchased or provisioned by this task. Record
actual completion before claiming these tests passed. Required future gates:
real cluster routing/MOVED/ASK, failover/restart/persistence loss, clock changes,
eviction/memory pressure, saturated registry, TLS and network partition; endpoint
middleware/enrollment integration; open-WS revocation and adversarial load.

Primary implementation references inspected:
- https://docs.rs/redis/0.32.7/redis/struct.Script.html
- https://docs.rs/redis/0.32.7/redis/struct.Client.html
- https://redis.io/docs/latest/develop/programmability/eval-intro/
- https://redis.io/docs/latest/commands/zadd/
- https://docs.rs/ed25519-dalek/2.2.0/ed25519_dalek/struct.VerifyingKey.html
