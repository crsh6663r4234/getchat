# GhostChat Full Audit Findings

| ID | Area | Severity | Finding | Failure scenario | Status | Commit |
| --- | --- | --- | --- | --- | --- | --- |
| A-1 | Build / formatting | S4 | `cargo fmt --all -- --check` fails: unformatted code in `crates/desktop/src/main.rs` (updater endpoint, ~line 60) and `crates/server/src/device_proof.rs` (import order, `ProofError` enum, `Display::fmt`). | Any CI or pre-merge step that enforces `cargo fmt --check` fails on the current branch. | fixed | 06f2c69 |
| A-2 | desktop build | S4 | crates/desktop/icons/icon.png missing; tauri::generate_context! panics on non-Windows builds so desktop tests cannot run | cargo test -p ghostchat-desktop on Linux panics at main.rs:764 | fixed | e887e8a |
| A-3 | Server / messages, attachments | S1 | `PATCH /v1/messages/:id` accepted new content starting with `file:`; `DELETE /v1/messages/:id` then deletes the attachment that content names (scoped only by conversation). | Group A/B/C. B uploads a file. C edits her own text message to `file:<B's attachment id>` (200), then deletes it: B's attachment row and blob are removed and downloads return 404. | fixed | 46fb583 |
| A-4 | Server / messages | S2 | `edit_message` checked only sender identity, not current conversation membership. | C is removed from group A/B/C; C PATCHes her old message → 200, history is rewritten and the edit is broadcast to A and B. | fixed | b88cd0c |
| A-5 | Server / sessions, WebSocket | S2 | `DELETE /v1/sessions` deleted one session but broadcast an untargeted `session_revoked`, closing every WebSocket of the account. | Alice signed in on laptop and desktop; logs out on laptop → desktop socket gets `session_revoked`, desktop clears its token and Credential Manager entry and shows the auth view though its session is still valid; she must use the recovery key. | fixed | 5207bf8 |
| A-6 | Server / presence | S2 | `GET /v1/me/presence` and `presence_updated` events sent the saved mode `invisible` to friends while the user was connected (only the desktop UI masked it). | Alice sets Invisible with the app open; friend Bob reads `/v1/me/presence` (or WS event) → `invisible`, revealing Alice is online. | fixed | d8044b4 |
| A-7 | Server / account deletion | S1 | `DELETE /v1/me/account` runs `DELETE FROM conversations WHERE id IN (… memberships of user)`, deleting every group the user belongs to, including other members' messages and files. | Group A/B/C; C deletes her account → group, A's and B's messages and attachments are gone (A's `GET …/messages` → 404). Fix needs a policy for group ownership succession and encrypted rosters; SECURITY.md wording ("its conversations") is ambiguous. | needs owner decision | |
| A-8 | Server / block list | S2 | `edit_message` does not apply `ensure_direct_not_blocked`; a blocked user can still change old DM text and the change is pushed to the blocker. | Bob blocks Alice; new sends → 403, but Alice PATCHes an earlier DM → 200 and Bob receives the new text. SECURITY.md only names "new direct messages"; owner to decide if edits count. | needs owner decision | |
| A-9 | Server / messages | S4 | `delete_message` has no membership check, so a removed/left member can still delete their own old messages. | C leaves group, then DELETEs her old message → 204, removed for A and B. May be intentional (sender privacy). | needs owner decision | |
| A-10 | Server / block list, GhostShare | S4 | `POST /v1/conversations/:id/share-token` (direct GhostShare room) does not check the block list, unlike voice signaling. | Bob blocks Alice; with LiveKit configured Alice can still get a token for `ghostshare-<dm>`; `ghostshare_started` signaling is blocked, so impact is limited to joining an empty room. | open | |
| A-11 | Server / WebSocket typing | S4 | Typing relay checks membership but not the block list. | Bob blocks Alice; Alice's typing indicator is still delivered to Bob in their DM. | open | |
| A-12 | Server / planner | S4 | Poll `closes_at` is stored but never enforced, and no route sets `closed_at` / `cancelled_at`. | A poll with `closes_at` in the past still accepts votes. | open | |
| A-13 | Server / rate limits | S4 | `DELETE /v1/users/:id/block` (unblock) has no rate limit although SECURITY.md lists "blocking changes" as limited. | A client can spam unblock requests (each emits a WS event) without 429. | open | |
| A-14 | Server / attachment storage | S4 | Encrypted attachments (`/encrypted-attachments`) are not linked to a message, so deleting the `e2e1:` message never deletes the blob; only account/conversation deletion removes them. | A member uploads and deletes encrypted files repeatedly; blobs (up to 5 GB each, 10/min) accumulate on disk. | open | |
| A-15 | Server / user search | S4 | `GET /v1/users/search` passes user input into `LIKE` unescaped; `_` and `%` act as wildcards. | Query `__` returns the first 20 usernames; varying prefixes enumerates the user directory. | open | |
| A-16 | Server / authorization | — | Candidate: message/attachment/pin/save/reaction/read/planner/invite/crypto-event routes readable or writable by non-members. | `message_routes_reject_non_member` passes: every route returns 404/403 to a non-member. | not a defect | |
| A-17 | Server / attachment storage | — | Candidate: global 96 KB `DefaultBodyLimit` truncates streaming uploads. | `attachment_upload_accepts_bodies_above_json_limit` passes (512 KB upload → 200); raw `Body` is not limited. | not a defect | |
| A-18 | Server / rate limits | — | Candidate: account creation (20/min) and recovery (10/min) limits are server-global, so one client can exhaust them for everyone. | Documented: SECURITY.md states there is no proxy-aware IP limiter and limits are process-local. | not a defect | |

## Verified checks
Run on Linux, branch `claude/dreamy-knuth-j0wr07`, after the A-1 and A-2 fixes (baseline before fixes: fmt failed, desktop crate did not compile).
Baseline run on Linux, branch `claude/dreamy-knuth-j0wr07`, before any fixes.

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check` | PASS (after A-1 fix) |
| `cargo test --locked -p ghostchat-server` | PASS (12 passed, 0 failed, 1 ignored); after Task 2: 18 passed, 0 failed, 1 ignored |
| `for f in scripts/test-*.cjs; do node "$f"; done` | PASS (ghost-share, media-relay, voice-gate, watch-party all exit 0) |
| `for f in crates/desktop/ui/*.js; do node --check "$f"; done` | PASS (7 files) |
| `git diff --check` | PASS |
| `cargo test --locked -p ghostchat-desktop` | PASS (2 passed, 0 failed) after A-2 fix |

### Desktop test build

Installed `libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev` (after `apt-get update`). The `gdk-sys` failure is resolved, but `cargo test --locked -p ghostchat-desktop` fails with:

```
error: proc macro panicked
   --> crates/desktop/src/main.rs:764:14
764 |         .run(tauri::generate_context!())
    = help: message: failed to open icon /home/user/getchat/crates/desktop/icons/icon.png: No such file or directory (os error 2)
```

`crates/desktop/icons/` contains only `ghostchat.ico` and `ghostchat.svg`. Fixed by A-2 (PNG generated from ghostchat.svg with rsvg-convert, 512x512 RGBA; tauri.conf.json unchanged). Desktop tests now compile and pass on Linux.
