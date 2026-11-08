const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ui = path.resolve(__dirname, '../crates/desktop/ui');

function directFixture(config, { failShare = false, relayOnly = false, relayResponds = true, deferConfig = false } = {}) {
  const pcs = []; const timers = []; const errors = []; let releaseConfig; const configGate = new Promise(resolve => { releaseConfig = resolve; }); const probes = []; const signals = []; let micRequests = 0; let screenRequests = 0; let shareLeaves = 0;
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
    async setLocalDescription(value) {
      this.localDescription = value;
      if (this.probe) queueMicrotask(() => this.onicecandidate?.({ candidate: relayResponds ? { type: 'relay', candidate: 'candidate:1 1 udp 1 192.0.2.9 5009 typ relay' } : null }));
    }
    async setRemoteDescription(value) { this.remoteDescription = value; }
    async createAnswer() { return { type: 'answer', sdp: 'fixture' }; }
    async addIceCandidate() {}
    async getStats() { return new Map(); }
    // The pre-call relay probe is the only peer that opens a data channel.
    createDataChannel() { pcs.splice(pcs.indexOf(this), 1); probes.push(this); this.probe = true; return {}; }
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
    setTimeout: (fn, ms) => { timers.push({ fn, ms, cleared: false }); return timers.length; },
    clearTimeout: id => { if (timers[id - 1]) timers[id - 1].cleared = true; }, setInterval: () => 1, clearInterval() {},
  };
  vm.runInNewContext(fs.readFileSync(path.join(ui, 'voice-client.js'), 'utf8'), context);
  const client = window.GhostChatVoice.create({
    getSettings: () => ({ relayOnly }), getIceConfiguration: async () => { if (deferConfig) await configGate; return config; },
    getUserId: () => 'a', getShareToken: async () => ({}), sendSignal: signal => signals.push(signal),
    onChange() {}, onError: message => errors.push(message), onIncoming() {},
  });
  return { client, pcs, probes, signals, timers, errors, releaseConfig, counts: () => ({ micRequests, screenRequests, shareLeaves }) };
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
    const fixture = directFixture(config, { relayOnly: true });
    await assert.rejects(fixture.client.start({ id: 'conversation', peer_id: 'b' }), /authenticated TURN relay/);
    assert.equal(fixture.pcs.length, 0, 'Missing authenticated TURN must fail before constructing any peer connection');
    assert.equal(fixture.counts().micRequests, 0, 'Missing TURN must fail before microphone capture');
  }
  const direct = directFixture({ turn_available: true, group_voice_available: true, ice_servers: [
    { urls: 'stun:public.fixture:19302' },
    { urls: ['stun:public.fixture:19302', 'turns:relay.fixture:5349?transport=tcp'], username: 'fixture', credential: 'fixture' },
  ] }, { failShare: true, relayOnly: true });
  await direct.client.start({ id: 'conversation', peer_id: 'b' });
  const pc = direct.pcs[0];
  assert.equal(direct.probes.length, 0, 'Strict relay-only mode never probes or falls back');
  assert.equal(pc.configuration.iceTransportPolicy, 'relay', 'Strict relay-only mode keeps the relay policy');
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

  // Default mode, working relay: same relay-only guarantees as strict mode.
  const turnConfig = { turn_available: true, ice_servers: [
    { urls: 'stun:public.fixture:19302' },
    { urls: ['turn:relay.fixture:3478?transport=udp'], username: 'fixture', credential: 'fixture' },
  ] };
  const healthy = directFixture(turnConfig);
  await healthy.client.start({ id: 'conversation', peer_id: 'b' });
  assert.equal(healthy.probes.length, 1, 'The relay is probed before the call');
  assert.equal(healthy.pcs[0].configuration.iceTransportPolicy, 'relay', 'A responding relay is always used');
  assert.deepEqual(healthy.pcs[0].configuration.iceServers.map(server => server.urls).flat(), ['turn:relay.fixture:3478?transport=udp'], 'Public STUN is not contacted while the relay works');
  healthy.client.hangup();

  // Default mode, configured but unreachable relay: fall back to direct so audio still flows.
  const dead = directFixture(turnConfig, { relayResponds: false });
  await dead.client.start({ id: 'conversation', peer_id: 'b' });
  const fallback = dead.pcs[0];
  assert.equal(fallback.configuration.iceTransportPolicy, 'all', 'An unreachable relay falls back to a direct connection');
  assert(fallback.configuration.iceServers.some(server => [server.urls].flat().includes('stun:public.fixture:19302')), 'The fallback uses STUN');
  const beforeHost = dead.signals.length;
  fallback.onicecandidate({ candidate: { type: 'host', candidate: 'candidate:1 1 udp 1 192.0.2.1 5000 typ host', toJSON: () => ({ candidate: 'host' }) } });
  assert.equal(dead.signals.length, beforeHost + 1, 'Direct candidates are signaled in fallback mode');
  fallback.iceGatheringState = 'complete'; fallback.onicegatheringstatechange();
  assert.equal(dead.client.getState().status, 'calling', 'A fallback call is not ended for having no relay candidate');
  dead.client.hangup();

  // Default mode, no TURN configured at all: direct call, like before 4.0.45.
  const noTurn = directFixture({ turn_available: false, ice_servers: [{ urls: 'stun:public.fixture:19302' }] });
  await noTurn.client.start({ id: 'conversation', peer_id: 'b' });
  assert.equal(noTurn.probes.length, 0, 'Nothing to probe without TURN');
  assert.equal(noTurn.pcs[0].configuration.iceTransportPolicy, 'all');
  noTurn.client.hangup();

  // Direct call lifecycle: accept racing a caller hangup, double accept, stuck connect.
  const plain = { turn_available: false, ice_servers: [] };
  const offerSignal = { conversation_id: 'conversation', from_user_id: 'b', kind: 'offer', payload: { type: 'offer', sdp: 'fixture' } };
  const hungUp = directFixture(plain, { deferConfig: true });
  await hungUp.client.handleSignal(offerSignal);
  const pendingAccept = hungUp.client.accept();
  await hungUp.client.handleSignal({ conversation_id: 'conversation', from_user_id: 'b', kind: 'hangup' });
  hungUp.releaseConfig();
  await pendingAccept;
  assert.equal(hungUp.client.getState().status, 'idle', 'accept after caller hangup opens no call');
  assert.equal(hungUp.pcs.length, 0, 'No peer connection is created for a cancelled call');
  assert(!hungUp.signals.some(signal => signal.kind === 'answer'), 'No answer is sent for a cancelled call');

  const twice = directFixture(plain, { deferConfig: true });
  await twice.client.handleSignal(offerSignal);
  const firstAccept = twice.client.accept(); const secondAccept = twice.client.accept();
  twice.releaseConfig();
  await Promise.all([firstAccept, secondAccept]);
  assert.equal(twice.pcs.length, 1, 'Double accept creates one peer connection');
  assert.equal(twice.signals.filter(signal => signal.kind === 'answer').length, 1, 'Double accept sends one answer');
  twice.client.hangup();

  const stuck = directFixture(plain);
  await stuck.client.start({ id: 'conversation', peer_id: 'b' });
  await stuck.client.handleSignal({ conversation_id: 'conversation', from_user_id: 'b', kind: 'answer', payload: { type: 'answer', sdp: 'fixture' } });
  stuck.pcs[0].connectionState = 'connecting';
  const connectTimer = stuck.timers.filter(timer => timer.ms === 30_000 && !timer.cleared);
  assert.equal(connectTimer.length, 1, 'Applying the answer starts the connect timeout');
  connectTimer[0].fn();
  assert.equal(stuck.client.getState().status, 'idle', 'A call stuck in connecting is closed');
  assert.equal(stuck.signals.at(-1).kind, 'hangup', 'The peer is told the call ended');
  assert.match(stuck.errors.at(-1), /could not connect/);

  const closedMidSignal = directFixture(plain);
  await closedMidSignal.client.start({ id: 'conversation', peer_id: 'b' });
  closedMidSignal.pcs[0].setRemoteDescription = async () => { closedMidSignal.client.hangup(); throw new Error('peer connection closed'); };
  await closedMidSignal.client.handleSignal({ conversation_id: 'conversation', from_user_id: 'b', kind: 'answer', payload: { type: 'answer', sdp: 'fixture' } });
  assert.equal(closedMidSignal.client.getState().status, 'idle', 'A call closed during a signal await does not make handleSignal throw');

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
  console.log(`Media relay checks passed: strict relay-only admission, relay probe with direct fallback, no SFU downgrade, group/share failures, reconnect state and vendored LiveKit ${SDK.version} RTC merges.`);
}
run().catch(error => { console.error(error); process.exitCode = 1; });
