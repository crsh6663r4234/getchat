# GhostChat

GhostChat is a lightweight Windows desktop chat client with a self-hostable Rust server. The current stable release remains 0.3.8. The unreleased 4.0 work adds group chats, file sharing, profile controls, tray behavior, and experimental opt-in MLS encryption for direct and group text. New attachments in verified MLS chats are encrypted on Windows; earlier messages and files remain plaintext. MLS attachments and messaging still need two-device validation and independent security review. Encrypted groups support experimental staged addition, removal, and owner-committed non-owner self-leave flows. See [docs/SECURITY.md](docs/SECURITY.md) before using it for sensitive conversations. Direct calls remain peer-to-peer; experimental group calls and screen sharing require a separately deployed LiveKit SFU and are not end-to-end encrypted. Group calling is not release-ready until deployment and member-removal behavior are validated.

## Run the Windows app

Run the `GhostChat-Setup-x64.msi` installer, or use the portable files. The app starts a private loopback server and stores its SQLite database in the current Windows account's application-data directory. Windows WebView2 Runtime is required; the MSI includes Microsoft's WebView2 bootstrapper.

The MSI and portable executables are unsigned. Windows may show an unfamiliar-app prompt. Portable use requires WebView2 Runtime to already be installed.

## Use it on two computers

The local server is private to one device. For a temporary free demo from a Windows computer, see [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md#temporary-free-demo-from-a-windows-pc). For a stable deployment, place the standalone server on a host reachable by both devices behind an HTTPS reverse proxy, then open **Settings → Server address** in each client and enter the HTTPS address. Create a separate anonymous account on that server for each person.

The standalone backend defaults to `0.0.0.0:38241` so another device or reverse proxy can reach it. Set `GHOSTCHAT_BIND=127.0.0.1:38241` when the HTTPS reverse proxy runs on the same host. Set `GHOSTCHAT_DATABASE` to configure storage. See [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md), [docs/VOICE.md](docs/VOICE.md), and the placeholder-only [server.env.example](server.env.example). Keep the raw backend port blocked from public traffic; expose the service through HTTPS with firewall rules and operational monitoring.

## Anonymous account recovery

Account creation asks only for a display name. GhostChat creates a unique random username tag, and you can optionally add a profile photo. It does not request an email, phone number, or conventional password. Save the recovery key when it is shown. The server stores a salted Argon2 verifier plus a lookup digest; it never stores the raw recovery key. Losing both the Windows session and recovery key can permanently lock you out.

Sessions are stored in Windows Credential Manager, scoped to the selected server. Logout revokes the active server session. Recovery rotates sessions. Account data can be exported from Settings, or permanently deleted after a second confirmation.

## Build from source

Requirements: Rust (MSVC target), Windows SDK, WebView2 Runtime, and the Rust crates listed in `Cargo.lock`.

```powershell
cargo fmt --all -- --check
cargo check --offline --workspace
cargo test --offline -p ghostchat-server
cargo build --release -p ghostchat-server
cargo build --release -p ghostchat-desktop
```

The desktop is a Tauri 2 app with a small static HTML/CSS/JavaScript interface. The server uses Axum, WebSockets, Tokio, SQLx, and SQLite migrations. No npm install is needed.

The Windows MSI was built with Tauri CLI 2.12.3 and WiX 3.14. See [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md) for the rebuild command.

## Updates

The installed Windows app checks for signed updates in Settings and on startup. GitHub Releases publishing is configured in [docs/UPDATES.md](docs/UPDATES.md). It becomes live after the project has a GitHub repository and the updater signing secret is added.

## Current scope

See [docs/FEATURES.md](docs/FEATURES.md) for the implementation checklist, [docs/SECURITY.md](docs/SECURITY.md) for data handling and security limits, and [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md) for common setup issues. Test results are recorded in [docs/TEST_REPORT.md](docs/TEST_REPORT.md). Measured performance and its limits are in [docs/PERFORMANCE.md](docs/PERFORMANCE.md).

The planned major release scope and delivery stages are tracked in [docs/GHOSTCHAT_4_0_ROADMAP.md](docs/GHOSTCHAT_4_0_ROADMAP.md). The encrypted-event relay foundation is documented in [docs/CRYPTO_EVENT_RELAY.md](docs/CRYPTO_EVENT_RELAY.md); the unreleased direct and group encryption code uses this relay for MLS setup events and stores text ciphertext through the normal message history API.
