# Server deployment

GhostChat requires a reachable server for communication between separate computers. The desktop's embedded server is loopback-only and private to that Windows device.

## Temporary free demo from a Windows PC

For a no-cost test with friends, run `scripts/start-free-demo-server.ps1` from PowerShell. If `cloudflared` is missing, the script downloads the official Windows binary and checks Cloudflare's published SHA-256 before running it. It builds and starts the standalone server if it is not already healthy; a server started by the script stores its SQLite database under `%LOCALAPPDATA%\GhostChat\shared-server`. It then opens a Cloudflare Quick Tunnel to the loopback-only API. Keep the script window and the host PC online. The generated `https://…trycloudflare.com` URL is copied to the clipboard; paste it in **Settings → Server address** on each client.

This is a temporary test setup, not a permanent or unlimited host. Quick Tunnel URLs change when the tunnel restarts, stop working when its process exits, have no uptime guarantee, and currently allow at most 200 in-flight requests. Anyone who gets the URL can attempt to create an account. Traffic passes through Cloudflare, and GhostChat messages are not end-to-end encrypted; do not use this path for sensitive conversations. For a stable address or sustained public use, deploy to a managed host and reverse proxy with HTTPS. Cloudflare's [Quick Tunnel documentation](https://developers.cloudflare.com/tunnel/get-started/quick-tunnels/) describes its limits.

The free PC-hosted setup cannot run while the host is asleep or offline. There is no genuinely unlimited free cloud server; free hosting tiers impose resource and availability limits.

## Build the Windows installer

From `crates/desktop`, run the Tauri CLI with WiX 3 installed:

```powershell
tauri build --bundles msi
```

The MSI will be written below `target/release/bundle/msi`. The application and installer need an Authenticode signature from a publisher certificate before distributing them broadly. The MSI built for this delivery is unsigned.

## Prepare the host

1. Build `ghostchat-server` for the host platform with Rust and the system SQLite prerequisites from SQLx (SQLite is bundled in this workspace build).
2. Create a service account and a private data directory writable only by that account.
3. Set `GHOSTCHAT_DATABASE` to an absolute path inside that directory. Set `GHOSTCHAT_BIND=127.0.0.1:38241` when using a local reverse proxy.
4. Run the server behind an HTTPS reverse proxy. Permit public traffic only to the proxy's TLS port; do not expose the raw application port.
5. In GhostChat on each computer, set the server address to the public `https://` URL. The WebSocket endpoint upgrades from HTTPS to WSS automatically.
6. Back up SQLite using a SQLite-aware online backup tool, or stop the service cleanly before copying the database. Protect backups as readable private-message data.

## Environment

`GHOSTCHAT_BIND` defaults to `127.0.0.1:38241`. `GHOSTCHAT_DATABASE` defaults to `ghostchat.sqlite` in the process working directory; use an absolute path for deployment. `RUST_LOG` configures server logging. Request bodies and logs do not include authentication tokens.

See [server.env.example](../server.env.example) for placeholders. Do not place secrets in that file. This server does not require a private server key.

## Operational limits

The server has health endpoint `GET /health`, SQLite WAL mode, versioned migrations, a five-second SQLite busy timeout, and graceful Ctrl+C shutdown. It does not include a TLS listener, clustered presence, replicated rate limits, automated backup rotation, metrics, or an operations dashboard. Use one server process per database.
