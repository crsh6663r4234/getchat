const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ui = path.resolve(__dirname, '../crates/desktop/ui');

function directFixture(config, { failShare = false } = {}) {
  const pcs = []; const signals = []; let micRequests = 0; let screenRequests = 0; let shareLeaves = 0;
  class Peer {
    connectionState = 'new'; signalingState = 'stable'; transceivers = [];
    constructor(configuration) { this.configuration = configuration; pcs.push(this); }
    addTransceiver(kind, options) {
      const sender = { track: null, getParameters: () => ({ encodings: [{}] }), setParameters: async () => {}, replaceTrack: async track => { sender.track = track; } };
      const transceiver = { sender, receiver: { track: { kind } }, direction: options.direction };
      this.transceivers.push(transceiver); return transceiver;
    }
    getTransceivers() { return this.transceivers; }
    getSenders() { return this.transceivers.map(t => t.sender); }
    async createOffer() { return { type: 'offer', sdp: 'fixture' }; }
    async setLocalDescription(value) { this.localDescription = value; }
    async getStats() { return new Map(); }
    close() { this.connectionState = 'closed'; }
  }
  const window = { GhostChatShare: { create: () => ({ start: async () => { if (failShare) throw new Error('TURN unavailable'); }, leave: async () => { shareLeaves++; } }) } };
  const context = {
    window, RTCPeerConnection: Peer, console,
    navigator: { mediaDevices: {
      getSupportedConstraints: () => ({}),
      getUserMedia: async () => { micRequests++; throw { name: 'NotFoundError' }; },
      getDisplayMedia: async () => { screenRequests++; throw new Error('Unexpected direct screen fallback'); },
    } },
    setTimeout: () => 1, clearTimeout() {}, setInterval: () => 1, clearInterval() {},
  };
  vm.runInNewContext(fs.readFileSync(path.join(ui, 'voice-client.js'), 'utf8'), context);
  const client = window.GhostChatVoice.create({
    getSettings: () => ({ relayOnly: false }), getIceConfiguration: async () => config,
    getUserId: () => 'a', getShareToken: async () => ({}), sendSignal: signal => signals.push(signal),
    onChange() {}, onError() {}, onIncoming() {},
  });
  return { client, pcs, signals, counts: () => ({ micRequests, screenRequests, shareLeaves }) };
}

function livekitFixture({ connectFails = false } = {}) {
  const rooms = [];
  class Room {
    events = new Map(); remoteParticipants = new Map();
    localParticipant = { identity: 'local', isMicrophoneEnabled: false, isScreenShareEnabled: false, setMicrophoneEnabled: async () => {} };
    constructor(options) { this.options = options; rooms.push(this); }
    on(event, handler) { this.events.set(event, handler); return this; }
    async connect(url, token, options) { this.connectOptions = options; if (connectFails) throw new Error('No TURN candidates'); }
    async startAudio() {}
    async disconnect() { this.disconnected = true; this.events.get('Disconnected')?.(); }
    static async getLocalDevices() { return []; }
  }
  const names = ['TrackSubscribed', 'TrackUnsubscribed', 'TrackPublished', 'TrackUnpublished', 'ParticipantConnected', 'ParticipantDisconnected', 'ActiveSpeakersChanged', 'LocalTrackPublished', 'LocalTrackUnpublished', 'Reconnecting', 'Reconnected', 'Disconnected', 'AudioPlaybackStatusChanged'];
  const LK = { Room, RoomEvent: Object.fromEntries(names.map(name => [name, name])), Track: { Kind: { Audio: 'audio', Video: 'video' }, Source: { ScreenShare: 'screen' } } };
  const window = { LivekitClient: LK };
  const context = { window, navigator: { mediaDevices: { getSupportedConstraints: () => ({}) } }, document: { querySelector: () => ({ append() {} }) }, console, setTimeout: () => 1, clearTimeout() {}, MediaStream: class {} };
  return { rooms, window, context };
}

async function run() {
  for (const config of [
    { ice_servers: [], turn_available: false },
    { ice_servers: [{ urls: 'stun:public.fixture:19302' }], turn_available: true },
    { ice_servers: [{ urls: 'turn:fixture:3478', username: '', credential: 'fixture' }], turn_available: true },
    { ice_servers: [{ urls: 'turn:fixture:3478', username: 'fixture' }], turn_available: true },
  ]) {
    const fixture = directFixture(config);
    await assert.rejects(fixture.client.start({ id: 'conversation', peer_id: 'b' }), /authenticated TURN relay/);
    assert.equal(fixture.pcs.length, 0, 'Missing authenticated TURN must fail before constructing any peer connection');
    assert.equal(fixture.counts().micRequests, 0, 'Missing TURN must fail before microphone capture');
  }
  const direct = directFixture({ turn_available: true, group_voice_available: true, ice_servers: [
    { urls: 'stun:public.fixture:19302' },
    { urls: ['stun:public.fixture:19302', 'turns:relay.fixture:5349?transport=tcp'], username: 'fixture', credential: 'fixture' },
  ] }, { failShare: true });
  await direct.client.start({ id: 'conversation', peer_id: 'b' });
  const pc = direct.pcs[0];
  assert.equal(pc.configuration.iceTransportPolicy, 'relay', 'A saved relayOnly=false preference cannot weaken required policy');
  assert.equal(pc.configuration.iceServers.length, 1);
  assert.deepEqual([...pc.configuration.iceServers[0].urls], ['turns:relay.fixture:5349?transport=tcp'], 'Public STUN must not be contacted');
  const beforeCandidates = direct.signals.length;
  pc.onicecandidate({ candidate: { type: 'host', candidate: 'candidate:1 1 udp 1 192.0.2.1 5000 typ host', toJSON: () => ({ candidate: 'host' }) } });
  assert.equal(direct.signals.length, beforeCandidates, 'Host candidates must not be signaled');
  pc.onicecandidate({ candidate: { type: 'relay', candidate: 'candidate:1 1 udp 1 192.0.2.2 5001 typ relay', toJSON: () => ({ candidate: 'relay' }) } });
  assert.equal(direct.signals.at(-1).kind, 'ice');
  pc.connectionState = 'connected'; pc.onconnectionstatechange();
  await assert.rejects(direct.client.toggleScreenSharing(), /no direct fallback/);
  assert.equal(direct.counts().screenRequests, 0, 'An SFU failure cannot silently invoke direct screen capture');
  assert.equal(direct.counts().shareLeaves, 1);
  direct.client.hangup();

  for (const connectFails of [false, true]) {
    const fixture = livekitFixture({ connectFails });
    vm.runInNewContext(fs.readFileSync(path.join(ui, 'group-voice-client.js'), 'utf8'), fixture.context);
    const group = fixture.window.GhostChatGroupVoice.create({
      getToken: async () => ({ url: 'wss://fixture', token: 'fixture' }), getMembers: async () => [],
      getSettings: () => ({}), getUserId: () => 'a', getDisplayName: () => 'Fixture', getAvatar: () => null,
      isPeerMuted: () => false, onChange() {}, onAudioBlocked() {},
    });
    if (connectFails) {
      await assert.rejects(group.join({ id: 'group' }), /required relay connection/);
      assert.equal(group.getState().status, 'idle');
      assert.equal(fixture.rooms[0].disconnected, true);
    } else {
      await group.join({ id: 'group' });
      fixture.rooms[0].events.get('Reconnecting')();
      fixture.rooms[0].events.get('Reconnected')();
      assert.equal(group.getState().status, 'connected');
      await group.leave();
    }
    assert.equal(fixture.rooms.length, 1, 'There must be no weaker replacement room after failure');
    assert.equal(fixture.rooms[0].connectOptions.rtcConfig.iceTransportPolicy, 'relay');
    vm.runInNewContext(fs.readFileSync(path.join(ui, 'ghost-share-client.js'), 'utf8'), fixture.context);
    const share = fixture.window.GhostChatShare.create({ getToken: async () => ({ url: 'wss://fixture', token: 'fixture' }) });
    if (connectFails) {
      await assert.rejects(share.watch('direct'), /required relay connection/);
      assert.equal(share.getState().connected, false);
      assert.equal(fixture.rooms[1].disconnected, true);
    } else { await share.watch('direct'); await share.leave(); }
    assert.equal(fixture.rooms[1].connectOptions.rtcConfig.iceTransportPolicy, 'relay');
  }

  // Exercise the actual checked-in SDK merge used for initial join and resumed
  // signaling responses, not only a mock's acceptance of an option object.
  const SDK = require(path.join(ui, 'vendor/livekit-client.umd.js'));
  const room = new SDK.Room();
  room.engine.rtcConfig = { iceTransportPolicy: 'relay' };
  for (const credential of ['initial-fixture', 'refreshed-fixture']) {
    const config = room.engine.makeRTCConfiguration({ iceServers: [{ urls: ['turn:fixture:3478'], username: 'fixture', credential }] });
    assert.equal(config.iceTransportPolicy, 'relay');
    assert.equal(config.iceServers[0].credential, credential, 'Reconnect signaling may refresh credentials without weakening relay policy');
  }
  await room.disconnect();
  console.log(`Media relay checks passed: direct fail-closed admission, TURN-only configuration, no SFU downgrade, group/share failures, reconnect state and vendored LiveKit ${SDK.version} RTC merges.`);
}
run().catch(error => { console.error(error); process.exitCode = 1; });
