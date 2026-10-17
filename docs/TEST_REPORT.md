# Test report

Date: 2026-10-16

## Automated checks

- `cargo fmt --all -- --check` — passed.
- `cargo check --offline -p ghostchat-server` — passed.
- `cargo check --offline -p ghostchat-desktop` — passed.
- `cargo clippy --offline -p ghostchat-server -p ghostchat-desktop --all-targets -- -D warnings` — passed.
- `node --check crates/desktop/ui/main.js` — passed.
- `cargo test --offline -p ghostchat-server` — passed, 3 unit tests.
- `crates/server/scripts/smoke-test.ps1` — passed against a local server and two authenticated WebSocket clients.
- `cargo build --offline --release -p ghostchat-server -p ghostchat-desktop` — passed.
- Tauri MSI bundle — passed; `GhostChat_0.1.0_x64_en-US.msi` created.
- Windows CORS regression check — reproduced the missing `Access-Control-Allow-Origin` header for `http://tauri.localhost`, added the Windows production origin, rebuilt the server, and verified `/health` returns HTTP 200 with the matching allow-origin header.

## Integration coverage

The smoke test exercised health, anonymous account creation, recovery-key issuance, unique IDs, friend request/accept and persisted relationships, rate limiting, authenticated WebSocket connections, message delivery, duplicate-message idempotency, server ordering, cursor pagination, input limits, private-conversation authorization, invalid sessions, reconnect/history, editing/deleting messages, recovery and session rotation, live WebSocket revocation, account export/deletion, and logout revocation. Expected authorization and validation failures returned their corresponding errors.

WebRTC media negotiation, microphone permissions, device routing, cross-network TURN relay, and screen capture are not covered by this server smoke test. They need interactive Windows devices and network-path checks.

## Manual launch

The release desktop app was launched and verified to expose a native `GhostChat` window. Its bundled local server uses loopback. The MSI was generated but not installed on this machine as part of this report.

## Limits

These checks establish basic functionality for the implemented MVP; they do not constitute a security audit, penetration test, load test, multi-host deployment validation, or a guarantee of production readiness. The app is unsigned. For the implemented product scope and omitted capabilities, see [FEATURES.md](FEATURES.md) and [SECURITY.md](SECURITY.md).
