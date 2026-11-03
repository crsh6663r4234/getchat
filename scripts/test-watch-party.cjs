const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const assert = require('node:assert/strict');

let monotonic = 1_000;
let wallClock = 1_800_000_000_000;
const sandbox = { window: {}, URL, Date: { now: () => wallClock }, performance: { now: () => monotonic } };
vm.createContext(sandbox);
vm.runInContext(fs.readFileSync(path.join(__dirname, '../crates/desktop/ui/watch-party.js'), 'utf8'), sandbox);
const { create, validateSource } = sandbox.window.GhostChatWatchParty;
function endpoint(id, peer) {
  const sent = [];
  const changes = [];
  const call = { status: 'connected', conversationId: 'conversation', peerId: peer };
  const client = create({ getCall: () => call, getUserId: () => id, sendSignal: signal => sent.push(signal), onChange: state => changes.push(state) });
  return { id, sent, changes, call, client };
}
function deliver(from, to, index = from.sent.length - 1) {
  const signal = from.sent[index];
  return to.client.handleSignal({ kind: signal.kind, conversation_id: signal.conversationId, from_user_id: from.id, payload: signal.payload });
}

const A = endpoint('a', 'b');
const B = endpoint('b', 'a');
const marker = 'https://local.watch.ghostchat.invalid/1048576-0123456789abcdef.mp4';
assert.equal(validateSource(marker), marker, 'Local-file identifiers are valid transport markers');
A.client.setSource(marker);
assert.equal(deliver(A, B), true);
assert.equal(B.client.getState().source, marker);
assert.equal(B.sent.length, 0, 'Remote state application must never echo');
assert.equal(deliver(A, B), false, 'Duplicate source events must be idempotent');

A.client.play(12);
monotonic += 2_000; wallClock += 2_000;
assert.equal(deliver(A, B), true);
assert.equal(B.client.getState().playing, true);
assert.equal(B.client.getState().position, 13.5, 'Transit compensation is capped at 1.5 seconds');

// Selecting the same local file later asks the peer for its current timeline.
const originalActor = B.client.getState().actor;
B.client.requestState();
assert.equal(B.sent.at(-1).payload.action, 'request');
assert.equal(deliver(B, A), true);
assert.equal(A.sent.at(-1).payload.action, 'state');
assert.equal(deliver(A, B), true, 'Requested same-revision state must resync');
assert.equal(B.client.getState().actor, originalActor, 'Resync does not reassign revision authority');
assert.equal(deliver(A, B), false, 'The same state response is accepted only once');

A.client.stop();
assert.equal(A.sent.at(-1).payload.source, '');
assert.equal(deliver(A, B), true);
assert.equal(B.client.getState().source, '');
assert.equal(B.client.getState().playing, false);
assert.equal(B.client.getState().position, 0);
// A peer that missed the stop receives the cleared state on request as well.
B.client.requestState();
const countBeforeRequest = A.sent.length;
assert.equal(deliver(B, A), true);
assert.equal(A.sent.length, countBeforeRequest + 1);
assert.equal(A.sent.at(-1).payload.source, '');
assert.equal(deliver(A, B), true);

const C = endpoint('a', 'b');
const D = endpoint('b', 'a');
C.client.setSource('https://example.com/a.webm');
D.client.setSource('https://example.com/b.mp4');
assert.equal(deliver(D, C), true, 'Concurrent equal revisions use deterministic user-ID ties');
assert.equal(deliver(C, D, 0), false);
assert.equal(C.client.getState().source, D.client.getState().source);
assert.equal(C.client.handleSignal({ kind: 'watch_party', conversation_id: 'conversation', from_user_id: 'outsider', payload: D.sent[0].payload }), false);
assert.equal(C.client.handleSignal({ kind: 'watch_party', conversation_id: 'other', from_user_id: 'b', payload: D.sent[0].payload }), false);
assert.equal(C.client.handleSignal({ kind: 'watch_party', conversation_id: 'conversation', from_user_id: 'b', payload: { ...D.sent[0].payload, revision: 1_000_000_001 } }), false);
assert.throws(() => C.client.seek(Infinity));
assert.throws(() => C.client.seek(-1));
assert.throws(() => C.client.setSource('http://example.com/video.mp4'));
assert.throws(() => C.client.setSource('https://example.com/watch?id=123'));
assert.throws(() => C.client.setSource('https://127.0.0.1/video.mp4'));
assert.throws(() => C.client.setSource('https://user:password@example.com/video.mp4'));
C.call.status = 'idle';
assert.throws(() => C.client.play(0));
C.client.reset();
assert.equal(C.client.getState().source, '');
assert.equal(C.client.getState().revision, 0);
console.log('Watch Party checks passed: local marker, sync, stop, requests, revision ties, peer scoping, validation and no echoes.');
