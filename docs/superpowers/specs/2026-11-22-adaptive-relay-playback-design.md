# Adaptive playback buffer for server-relayed calls

Date: 2026-11-22. Status: decided by the controller under the owner's standing instruction ("do it as you see fit, use the skills in full"). Branch: `claude/dreamy-knuth-j0wr07`.

## Intent

Calls that run through the GhostChat server relay (`/v1/media/ws`) stutter. The owner wants the voice smooth and uninterrupted, with no LiveKit or other extra service, using the existing server.

## Diagnosis (from reading the code, 2026-11-22)

- The relay carries audio as binary WebSocket frames, so it runs over TCP. TCP delivers in order: one late packet holds back every packet behind it. This is the stall source. A stall of 200 ms or more is common on real networks.
- Server limits are not the cause. Audio is about 4 KB/s (Opus 32 kb/s, 20 ms frames). The audio budget is 64 KB/s with a 128 KB burst (`media_relay.rs`, `AUDIO_BYTE_RATE`). Frame count limit is 100/s.
- Sender side (`server-media-client.js`, `MAX_BUFFERED_BYTES = 4 KB`): frames are dropped when the socket's `bufferedAmount` exceeds about 1 s of audio. This only triggers in a real stall.
- Receiver side (`server-media-worklet.js`, `ghostchat-relay-playback`): a fixed jitter buffer. It starts after 60 ms (`START_SAMPLES = 3 frames`), keeps at most 200 ms (`MAX_SAMPLES = 10 frames`) and drops the oldest audio beyond that. On underrun it waits for another 60 ms before resuming.
- Result: a stall longer than about 200 ms loses audio (trimmed) and then restarts with a gap. A stall between 60 and 200 ms is absorbed. The buffer is too small for the stalls TCP produces, and it never grows.

So the fix is an **adaptive** buffer: it grows when stalls happen, so the stall becomes a short extra delay instead of a gap, and it shrinks back slowly so latency does not stay high.

## Goals

- A stall of up to about 1 s on the relay path produces no audible gap. The cost is extra latency during and shortly after the stall.
- Latency returns to the normal target (about 60 ms) within about 10 s of a quiet connection.
- Nothing audible changes on a clean connection: same start delay and same frames.
- The behaviour is covered by Node tests with the existing `vm` fixtures (`scripts/test-server-media.cjs`).

## Non-goals

- No protocol change. The server, the frame format and the sequence numbers stay as they are.
- No change to the sender's encoder or the 4 KB send threshold. Dropping while the socket is stalled is still right, because queued audio only adds latency. (Revisit with measurements in stage 2.)
- No transport change (WebTransport/QUIC, UDP). That is stage 2 (below).
- No change to P2P, LiveKit or group-room logic.

## Constraints

- Plain JS in an AudioWorklet, loaded from `server-media-worklet.js`. The Node tests load it through a fake `AudioWorkletProcessor`.
- Desktop CSP forbids blob scripts; the worklet file stays where it is.
- Tests must run on Linux with `node scripts/test-server-media.cjs`. Real stall behaviour needs two Windows PCs and a real tunnel; that is `needs Windows verification`.

## Approaches considered

- **A. Adaptive jitter buffer in the playback worklet (chosen for stage 1).** Target depth grows on underrun and shrinks slowly. Fits the existing structure, one file, no protocol change, testable here. Trade-off: still TCP, so very long stalls (over about 1 s) still gap.
- **B. WebTransport (QUIC datagrams) to the server.** Packets do not wait for each other, so this is the real fix for head-of-line stalls. Needs UDP reachable on the VDS (not verified; Windows Server 2012 R2 host), a TLS certificate for a voice subdomain, a QUIC stack in the Rust server, and WebView2 support. Large, with infrastructure risk. Stage 2, after stage 1 is measured.
- **C. Raw UDP audio from the Rust desktop side.** Full control, but we would write our own encryption and NAT handling. Rejected: most work, most risk.

Chosen: **A now, B later**, because A fixes the audible gaps with a small, testable change and B depends on infrastructure we have not verified.

## Design (stage 1)

Playback worklet, per peer:

1. **Target depth.** `targetSamples` starts at 3 frames (60 ms). Maximum is 30 frames (600 ms).
2. **Underrun.** When the queue is empty while playing, the worklet plays silence (as now) and raises the target by 2 frames (40 ms). It then waits until the target is buffered before resuming. This replaces the fixed 60 ms restart.
3. **Recovery.** After 250 consecutive frames (5 s) without an underrun, the target drops by 1 frame, down to the 3-frame minimum.
4. **Cap.** The queue may hold up to the current target plus 10 frames of slack (about 200 ms beyond target). Only audio beyond that cap is dropped, oldest first. This keeps latency bounded even if the sender bursts.
5. **Stats.** The worklet posts a small stats object on request (underruns, current target in ms, queue depth in ms), so the UI or a test can read it. Not shown in UI in stage 1.

Frames are not renumbered. The worklet does not look at sequence numbers; ordering is guaranteed by TCP.

## Parts (builder)

- `crates/desktop/ui/server-media-worklet.js`: the playback processor changes above.
- `scripts/test-server-media.cjs`: new checks for the worklet (see Tests).
- No other files. `server-media-client.js` is untouched.

## Tests (Node, `scripts/test-server-media.cjs`)

- Clean stream: 60 ms start, output identical to the current behaviour (existing checks still pass).
- Underrun raises the target by 40 ms and resumes only after the new target is buffered.
- Target never exceeds 600 ms and never drops below 60 ms.
- After 250 underrun-free frames the target falls by one frame.
- A burst of many frames is capped at target + 200 ms; the oldest audio is the part dropped.
- Stats report underruns and the current target.

## Verification

- `node scripts/test-server-media.cjs` and the rest of `scripts/test-*.cjs`: green.
- `node --check crates/desktop/ui/*.js`: green.
- `cargo fmt --all -- --check` and `cargo test --locked -p ghostchat-server`: unchanged code, must stay green.
- `git diff --check`: clean.
- Windows (`needs Windows verification`): two PCs, relay call through the tunnel, 5-minute call, count audible gaps; compare with 4.0.64 build.

## Stage 2 (not in this plan)

WebTransport over QUIC for the relay, gated on: UDP 443 (or another port) reachable on the VDS, a certificate for a voice subdomain, and a Windows test. Reviewed separately after stage 1 numbers are in.

## Left to the builder

Exact code of the worklet changes and the test harness shape, as long as the tests above exist.
