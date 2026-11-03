# Watch Party (4.0.43)

Available inside a connected one-to-one call. The old music playlist is removed.

- Direct HTTPS video: each participant loads the URL locally; play, pause and
  seeking are relayed through authenticated call signaling. It must be a direct
  supported video file, not a YouTube/Netflix page or protected stream.
- Local video: both participants select matching local files. The video bytes
  stay on each device. A size and SHA-256 fingerprint of the first 64 KiB are
  shared to match the selection; this is not full-file integrity verification.
- App/window: opens GhostShare preparation and the existing Windows display
  picker. This streams the selected source rather than synchronizing a third
  party app's player. System sound is available only when the chosen capture
  source and platform support it. Protected content may not be capturable.

Opening Watch Party is required before an incoming video source is loaded. The
server never downloads a source. Source controls are scoped to the current peer
and direct conversation; sender membership, payload bounds and revision checks
apply. End session clears the peer state. Ending the call clears local media.

Update the VDS server to 4.0.43 for the watch_party signaling type. Group Watch
Party and automatic file distribution are not implemented. Two-device video,
buffering and real network behavior still need validation.

## GhostShare scope

The new preparation dialog is an application-owned desktop interface with
quality/system-sound choices. Capture still uses WebView2 getDisplayMedia and
the configured LiveKit SFU, with direct compatibility fallback. This release
does **not** implement Windows Graphics Capture/DXGI or a native encoder. A
native capture rewrite requires a separate capture/encoding/transport path.

## Checks

`node scripts/test-watch-party.cjs` exercises two simulated clients, ordering,
stop/resync, local source markers and peer scoping. Rust tests validate payloads.
The local visual fixture covers source selection and GhostShare dialog opening;
it does not prove real call or media delivery.
