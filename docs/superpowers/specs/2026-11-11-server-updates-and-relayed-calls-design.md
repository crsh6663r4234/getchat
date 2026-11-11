# Server self-update, client/server compatibility, and calls through the server

Date: 2026-11-11. Approved by the owner in conversation (both "recommended" options).

## Why

The owner runs the standalone `ghostchat-server-windows-x64.exe` on a Windows PC, reached through a Cloudflare tunnel (HTTP only). Two problems in real use:

1. Every desktop release forces a manual server update, and a client newer than its server produces server errors.
2. Since 4.0.50 direct and group calls are relay-only (TURN / LiveKit). Those relays need UDP, which the tunnel cannot carry, so calls hang at "connecting". Before 4.0.50 direct calls fell back to a direct connection.

## Part A: server updates itself

- The server checks the GitHub release feed (`https://github.com/crsh6663r4234/getchat/releases/latest`) at startup and every 24 hours. It runs only for the Windows standalone exe, not inside the desktop app's built-in loopback server. It can be turned off with `GHOSTCHAT_AUTO_UPDATE=false`.
- Authenticity: the release workflow signs `ghostchat-server-windows-x64.exe` with the same minisign updater key it already uses for the MSI (`tauri signer sign`) and uploads the `.sig`. The server embeds the public key that is already in `crates/desktop/tauri.conf.json` and verifies the signature before using the file. The sha256 file is not trusted on its own.
- Install: download to a temp file next to the exe, verify, take the database backup (the existing pre-upgrade `VACUUM INTO` backup logic), then rename the running exe to `.old` (Windows allows renaming a running exe), move the new one into place, start it with the same arguments, environment and working directory, and exit. The new process deletes a leftover `.old` on startup. If the new exe fails its health check, restore the old one.
- Only update to a higher version; never downgrade. Log every step.

## Part B: compatibility

- Server exposes its version in `/health` (or a `/v1/version`) response.
- The desktop treats server features as optional: if a field or route added in 4.0.51/4.0.52 is missing on an older server, the client keeps working with the old behavior instead of failing. Where it cannot (for example encrypted-group commit sequencing), it shows a clear translated message: "This server runs GhostChat X; update the server to use Y." instead of a generic error.
- The server keeps accepting requests from older clients that omit new fields (`expected_commit_seq` etc.), as it does now; confirm with tests.

## Part C: calls through the server ("server media relay")

- New fallback media path: audio is captured, encoded with WebCodecs (Opus, `AudioEncoder`), and sent as binary frames over a dedicated authenticated WebSocket on the GhostChat server; the server forwards frames to the other participants of that call (1:1 or group room), decoded with `AudioDecoder` and played through an AudioWorklet. This works through the HTTP tunnel and never exposes the user's IP to peers.
- Path selection: use TURN (direct calls) / LiveKit (group calls) when the server has them configured and they connect; otherwise, or when they have not connected within a short timeout (about 8 s), switch to the server media relay automatically. If neither TURN nor LiveKit is configured, use the server relay from the start.
- Authorization: same membership and block checks as signaling; group relay rooms only for current members; frames from blocked or removed users dropped. Rate-limit frame size and rate per connection.
- Privacy: like TURN and LiveKit today, the server relays media; frames are not end-to-end encrypted. Document it in SECURITY.md and VOICE.md.
- Existing call UI, mute, push-to-talk, device selection, ringtone, invisible-mode rules and the connect timeout keep working on the relay path.
- Screen sharing over the server relay (WebCodecs `VideoEncoder`, low frame rate) is phase 2 of this part, done after audio works.

## Non-goals

No Linux/macOS server self-update packaging, no new desktop features beyond the relay path, no changes to the MLS text encryption.

## Verification

Unit tests for the server updater (signature check, no downgrade, rollback on failed health check — with the network and process swap behind injectable traits), route tests for the relay WebSocket (auth, membership, blocking, forwarding, rate limits), Node vm tests for path selection and the relay client. Real two-PC calls through a Cloudflare tunnel need Windows verification.
