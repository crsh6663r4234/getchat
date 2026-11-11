# Server deployment

GhostChat requires a reachable server for communication between separate computers. The desktop's embedded server is loopback-only and private to that Windows device.

## Temporary free demo from a Windows PC

For a no-cost test with friends, run `scripts/start-free-demo-server.ps1` from PowerShell. If `cloudflared` is missing, the script downloads the official Windows binary and checks Cloudflare's published SHA-256 before running it. When no server is healthy, it downloads the latest standalone GhostChat server release, checks its published SHA-256, and starts it. A server started by the script stores its SQLite database under `%LOCALAPPDATA%\GhostChat\shared-server`. It then opens a Cloudflare Quick Tunnel to the loopback-only API. Keep the script window and the host PC online. The generated `https://…trycloudflare.com` URL is copied to the clipboard; paste it in **Settings → Server address** on each client.

This is a temporary test setup, not a permanent or unlimited host. Quick Tunnel URLs change when the tunnel restarts, stop working when its process exits, have no uptime guarantee, and currently allow at most 200 in-flight requests. Anyone who gets the URL can attempt to create an account. Traffic passes through Cloudflare, and GhostChat messages are not end-to-end encrypted; do not use this path for sensitive conversations. For a stable address or sustained public use, deploy to a managed host and reverse proxy with HTTPS. Cloudflare's [Quick Tunnel documentation](https://developers.cloudflare.com/tunnel/get-started/quick-tunnels/) describes its limits.

The free PC-hosted setup cannot run while the host is asleep or offline. The launcher checks both server health and the voice API, so it reports when a healthy server is too old for voice or screen sharing. Stop the current demo script with Ctrl+C and run it again to load the latest server; the SQLite database stays under `%LOCALAPPDATA%\GhostChat\shared-server`, and the public URL will change. There is no genuinely unlimited free cloud server; free hosting tiers impose resource and availability limits.

## Build the Windows installer

From `crates/desktop`, run the Tauri CLI with WiX 3 installed:

```powershell
tauri build --bundles msi
```

The MSI will be written below `target/release/bundle/msi`. The application and installer need an Authenticode signature from a publisher certificate before distributing them broadly. The MSI built for this delivery is unsigned.

## Prepare the host

1. Download the standalone server asset for your platform from GitHub Releases, or build `ghostchat-server` with Rust and the system SQLite prerequisites from SQLx (SQLite is bundled in this workspace build). The automated release currently publishes the Windows x64 server binary.
2. Create a service account and a private data directory writable only by that account.
3. Set `GHOSTCHAT_DATABASE` to an absolute path inside that directory. The standalone server defaults to `GHOSTCHAT_BIND=0.0.0.0:38241` so a proxy on another machine can reach it. If the HTTPS reverse proxy runs on the same VDS, set `GHOSTCHAT_BIND=127.0.0.1:38241`. In either case, keep the raw API port blocked from public traffic in the VDS firewall; expose it through the HTTPS reverse proxy.
4. Run the server behind an HTTPS reverse proxy. Permit public traffic only to the proxy's TLS port; do not expose the raw application port. For reliable one-to-one calls across restrictive NATs, also run a public coturn TURN service on a host that accepts UDP/TCP traffic. Set `GHOSTCHAT_TURN_URLS` and `GHOSTCHAT_TURN_SHARED_SECRET` on GhostChat; use the same secret as coturn's `static-auth-secret` and enable `use-auth-secret`. Configure coturn's relay port range and firewall accordingly.
5. Group calls use a separate LiveKit SFU. Deploy LiveKit on a media-capable host with a trusted TLS domain and direct UDP reachability, then set `GHOSTCHAT_LIVEKIT_URL=wss://voice.example.com`, `GHOSTCHAT_LIVEKIT_API_KEY`, and `GHOSTCHAT_LIVEKIT_API_SECRET` only in the GhostChat server environment. Keep the API secret out of the desktop app, source code, and client-visible settings. LiveKit's default ports include TCP 7881 and UDP 3478 plus 50000–60000; configure the host firewall using LiveKit's current [ports and firewall guide](https://docs.livekit.io/transport/self-hosting/ports-firewall/). Its VM guide is tested on Ubuntu and Amazon Linux, so do not assume an older Windows Server image is a supported production host.
6. In GhostChat on each computer, set the server address to the public `https://` URL. The WebSocket endpoint upgrades from HTTPS to WSS automatically.
7. Back up SQLite using a SQLite-aware online backup tool, or stop the service cleanly before copying the database. Protect backups as readable private-message data.

## Upgrading the server

Back up before every upgrade. Migrations are one-way: there are no reverse migrations (several rebuild tables or change keys and constraints, and the desktop clients rebind their encrypted group storage at the same time), and an older server binary refuses to start on a database a newer one migrated, because SQLx stops when the database records migrations it does not know. Restoring a backup is the only way back.

Builds from 4.0.54 on back up automatically: before applying a migration numbered 0025 or higher to a database that already holds data, the server writes `<GHOSTCHAT_DATABASE>.pre-<first pending migration, four digits>.bak` in the same directory (for example `ghostchat.sqlite.pre-0033.bak`) with SQLite `VACUUM INTO`. An existing backup is not overwritten, a new database gets none, and the server exits with an error, without migrating, if the backup cannot be written (for example a full disk or a read-only directory). After a new backup has been written, older `pre-<version>.bak` files of the same database are deleted so that the three newest (by version) remain; the backup just written is never deleted, and a failed backup deletes nothing. Copy a backup elsewhere if you need it for longer. The backups contain the same private data as the database; protect them the same way. 4.0.53 and earlier shipped without this backup, so a server already on 4.0.51, 4.0.52 or 4.0.53 has passed migrations 0025-0032 and is backed up automatically only for later migrations.

To go back: stop the service, move the current database and its `-wal` and `-shm` files aside, copy the `.bak` file to the `GHOSTCHAT_DATABASE` path, and start the older binary. Data written since the backup is lost. Run the same version on the server and the desktop apps.

## Server self-update (Windows)

The standalone `ghostchat-server-windows-x64.exe` of builds from 4.0.54 on updates itself from the signed GitHub releases at startup and every 24 hours; [UPDATES.md](UPDATES.md#the-standalone-server-updates-itself) describes each step. What an operator needs to know:

- It installs only a strictly newer release whose `.sig` matches the updater public key built into the server.
- Before swapping, it writes `<GHOSTCHAT_DATABASE>.pre-update.bak` next to the database (replacing the one of the previous update); migrations of the new version still write their own `pre-<migration>.bak`.
- The previous exe is kept as `<exe>.old` in the exe's folder until the new server has started and Windows releases the file; the download is staged there as `<exe>.new` and `<exe>.new.sig`. The exe's folder must be writable by the account that runs the server.
- The old server starts the new one with the same arguments, environment and working directory and exits once the new one answers `GET /health` with its version. If it does not within two minutes, the old exe is restored and keeps serving. If the new server had already migrated the database, the old exe is not restored (it cannot open that database): the new exe stays, the old process exits with code 1, and going back means restoring both `<exe>.old` and `<GHOSTCHAT_DATABASE>.pre-update.bak`.
- Clients are disconnected for a few seconds during the swap and reconnect by themselves.
- Set `GHOSTCHAT_AUTO_UPDATE=false` to turn it off, for example when a service manager restarts the server whenever it exits (the manager would start a second copy next to the one the updater started), or when you want to choose the moment of each upgrade.
- 4.0.53 and older servers (including 4.0.52 and 4.0.53) have no self-updater: replace the exe by hand once with 4.0.54 or newer.

## Environment

`GHOSTCHAT_BIND` defaults to `0.0.0.0:38241`; use `127.0.0.1:38241` when the HTTPS reverse proxy runs on the same host. `GHOSTCHAT_DATABASE` defaults to `ghostchat.sqlite` in the process working directory; use an absolute path for deployment. `GHOSTCHAT_STUN_URLS` is a comma-separated list and defaults to Google's public STUN example. `GHOSTCHAT_TURN_URLS` is an optional comma-separated coturn URL list. Set `GHOSTCHAT_TURN_SHARED_SECRET` only on the GhostChat server; authenticated clients receive user-scoped, 12-hour TURN credentials. `GHOSTCHAT_LIVEKIT_URL`, `GHOSTCHAT_LIVEKIT_API_KEY`, and `GHOSTCHAT_LIVEKIT_API_SECRET` optionally enable group voice; only the GhostChat server signs short-lived, membership-checked LiveKit tokens. Group media passes through the configured SFU and is not end-to-end encrypted. `GHOSTCHAT_AUTO_UPDATE=false` turns off the Windows server's self-update (see above). `RUST_LOG` configures server logging. Request bodies and logs do not include authentication tokens.

See [server.env.example](../server.env.example) for placeholders. Do not place real secrets in that file. The shared TURN secret is a server-only secret.

## Operational limits

The server has health endpoint `GET /health` (it reports `status`, `service` and, from 4.0.54 on, the server `version`; the desktop names that version when a feature needs a newer server), SQLite WAL mode, versioned migrations, a five-second SQLite busy timeout, and graceful Ctrl+C shutdown. It does not include a TLS listener, clustered presence, replicated rate limits, scheduled backups (it only backs up before a migration and keeps the three newest of those), metrics, or an operations dashboard. Use one server process per database.

Run exactly one server process. Calls through the GhostChat server (the server media relay, `/v1/media/ws`, see [VOICE.md](VOICE.md)) keep their call rooms in that process's memory: the two ends of a call, or the members of a group call, must reach the same process to hear each other. Behind a load balancer with several replicas a call would be split and stay silent. The Windows server exe and the desktop app's built-in server are single processes, so they need nothing extra. Restarting the server ends the relayed calls in progress; the callers call again. A reverse proxy must pass WebSocket upgrades on `/v1/media/ws` as it does on `/v1/ws`.
