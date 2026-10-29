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

The desktop bundles the official `livekit-client` JavaScript SDK and its Apache-2.0 license. Production use still requires a reachable LiveKit deployment with trusted TLS and working WebRTC firewall rules. Do not claim group media is end-to-end encrypted. Group camera calls are not implemented.

Without TURN or a reachable SFU, voice may work on some networks but is not guaranteed across restrictive NATs. One-to-one calls remain peer-to-peer; group calls are routed through LiveKit.
