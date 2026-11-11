# GhostChat Full Bug Audit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Find and fix real defects across the server, desktop Rust and UI, and record every finding in a report.

**Architecture:** One audit task per area, highest risk first. Each confirmed defect gets a failing regression test, a minimal fix and its own commit. Findings accumulate in one report file.

**Tech Stack:** Rust (Axum, SQLx/SQLite, Tokio, Tauri 2, OpenMLS), plain browser JavaScript tested with Node `vm` fixtures (`scripts/test-*.cjs`).

**Spec:** `docs/superpowers/specs/2026-11-08-full-audit-design.md`

## Global Constraints

- No new features, redesigns, style refactors or dependency upgrades.
- Do not change intended behavior without asking the owner; record it as "needs owner decision" instead.
- Every fix needs a concrete failure scenario (inputs/state → wrong result).
- Never skip, weaken or delete an existing test.
- Green before every commit: `cargo test --locked -p ghostchat-server`, `for f in scripts/test-*.cjs; do node "$f"; done`, `for f in crates/desktop/ui/*.js; do node --check "$f"; done`, `cargo fmt --all -- --check`, `git diff --check`. Add `cargo test --locked -p ghostchat-desktop` once Task 1 makes it build.
- One defect per commit, message `fix(<area>): <what was broken>`.
- Severity labels: `S1` security/data loss, `S2` broken feature, `S3` stuck/confusing state, `S4` edge case. S4 may be reported without a fix.
- Anything that needs Windows, two devices, a real TURN/LiveKit server or MLS interop to confirm is marked `needs Windows verification`.

## Review Focus

- Requests from a user who is not a member/owner of the conversation, group or attachment they reference: expect 403/404, never data. (Task 2)
- Deleted, blocked or removed users still using an old session or WebSocket: expect rejection. (Task 2)
- Signals and events arriving after the call/room they belong to has ended: expect them to be ignored, no zombie state. (Tasks 4, 5)
- Double clicks and repeated actions on call/accept/share buttons while an async step runs: expect exactly one action. (Tasks 4, 6)
- App restart in the middle of a multi-step operation (MLS commit, upload, recovery): expect no corrupted or half-written state. (Task 3)

---

### Task 1: Findings report and desktop test build

**Files:**
- Create: `docs/superpowers/specs/2026-11-08-full-audit-findings.md`

- [ ] **Step 1: Create the report** with a table header `| ID | Area | Severity | Finding | Failure scenario | Status | Commit |` and a "Verified checks" section. IDs are `A-<n>` in discovery order.
- [ ] **Step 2: Try to make the desktop crate build on Linux**

Run: `sudo apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev && cargo test --locked -p ghostchat-desktop`
Expected: tests compile and pass. If the install or build fails for platform reasons (Windows-only APIs), record that in the report and audit desktop Rust by reading only.
- [ ] **Step 3: Record baseline results** of every Global Constraints check in "Verified checks".
- [ ] **Step 4: Commit** `docs: start full audit findings report`.

### Task 2: Server audit

**Files:**
- Audit: `crates/server/src/lib.rs`, `ratelimit.rs`, `device_proof.rs`, `media_revocation.rs`, `crates/server/migrations/*.sql`
- Test: `crates/server/src/lib.rs` `mod tests` (follow existing `#[test]`/`#[tokio::test]` style; build an in-memory SQLite app for route tests)

Checklist, each item checked against every route and WebSocket message handler:
- session lookup on every authenticated route; revoked/deleted session rejected
- membership/role check before reading or writing conversations, messages, attachments, pins, reads, invites, planner items, saved messages, crypto events
- block list enforced on DMs, uploads and call signaling
- rate limits applied where documented in `docs/SECURITY.md`
- migrations: constraints match what handlers assume; no handler relies on a column a migration drops
- attachment storage: path traversal, size limits, deletion with message/account

- [ ] **Step 1:** Walk the checklist; add each candidate to the report as `open`.
- [ ] **Step 2:** For each S1–S3 candidate, write a failing test named `<route_or_area>_<expected_behavior>` (e.g. `pins_reject_non_member`) asserting the expected status/data.
- [ ] **Step 3:** Run `cargo test --locked -p ghostchat-server <test_name>`; expected FAIL. If it passes, the candidate is not a defect: mark it `not a defect` in the report.
- [ ] **Step 4:** Fix minimally; run all Global Constraints checks; expected PASS.
- [ ] **Step 5:** Commit per defect; update the report row to `fixed` with the commit hash.

### Task 3: Desktop Rust audit

**Files:**
- Audit: `crates/desktop/src/main.rs`, `secure_vault.rs`, `identity_recovery.rs`, `crypto.rs`, `crypto_attachments.rs`
- Test: existing `#[cfg(test)]` modules in those files

Checklist:
- Tauri commands validate input from the webview (the webview is untrusted after XSS)
- vault: keys never written in plaintext; failed decrypt does not wipe data
- MLS: state, decrypted cache and cursor committed atomically (claimed in release doc; verify); replayed/out-of-order events rejected without corrupting state
- attachments: chunk nonce uniqueness, truncation and reordering detected, partial files cleaned up
- recovery: rotating sessions does not leave the old session valid

- [ ] **Steps 1–5:** Same cycle as Task 2, with `cargo test --locked -p ghostchat-desktop <test_name>`. If Task 1 could not build the crate, record each finding with `needs Windows verification` and fix only when the reasoning is airtight.

### Task 4: Direct call fixes (already identified)

**Files:**
- Modify: `crates/desktop/ui/voice-client.js` (`accept()` ~782, `applyDescription()` ~837, `handleSignal()` ~867)
- Test: `scripts/test-media-relay.cjs` (extend `directFixture`; make `setTimeout` fixture capture callbacks so timers can be fired)

- [ ] **Step 1: Write failing test `accept after caller hangup during relay probe opens no call`**: deliver an `offer` signal, call `client.accept()` while `getIceConfiguration` is pending, deliver `hangup` from the same peer, resolve config; assert `client.getState()` shows no active call, no `RTCPeerConnection` besides the probe was created, and no `answer` signal was sent.
- [ ] **Step 2: Write failing test `double accept creates one peer connection`**: call `accept()` twice before config resolves; assert exactly one non-probe peer and one `answer` signal.
- [ ] **Step 3: Write failing test `call stuck in connecting times out`**: complete offer/answer, keep `connectionState` at `connecting`, fire the captured timer; assert the call is closed, a `hangup` signal was sent and `onError` received a message.
- [ ] **Step 4:** Run `node scripts/test-media-relay.cjs`; expected FAIL on the three new checks.
- [ ] **Step 5: Fix in `voice-client.js`:**
  - `accept()`: mark `incoming.accepting = true` before awaiting config, return early if already set; after the await, abort if `incoming !== request`.
  - Add a connect timeout constant `CONNECT_TIMEOUT_MS = 30_000` started when the answer is applied (caller) or sent (callee), cleared on `connected`; on fire, `closeCall({ notifyPeer: true, message: "The call could not connect. Check your connection and call again." })`.
  - `handleSignal()` catch: guard `call?.ignoreOffer` so a call closed during an await does not throw.
- [ ] **Step 6:** Run all Global Constraints checks; expected PASS.
- [ ] **Step 7:** Commit each fix separately; add rows A-n to the report (`needs Windows verification` for real-network behavior).

### Task 5: Other UI media audit

**Files:**
- Audit: `crates/desktop/ui/voice-client.js` (rest), `group-voice-client.js`, `ghost-share-client.js`, `watch-party.js`, `call-ringtone.js`
- Test: `scripts/test-media-relay.cjs`, `test-ghost-share.cjs`, `test-watch-party.cjs`, `test-voice-gate.cjs`

Checklist: events for ended calls/rooms ignored; tracks and timers stopped on every exit path; ringtone stops on accept/decline/remote hangup/error; device change mid-call; screen share stop on window close.

- [ ] **Steps 1–5:** Same cycle as Task 2, with the owning `scripts/test-*.cjs` file and `node scripts/<file>.cjs`.

### Task 6: UI app logic audit

**Files:**
- Audit: `crates/desktop/ui/main.js`, `desktop-controls.js`, `index.html`
- Test: new `scripts/test-main-logic.cjs` only if a defect lives in a function that can be loaded in a `vm` fixture; otherwise record as `needs Windows verification`

Checklist: user-supplied text rendered with `textContent` not `innerHTML` (XSS reaches Tauri commands); WebSocket reconnect does not duplicate handlers or messages; buttons disabled during async actions are re-enabled on every path; server address change clears per-server state; settings switches persist and reload; update check failure is reported, not silent.

- [ ] **Steps 1–5:** Same cycle as Task 2.

### Task 7: Close the report

- [ ] **Step 1:** Every report row has a final status; summary at top: counts per severity and status, list of `needs Windows verification` items with a one-line manual test each.
- [ ] **Step 2:** Run all Global Constraints checks; record results in "Verified checks".
- [ ] **Step 3:** Commit `docs: complete full audit findings report` and push.
