const fs = require('node:fs'), vm = require('node:vm'), assert = require('node:assert/strict');
const ui = require('node:path').resolve(__dirname, '../crates/desktop/ui');
const deferred = () => { let resolve; const promise = new Promise(r => resolve = r); return { promise, resolve }; };
function session() {
  const track = { enabled: false, readyState: 'live', kind: 'audio', stop() { this.readyState = 'ended'; } };
  const stream = { getAudioTracks: () => [track], getTracks: () => [track] };
  return { stream, rawStream: stream, status: { state: 'running', engine: 'rnnoise' }, setMuted(value) { track.enabled = !value; }, async close() { this.closed = true; track.stop(); } };
}
const settings = { pushToTalk: false };
function fixture(group) {
  const sessions = [], sent = []; let pending = null, fail = false;
  const engine = { async open() { if (fail) throw Error('device unavailable'); const s = session(); sessions.push(s); if (pending) { const wait = pending; pending = null; await wait.promise; } return s; } };
  const api = { getSettings: () => settings, getUserId: () => 'a', onChange() {}, onError() {}, onIncoming() {}, sendSignal: x => sent.push(x), getDisplayName: () => 'A', getAvatar: () => null, isPeerMuted: () => false, getMembers: async () => [], getToken: async () => ({}), onScreenRemoved() {}, getIceConfiguration: async () => ({ turn_available: true, ice_servers: [{ urls: 'turn:fixture', username: 'x', credential: 'y' }] }) };
  class Peer {
    transceivers = []; signalingState = 'stable'; connectionState = 'new';
    addTransceiver(kind, options) { const t = { direction: options.direction, receiver: { track: { kind } }, sender: { track: null, async replaceTrack(track) { this.track = track; }, getParameters: () => ({ encodings: [] }) } }; this.transceivers.push(t); return t; }
    getTransceivers() { return this.transceivers; }
    async createOffer() { return { type: 'offer', sdp: 'fixture' }; }
    async setLocalDescription(value) { this.localDescription = value; }
    close() {}
  }
  let published;
  class Track { constructor(track) { this.mediaStreamTrack = track; } async mute() { this.mediaStreamTrack.enabled = false; } async unmute() { this.mediaStreamTrack.enabled = true; } async replaceTrack(track) { this.mediaStreamTrack = track; } }
  class Room {
    remoteParticipants = new Map(); events = new Map();
    localParticipant = { identity: 'a', isMicrophoneEnabled: false, async publishTrack(track) { published = track; }, async unpublishTrack() {}, setMicrophoneEnabled() { throw Error('SDK capture bypass forbidden'); } };
    on(name, handler) { this.events.set(name, handler); return this; }
    async connect() {} async startAudio() {} async disconnect() { this.events.get('Disconnected')?.(); }
    static async getLocalDevices() { return [{}]; }
  }
  const window = { GhostChatAudio: engine, LivekitClient: { Room, LocalAudioTrack: Track, RoomEvent: new Proxy({}, { get: (_, key) => key }), Track: { Source: { Microphone: 'microphone' }, Kind: { Audio: 'audio', Video: 'video' } } } };
  vm.runInNewContext(fs.readFileSync(ui + (group ? '/group-voice-client.js' : '/voice-client.js'), 'utf8'), { window, RTCPeerConnection: Peer, console, setTimeout: () => 1, clearTimeout() {}, setInterval: () => 1, clearInterval() {} });
  const client = (group ? window.GhostChatGroupVoice : window.GhostChatVoice).create(api);
  return { client, sessions, sent, published: () => published, delay() { pending = deferred(); return pending; }, fail(value) { fail = value; } };
}
(async () => {
  for (const group of [false, true]) {
    settings.pushToTalk = false;
    const f = fixture(group), c = f.client;
    await (group ? c.join({ id: 'chat' }) : c.start({ id: 'chat', peer_id: 'b' }));
    if (group) { assert.equal(f.sessions.length, 0); await c.toggleMicrophone(); }
    assert(f.sessions[0].stream.getAudioTracks()[0].enabled, 'Processed track is enabled when speaking');
    if (group) assert.equal(f.published().mediaStreamTrack, f.sessions[0].stream.getAudioTracks()[0]);
    await c.toggleMicrophone();
    assert(!f.sessions[0].stream.getAudioTracks()[0].enabled, 'Mute is effective');
    await c.applyAudioSettings();
    assert(f.sessions[0].closed, 'Reconfiguration releases the old microphone');
    assert(!f.sessions[1].stream.getAudioTracks()[0].enabled, 'Reconfiguration preserves mute');
    f.fail(true); await assert.rejects(c.setMicrophoneDevice('missing')); f.fail(false);
    assert(!f.sessions[1].closed, 'Failed switch keeps the working session');
    settings.pushToTalk = true;
    await c.toggleMicrophone();
    assert(!f.sessions[1].stream.getAudioTracks()[0].enabled, 'Arming PTT does not transmit');
    await c.setPushToTalk(true); assert(f.sessions[1].stream.getAudioTracks()[0].enabled);
    await c.setPushToTalk(false); assert(!f.sessions[1].stream.getAudioTracks()[0].enabled);
    const wait = f.delay(); const switching = c.setMicrophoneDevice('new');
    await new Promise(resolve => setImmediate(resolve));
    await (group ? c.leave() : c.hangup()); wait.resolve(); await switching;
    assert(f.sessions.every(s => s.closed), 'Late capture is closed after hangup');
    assert.equal(c.getState().status, 'idle');
  }
  const f = fixture(false), wait = f.delay();
  const start = f.client.start({ id: 'chat', peer_id: 'b' }); await new Promise(r => setImmediate(r));
  f.client.hangup(); wait.resolve(); await start;
  assert(f.sessions.every(s => s.closed)); assert.equal(f.sent.length, 0, 'Cancelled call never signals an offer');
  console.log('Audio routing passed: direct/group processed tracks, mute, PTT, failed switch, late capture cleanup, cancelled call.');
})().catch(error => { console.error(error); process.exitCode = 1; });
