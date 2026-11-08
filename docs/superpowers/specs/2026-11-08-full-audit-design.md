# GhostChat full bug audit: design

Date: 2026-11-08

## Intent

The owner wants to "fix what doesn't work" before deciding where the product goes next (it should not stay a standard P2P chat app; that direction is a later conversation). No specific symptoms were reported, so the whole application is audited for real defects.

## Goals

- Find defects that break user-visible behavior, lose or corrupt data, or weaken security/privacy.
- Fix each confirmed defect with a regression test where the code can be tested on Linux.
- Leave a findings report: what was found, what was fixed, what could not be verified.

## Non-goals

- New features, redesigns, refactors for style, dependency upgrades.
- The "not a standard P2P app" product direction.
- Performance tuning unless it causes a visible failure.

## Anti-goals

- "Fixes" that change intended behavior without the owner's say.
- Speculative changes with no demonstrated failure scenario.
- Skipping or weakening existing tests to get green.

## Constraints

- This environment is Linux. The Windows Tauri app cannot be launched or clicked through; two-device calls, MLS interop and LiveKit cannot be exercised here. Desktop findings are verified by reading code and by Node unit tests of UI modules (the existing `scripts/test-*.cjs` pattern).
- Existing checks must stay green: `cargo test -p ghostchat-server`, `node scripts/test-*.cjs`, `node --check` on UI files.

## Approaches considered

- A) Owner reports symptoms, targeted fixes: fastest and most accurate, but no symptoms were available.
- B) Calls/voice only: most recent churn, smaller scope.
- C) Whole application audit: **chosen by the owner.** Widest coverage; risk of time spent on issues the owner never hits, mitigated by severity ordering below.

## Method

Audit area by area, in this order (highest risk first):

1. Server (`crates/server`): authentication, sessions, authorization on every route and WebSocket event, rate limits, migrations, attachments, crypto event relay, group roles/invites.
2. Desktop Rust (`crates/desktop/src`): secure vault, identity recovery, MLS crypto, encrypted attachments, Tauri commands.
3. UI calls and media: `voice-client.js`, `group-voice-client.js`, `ghost-share-client.js`, `watch-party.js`, `call-ringtone.js`.
4. UI app logic: `main.js` (chat, groups, planner, settings, updates), `desktop-controls.js`.

For each candidate defect: state a concrete failure scenario; confirm it by reading the code paths (and a failing test where possible); fix it minimally; run all checks; commit one fix per commit.

Severity order for fixing: security/data loss > broken feature > stuck/confusing state > edge case. Low-severity items may be recorded in the report without a fix.

Already identified (calls):
- Accepting an incoming call while the TURN probe runs (up to 4 s) still opens a call after the caller hung up.
- A call can stay in "connecting" forever after the answer if ICE neither connects nor fails.

## Left to the builder

Exact test harness per area, grouping of commits, and whether a low-severity item is worth fixing.

## Output

`docs/superpowers/specs/2026-11-08-full-audit-findings.md`: every finding with severity, status (fixed / not fixed / needs Windows verification), and the commit that fixed it.
