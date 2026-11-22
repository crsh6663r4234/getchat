# Adaptive relay playback buffer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make server-relayed call audio stop gapping after TCP stalls by growing the playback jitter buffer when it underruns, and shrinking it back when the stream is calm.

**Architecture:** The playback AudioWorklet (`ghostchat-relay-playback`) keeps a target depth in frames instead of fixed constants. Underrun raises the target by two frames, calm periods lower it by one, the queue is capped at target plus ten frames. No protocol or client changes.

**Tech Stack:** Plain browser JavaScript (AudioWorklet), Node `vm` fixtures in `scripts/test-server-media.cjs`.

**Spec:** `docs/superpowers/specs/2026-11-22-adaptive-relay-playback-design.md`

## Global Constraints

- Frame is 960 samples (20 ms at 48 kHz). Target bounds: minimum 3 frames (60 ms), maximum 30 frames (600 ms).
- Underrun raises target by 2 frames. Calm is 250 consecutive frames without underrun, then target drops by 1 frame.
- Queue cap is target + 10 frames; beyond it oldest audio is dropped.
- No change to `server-media-client.js`, the server, or the frame format.
- Green before every commit: `node scripts/test-server-media.cjs`, `for f in scripts/test-*.cjs; do node "$f"; done`, `for f in crates/desktop/ui/*.js; do node --check "$f"; done`, `git diff --check`. (`cargo fmt --all -- --check` and `cargo test --locked -p ghostchat-server` are unaffected; run once at the end.)

## Review Focus

- A long stall (over 1 s) then resume: target must stay at the 600 ms maximum and latency must not grow without bound. Test in Task 1.
- Clean stream after a stall: target must return to 60 ms, so latency does not stay high. Test in Task 1.
- Burst of frames while playing: queue must not exceed cap, and the oldest audio is the part dropped. Test in Task 1.
- Stats must report underruns and the current target in milliseconds (for measuring on Windows). Test in Task 1.

---

### Task 1: Adaptive target depth in the playback worklet

**Files:**
- Modify: `crates/desktop/ui/server-media-worklet.js` (the `RelayPlayback` class and its constants)
- Test: `scripts/test-server-media.cjs` (extend the existing worklet block near line 434)

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: the worklet message `{ type: "stats" }` posted to the playback node port replies with `{ type: "stats", underruns: number, targetMs: number, queueMs: number }`. Audio messages stay plain `Float32Array`s, unchanged.

- [ ] **Step 1: Write the failing tests**

Extend the worklet block in `scripts/test-server-media.cjs` (it already builds `playback` with the fake `AudioWorkletProcessor`; `out()` pulls one 128-sample render quantum). Add these checks, each with its own `assert` message:

```js
// Constants the tests assume (mirror the worklet's documented values).
// FRAME = 960, minimum target 3 frames, maximum 30, step 2, calm 250.
const frame = () => new Float32Array(960).fill(0.25);
const pull = (node, quanta) => { for (let i = 0; i < quanta; i++) node.process([], [[new Float32Array(128)]]); };

// 1. Underrun raises the target and resumes only after the new target is buffered.
const p1 = new processors['ghostchat-relay-playback']();
p1.port.onmessage({ data: frame() }); p1.port.onmessage({ data: frame() }); p1.port.onmessage({ data: frame() });
pull(p1, 60); // play the 3 frames (2880 samples = 22.5 quanta), then underrun
assert.equal(stats(p1).targetMs, 100, 'underrun raises the target from 60 ms to 100 ms');
p1.port.onmessage({ data: frame() }); p1.port.onmessage({ data: frame() }); // 2 frames = 40 ms < 100 ms
assert.equal(out(p1)[0], 0, 'playback waits until the new target is buffered');
p1.port.onmessage({ data: frame() }); p1.port.onmessage({ data: frame() }); p1.port.onmessage({ data: frame() }); // 5 frames total >= 5 frames
assert.equal(out(p1)[0], 0.25, 'playback resumes once the raised target is buffered');

// 2. Target is bounded: 600 ms maximum, 60 ms minimum.
// (Repeat underruns until the cap; assert stats(p).targetMs === 600 and never more.)

// 3. Calm stream lowers the target by 20 ms after 250 underrun-free frames.
// (Start from a raised target, feed 250 frames without underrun, assert the target drops by one frame.)

// 4. Cap: a burst while playing keeps at most target + 10 frames queued; oldest dropped.
// assert playback.buffered - playback.offset <= (targetFrames + 10) * 960

// 5. stats message reports underruns and targetMs.
p1.port.onmessage({ data: { type: 'stats' } }) // the reply is posted to port.posted (or a stats callback) and has fields underruns, targetMs, queueMs
```

Also keep the existing assertions at lines 453–455 (clean start at 60 ms, cap of 200 ms) passing; they cover the initial target and the first cap.

Put the helpers `stats(node)` and `out(node)` in the test block. `stats(node)` returns the last object in `node.port.posted` whose `type === 'stats'`; the worklet posts it with `this.port.postMessage({ type: 'stats', ... })`.

- [ ] **Step 2: Run the test to verify it fails**

Run: `node scripts/test-server-media.cjs`
Expected: FAIL on the new checks (the worklet has no adaptive target or stats yet). The existing checks still pass.

- [ ] **Step 3: Implement the adaptive target in `RelayPlayback`**

In `server-media-worklet.js`, replace the constants `START_SAMPLES` and `MAX_SAMPLES` with frame-count constants: `MIN_TARGET_FRAMES = 3`, `MAX_TARGET_FRAMES = 30`, `TARGET_STEP_FRAMES = 2`, `CALM_FRAMES = 250`, `SLACK_FRAMES = 10`. Keep `FRAME = 960`.

Per-node state: `targetFrames` (starts at 3), `calmFrames` (0), `underruns` (0).

- In `process`, when the queue runs dry while `playing` (the existing `written < output.length` branch), increment `underruns`, set `targetFrames = min(MAX, targetFrames + 2)`, reset `calmFrames = 0`, set `playing = false`.
- When a render quantum plays without underrun, increment `calmFrames` by one frame's worth; when it reaches 250, lower `targetFrames` by one (not below 3) and reset `calmFrames`. Count in frames (960 samples), not quanta.
- Start condition: `if (!this.playing && available < targetFrames * FRAME) return true;`
- Cap in the message handler: keep `this.buffered - this.offset <= (targetFrames + SLACK_FRAMES) * FRAME`; drop oldest queue entries beyond that, as now.
- Add the stats reply: in `port.onmessage`, if `event.data?.type === 'stats'`, post `{ type: 'stats', underruns, targetMs: targetFrames * 20, queueMs: (buffered - offset) / 48 }`. Return before the audio branch.

Keep the existing audio-message path unchanged for `Float32Array` data.

- [ ] **Step 4: Run the test to verify it passes**

Run: `node scripts/test-server-media.cjs`
Expected: PASS (all existing and new checks).

- [ ] **Step 5: Run every Global Constraints check**

Run:
```bash
for f in scripts/test-*.cjs; do node "$f" || exit 1; done
for f in crates/desktop/ui/*.js; do node --check "$f" || exit 1; done
git diff --check
```
Expected: no output from `git diff --check`; every script exits 0.

- [ ] **Step 6: Commit**

```bash
git add crates/desktop/ui/server-media-worklet.js scripts/test-server-media.cjs
git commit -m "fix(calls): adaptive playback buffer for server-relayed audio"
```

## Final verification (after Task 1)

- [ ] Run `cargo fmt --all -- --check` and `cargo test --locked -p ghostchat-server`. Expected: pass (no Rust changed).
- [ ] Note in the report: real stall behaviour needs `needs Windows verification` (two PCs, relay call through the tunnel, 5-minute call, count gaps).
