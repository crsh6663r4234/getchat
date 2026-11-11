# Voice calls and screen sharing

GhostChat supports one-to-one WebRTC calls and group calls through LiveKit's SFU. Direct calls use the browser's native WebRTC stack in Windows WebView2; the application server only relays authenticated call setup messages. Group media is published to the configured LiveKit SFU, which routes audio and screen-share tracks to the group. Direct and group media are encrypted in transit, but group calls are not end-to-end encrypted: the SFU terminates each participant's media transport.

The [WebRTC peer-connection guide](https://webrtc.org/getting-started/peer-connections) describes the offer/answer, signaling, and ICE candidate exchange used here. Display capture follows the [W3C Screen Capture specification](https://www.w3.org/TR/screen-capture/) and uses the standards-based system source picker. GhostChat does not choose a display for the user or start capture without the user's click and selection. Microsoft documents WebView2's [microphone permission](https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/winrt/microsoft_web_webview2_core/corewebview2permissionkind) and [`getDisplayMedia()` screen-capture event](https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/icorewebview2screencapturestarting).

## User settings

Open **Settings → Voice and screen sharing** to select the microphone and speaker, enable echo cancellation, noise suppression and automatic microphone gain, set a remote volume level, or use push-to-talk (hold Space while the chat is focused). Screen sharing starts from a connected call; the Windows display picker lets the user choose a screen or window. System audio sharing is optional and off by default. Stop sharing with the screen button or the operating system's capture indicator.

The browser's audio processing controls are capture constraints; the available behavior depends on the selected audio device and WebView2 version. Screen and microphone capture need Windows permissions. If WebView2 or Windows denies access, GhostChat shows an error and stops the attempted capture.

## Network and privacy

The server gives clients its ICE configuration through authenticated `GET /v1/voice/ice`. By default, it includes Google's public STUN example; that service sees the network address making the STUN request. Direct WebRTC can also reveal each caller's public network address to the other caller, and it can fail when a router, firewall, or carrier-grade NAT blocks the direct path. The server relays but does not persist the call setup messages, which include ICE candidates and connection metadata.

For improved connectivity and to keep peer addresses hidden from each other, configure a public coturn server using TURN REST shared-secret authentication. The GhostChat server creates user-specific credentials that expire after 12 hours; the shared secret stays on the server. The two sides must be able to reach the TURN listener and relay UDP port range. Configure **Relay-only** only after the app reports that TURN is available.

Example GhostChat server environment:

```dotenv
GHOSTCHAT_STUN_URLS=stun:stun.l.google.com:19302
GHOSTCHAT_TURN_URLS=turn:turn.example.com:3478?transport=udp,turn:turn.example.com:3478?transport=tcp
GHOSTCHAT_TURN_SHARED_SECRET=replace-with-a-long-random-server-only-secret
GHOSTCHAT_LIVEKIT_URL=wss://voice.example.com
GHOSTCHAT_LIVEKIT_API_KEY=server-only-api-key
GHOSTCHAT_LIVEKIT_API_SECRET=server-only-api-secret
```

Configure coturn with the same secret, `use-auth-secret`, a public DNS name, a relay port range, and firewall rules for the listener and relay ports. See the [coturn TURN REST authentication documentation](https://github.com/coturn/coturn/blob/master/README.turnserver). Do not put the shared secret in the client or publish it in a repository. TURN hosting consumes bandwidth and is not an unlimited free cloud service. A Cloudflare Quick Tunnel only carries the GhostChat HTTP/WebSocket signaling path; it does not act as a TURN media relay.

Group calls require all three LiveKit settings above. The authenticated endpoint issues a short-lived room token only to current group members; the API secret never reaches the desktop. Participants join with their microphone muted, can choose input/output devices, use echo cancellation/noise suppression/automatic gain control, mute individual remote speakers locally, and share/refresh their screen. Joining a room itself does not turn on the microphone. Group screen shares are SFU-relayed and share system audio only when the platform exposes it. When a member leaves or is removed, GhostChat asks LiveKit to disconnect that account's active sessions and also sends the normal-client membership update. Self-hosted LiveKit does not revoke an already-issued participant token, so that token can potentially be reused to reconnect until it expires; deployment and this behavior require real-server validation before group calling is broadly enabled.

When LiveKit is configured, a one-to-one call uses a separate short-lived **GhostShare** room for screen publishing. Voice stays on the existing call, while the SFU can adapt the screen layers independently. The desktop offers Sharp text, Balanced, Smooth motion, and Data saver profiles; it also keeps an adaptive direct-share fallback for a server without LiveKit. GhostShare is SFU-relayed, encrypted in transit, and not end-to-end encrypted.

The desktop bundles the official `livekit-client` JavaScript SDK and its Apache-2.0 license. Production use still requires a reachable LiveKit deployment with trusted TLS and working WebRTC firewall rules. Do not claim group media is end-to-end encrypted. Group camera calls are not implemented.

Without TURN or a reachable SFU, voice may work on some networks but is not guaranteed across restrictive NATs. One-to-one calls remain peer-to-peer; group calls are routed through LiveKit.

## Calls through the GhostChat server (server media relay)

TURN and LiveKit carry media over UDP (or TCP to their own ports). When the GhostChat server is reached only through an HTTP tunnel such as Cloudflare, those relays are unreachable and calls used to hang at "connecting". The server therefore also relays call audio itself, over an authenticated WebSocket (`/v1/media/ws`) on the same address the app already uses. `GET /v1/voice/ice` reports `"media_relay": true` on servers that offer it.

Path selection, with no setting to change:

- **No TURN configured** (direct calls) or **no LiveKit configured** (group calls): the call uses the server relay from the start.
- **TURN or LiveKit configured**: it is tried first. If it has not connected about 8 seconds after the call was answered (or the group was joined), or it fails, the call switches to the server relay automatically. A call that connected through TURN or LiveKit never uses the relay.
- **An older server without the relay**: calls behave as before (TURN or LiveKit is required).

On the relay path the desktop records the microphone, encodes it with Opus (WebCodecs, 32 kb/s mono, 20 ms frames) and sends the frames to the server, which forwards them to the other participants of that call; each peer's audio is decoded and played through the normal call audio output, so the speaker device, volume and per-peer mute apply. Mute, push-to-talk and microphone changes work as on the other paths. The other caller never learns your network address, because neither side connects to the other directly. The server checks the same rights as call signaling: a direct call room admits only the two members of that conversation while neither blocks the other; a group room admits only current members. A block or a removal from the group stops that user's audio immediately in both directions. Each connection is limited to 64 KB per frame, 100 frames and 256 KB per second.

**Privacy: relayed media passes through the GhostChat server and is not end-to-end encrypted**, exactly like TURN and LiveKit. The server operator can access the audio while it is relayed (it is encrypted in transit only by the HTTPS/WSS connection to the server or tunnel, and a tunnel provider such as Cloudflare terminates that TLS). The server does not store relayed media.

Relayed audio adds the latency of the path to the server and back (and through the tunnel), typically a little more than TURN. Each participant's upload is about 40 kb/s; the server forwards one copy to each other participant, so a large group call costs the server more upload bandwidth than a direct call.

Screen sharing over the server relay is not available yet; on a relayed call the share button explains this.
