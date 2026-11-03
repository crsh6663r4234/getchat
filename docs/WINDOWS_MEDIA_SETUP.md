# Windows VDS media setup

GhostChat's HTTP server and Caddy can work while group voice remains disabled.
Group voice requires a separately running LiveKit media server and these three
settings in the **GhostChat server process**, not the desktop client:

```dotenv
GHOSTCHAT_LIVEKIT_URL=wss://your-media-host
GHOSTCHAT_LIVEKIT_API_KEY=your-private-server-key
GHOSTCHAT_LIVEKIT_API_SECRET=your-private-server-secret
```

Do not use the example values or LiveKit's development key on a public server.
Restart the GhostChat server from a new session after configuring machine-level
environment variables. A scheduled task or service must also receive the new
environment. Preserve the existing database path when restarting.

## Check the existing VDS first

Copy `scripts/Test-GhostChatMedia.ps1` to the VDS and run in PowerShell:

```powershell
powershell.exe -NoProfile -File .\Test-GhostChatMedia.ps1
```

The diagnostic reports whether settings exist without displaying credentials.
If testing a local executable in another location, pass `-LiveKitExe` with its
full path. It tries `--version` before any installation or firewall change.
Windows Server 2012 R2 compatibility has **not** been established for current
LiveKit builds. A Windows download does not establish support for that old OS.

## Self-hosted media network

Use an official Windows release from https://github.com/livekit/livekit/releases.
First verify that it runs on the VDS. Then create private production credentials
and configure LiveKit, with its API listener behind Caddy's TLS endpoint.
Do not replace the existing GhostChat Caddy site. Give media its own hostname
pointing to the same VDS and reverse proxy that hostname to `127.0.0.1:7880`.

LiveKit media must also reach the VDS directly: TCP 7881 and either the configured
UDP range (default 50000–60000) or an explicitly configured UDP mux port (7882).
Opening Caddy's HTTPS port alone is insufficient. Provider firewall rules must
match Windows firewall rules. Existing coturn credentials are separate from
LiveKit API credentials and must not be substituted for them.

Official references:
- https://docs.livekit.io/transport/self-hosting/local/
- https://docs.livekit.io/transport/self-hosting/deployment/
- https://docs.livekit.io/transport/self-hosting/ports-firewall/

## Completion checks

The current application checks only whether all three settings are configured
before enabling group join. This is not a LiveKit health check. Confirm the
media hostname has trusted TLS, then test two devices joining the same group,
both audio directions, screen start/stop, refresh and reconnect. No working
deployment or call test is implied by adding environment variables alone.
