# Build progress

Date: 2026-10-16

- Environment and toolchains inspected; exact workstation identifiers and machine-specific measurements are omitted from this public report.
- Created a Rust workspace with a Tauri 2 desktop executable, Axum server executable/library, SQLite migration, static UI, and Windows Credential Manager commands.
- Implemented anonymous accounts, recovery, sessions, friend requests, direct conversations, text messages, WebSockets, message search/edit/delete, account export/deletion, rate limits, and documentation.
- `cargo check --offline --workspace` initially needed uncached official crates. The user-authorized crates.io download succeeded; subsequent checks used the local cache.
- `cargo check --offline -p ghostchat-server`: passed.
- `cargo check --offline -p ghostchat-desktop`: passed.
- `cargo test --offline -p ghostchat-server`: passed, 3 unit tests.
- `crates/server/scripts/smoke-test.ps1`: passed two-client HTTP/WebSocket account, friendship, message, authorization, rate-limit, reconnect, recovery, export, deletion, and logout checks.
- `cargo build --offline --release -p ghostchat-desktop`: passed.
- Desktop launch: verified a running `GhostChat` window with a non-zero window handle when launched with the required per-user data and loopback permissions.
- `cargo build --offline --release -p ghostchat-server -p ghostchat-desktop`: passed.
- Tauri CLI 2.12.3 and WiX 3.14 produced `target/release/bundle/msi/GhostChat_0.1.0_x64_en-US.msi` (unsigned).
- The desktop launched with window title `GhostChat`, and a local idle performance sample was collected. Exact workstation-specific values are omitted; see [PERFORMANCE.md](PERFORMANCE.md).
- Voice/video/screen-share, communities, attachments, and E2EE are not implemented. This is a tested MVP with text DMs and a self-hosted server, not a full Discord replacement. See [FEATURES.md](FEATURES.md) and [TEST_REPORT.md](TEST_REPORT.md).
- 2026-10-17 follow-up: reproduced the Windows desktop's failed server health check. Tauri uses `http://tauri.localhost` for the packaged Windows webview, which was missing from the server's CORS allowlist. Added that exact origin, rebuilt the app and server, verified the health response's CORS header, and restarted the local standalone server while preserving its SQLite database.
