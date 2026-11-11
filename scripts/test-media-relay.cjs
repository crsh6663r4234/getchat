const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ui = path.resolve(__dirname, '../crates/desktop/ui');

function directFixture(config, { failShare = false, relayOnly = true, deferConfig = false, shareFactory = null, invisible = false, screen = null, serverMedia = null, settings = {} } = {}) {
  const intervals = [];
  const pcs = []; const timers = []; const errors = []; let releaseConfig; let settingsFailures = 0; const configGate = new Promise(resolve => { releaseConfig = resolve; }); const signals = []; let micRequests = 0; let micSource = null; let screenRequests = 0; let shareLeaves = 0;
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
    }
    async setRemoteDescription(value) { this.remoteDescription = value; }
    async createAnswer() { return { type: 'answer', sdp: 'fixture' }; }
    async addIceCandidate() {}
    async getStats() { return new Map(); }
    close() { this.connectionState = 'closed'; }
  }
  const window = { GhostChatShare: { create: shareFactory || (() => ({ start: async () => { if (failShare) throw new Error('TURN unavailable'); }, leave: async () => { shareLeaves++; } })) } };
  const context = {
    window, RTCPeerConnection: Peer, console,
    navigator: { mediaDevices: {
      getSupportedConstraints: () => ({}),
      getUserMedia: async () => { micRequests++; if (micSource) return micSource(); throw { name: 'NotFoundError' }; },
      getDisplayMedia: async () => { screenRequests++; if (screen) return screen(); throw new Error('Unexpected direct screen fallback'); },
    } },
    setTimeout: (fn, ms) => { timers.push({ fn, ms, cleared: false }); return timers.length; },
    clearTimeout: id => { if (timers[id - 1]) timers[id - 1].cleared = true; },
    setInterval: fn => { intervals.push({ fn, cleared: false }); return intervals.length; }, clearInterval: id => { if (intervals[id - 1]) intervals[id - 1].cleared = true; },
  };
  window.GhostChatAudio = { open: async () => {
    const stream = await context.navigator.mediaDevices.getUserMedia();
    return { stream, rawStream: stream, status: { state: 'running' },
      setMuted(value) { for (const track of stream.getAudioTracks()) track.enabled = !value; },
      async close() { for (const track of stream.getTracks()) track.stop(); } };
  } };
  vm.runInNewContext(fs.readFileSync(path.join(ui, 'voice-client.js'), 'utf8'), context);
  const client = window.GhostChatVoice.create({
    getSettings: () => { if (settingsFailures > 0) { settingsFailures--; throw new Error('settings unavailable'); } return { relayOnly, ...settings }; }, getIceConfiguration: async () => { if (deferConfig) await configGate; return config; },
    getUserId: () => 'a', getShareToken: async () => ({}), sendSignal: signal => signals.push(signal),
    onChange() {}, onError: message => errors.push(message), onIncoming() {}, isInvisible: () => invisible,
    ...(serverMedia ? { createServerMedia: serverMedia } : {}),
  });
  return { client, pcs, signals, timers, intervals, errors, releaseConfig, failSettingsOnce: () => { settingsFailures = 1; }, counts: () => ({ micRequests, screenRequests, shareLeaves }), setMic: fn => { micSource = fn; } };
}

// Fake server media relay transport (server-media-client.js).
function fakeServerMedia({ connect = async () => {} } = {}) {
  const instances = [];
  const factory = options => {
    const media = {
      options, inputs: [], sending: [], closed: false,
      connect: () => connect(media), setInputStream(stream) { media.inputs.push(stream); },
      screens: [], setScreenTrack(track, options) { media.screens.push([track, options]); },
      setSending(value) { media.sending.push(value); }, close() { media.closed = true; },
      peers: () => new Set(), isConnected: () => !media.closed,
    };
    instances.push(media);
    return media;
  };
  return { factory, instances };
}

function fakeScreen() {
  const video = { kind: 'video', applyConstraints: async () => {}, stop() { video.stopped = true; } };
  return { video, getTracks: () => [video], getVideoTracks: () => [video], getAudioTracks: () => [] };
}

function fakeMicrophone() {
  const track = { kind: 'audio', enabled: true, getSettings: () => ({}), stop() { track.stopped = true; } };
  return { track, getAudioTracks: () => [track], getTracks: () => [track] };
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
    async switchActiveDevice(kind, deviceId) { (this.switches ||= []).push([kind, deviceId]); return true; }
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
  assert.equal(pc.configuration.iceTransportPolicy, 'relay', 'Calls always use relay-only transport');
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

  // Privacy is mandatory: an authenticated TURN relay is used for every direct call.
  const turnConfig = { turn_available: true, ice_servers: [
    { urls: 'stun:public.fixture:19302' },
    { urls: ['turn:relay.fixture:3478?transport=udp'], username: 'fixture', credential: 'fixture' },
  ] };
  const healthy = directFixture(turnConfig);
  await healthy.client.start({ id: 'conversation', peer_id: 'b' });
  assert.equal(healthy.pcs[0].configuration.iceTransportPolicy, 'relay', 'Calls are always relay-only');
  assert.deepEqual(healthy.pcs[0].configuration.iceServers.map(server => server.urls).flat(), ['turn:relay.fixture:3478?transport=udp'], 'Public STUN is never contacted');
  healthy.client.hangup();

  // No TURN configuration must fail before a peer connection can expose an address.
  const noTurn = directFixture({ turn_available: false, ice_servers: [{ urls: 'stun:public.fixture:19302' }] });
  await assert.rejects(noTurn.client.start({ id: 'conversation', peer_id: 'b' }), /authenticated TURN relay/);
  assert.equal(noTurn.pcs.length, 0, 'No direct peer connection is constructed without TURN');

  // Calls through the server (server media relay). No TURN configured: the
  // relay carries the audio from the start, and no ICE server is contacted.
  const relayOnlyConfig = { turn_available: false, media_relay: true, ice_servers: [{ urls: 'stun:public.fixture:19302' }] };
  const relayOffer = { conversation_id: 'conversation', from_user_id: 'b', kind: 'offer', payload: { type: 'offer', sdp: 'fixture' } };
  const answerSignal = { conversation_id: 'conversation', from_user_id: 'b', kind: 'answer', payload: { type: 'answer', sdp: 'fixture' } };
  {
    const relay = fakeServerMedia();
    const fixture = directFixture(relayOnlyConfig, { serverMedia: relay.factory });
    await fixture.client.start({ id: 'conversation', peer_id: 'b' });
    const pc = fixture.pcs[0];
    assert.deepEqual([...pc.configuration.iceServers], [], 'without TURN no ICE server (and no public STUN) is used');
    assert.equal(pc.configuration.iceTransportPolicy, 'relay', 'the signaling-only peer connection can never expose an address');
    assert.equal(relay.instances.length, 0, 'the relay room is joined only once the call is answered');
    pc.iceGatheringState = 'complete'; pc.onicegatheringstatechange();
    assert.equal(fixture.client.getState().status, 'calling', 'no TURN candidates is expected on the relay path');
    await fixture.client.handleSignal(answerSignal);
    assert.equal(relay.instances.length, 1, 'the answered call joins the relay room');
    assert.equal(relay.instances[0].options.conversationId, 'conversation');
    assert.equal(fixture.client.getState().status, 'connecting');
    const stream = { id: 'relayed-b' };
    relay.instances[0].options.onPeerStream('b', stream);
    assert.equal(fixture.client.getState().status, 'connected', 'the peer in the relay room means the call is connected');
    assert.equal(fixture.client.getState().remoteAudioStream, stream, 'relayed audio plays through the usual call audio element');
    assert.equal(fixture.client.getState().mediaPath, 'server');
    pc.connectionState = 'failed'; pc.onconnectionstatechange();
    assert.equal(fixture.client.getState().status, 'connected', 'the unused peer connection cannot end a relayed call');
    const connectTimer = fixture.timers.find(timer => timer.ms === 30_000);
    assert(connectTimer?.cleared, 'the connect timeout is cleared once the relay connects');
    fixture.client.hangup();
    assert(relay.instances[0].closed, 'hangup closes the relay');
    assert.equal(fixture.signals.at(-1).kind, 'hangup');
  }
  // The callee joins at accept; mute, push-to-talk and a microphone change reach the relay.
  {
    const relay = fakeServerMedia();
    const fixture = directFixture(relayOnlyConfig, { serverMedia: relay.factory });
    const first = fakeMicrophone(); const second = fakeMicrophone();
    let mic = first; fixture.setMic(() => mic);
    await fixture.client.handleSignal(relayOffer);
    await fixture.client.accept();
    await new Promise(resolve => setImmediate(resolve));
    const media = relay.instances[0];
    assert(media, 'accepting a relayed call joins the relay room');
    assert.equal(media.inputs.at(-1).getAudioTracks()[0], first.track, 'the microphone feeds the relay');
    assert.equal(media.sending.at(-1), true);
    await fixture.client.toggleMicrophone();
    assert.equal(media.sending.at(-1), false, 'mute stops relayed audio');
    await fixture.client.toggleMicrophone();
    assert.equal(media.sending.at(-1), true);
    mic = second;
    await fixture.client.setMicrophoneDevice('second');
    assert.equal(media.inputs.at(-1).getAudioTracks()[0], second.track, 'a new microphone feeds the relay');
    fixture.client.hangup();
  }
  {
    const relay = fakeServerMedia();
    const fixture = directFixture(relayOnlyConfig, { serverMedia: relay.factory, settings: { pushToTalk: true } });
    fixture.setMic(() => fakeMicrophone());
    await fixture.client.handleSignal(relayOffer);
    await fixture.client.accept();
    await new Promise(resolve => setImmediate(resolve));
    const media = relay.instances[0];
    assert.equal(media.sending.at(-1), false, 'push-to-talk sends nothing until the key is held');
    fixture.client.setPushToTalk(true);
    assert.equal(media.sending.at(-1), true);
    fixture.client.setPushToTalk(false);
    assert.equal(media.sending.at(-1), false);
    fixture.client.hangup();
  }
  // A relay that cannot be joined ends the call with its reason.
  {
    const relay = fakeServerMedia({ connect: async () => { throw new Error('You are not allowed to join this call through the server relay.'); } });
    const fixture = directFixture(relayOnlyConfig, { serverMedia: relay.factory });
    await fixture.client.start({ id: 'conversation', peer_id: 'b' });
    await fixture.client.handleSignal(answerSignal);
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(fixture.client.getState().status, 'idle');
    assert.match(fixture.errors.at(-1), /not allowed to join/);
  }
  // Screen sharing on a relayed direct call goes through the relay too.
  {
    const relay = fakeServerMedia();
    const shared = fakeScreen();
    const fixture = directFixture(relayOnlyConfig, { serverMedia: relay.factory, screen: () => shared });
    await fixture.client.start({ id: 'conversation', peer_id: 'b' });
    await fixture.client.handleSignal(answerSignal);
    const media = relay.instances[0];
    media.options.onPeerStream('b', { id: 'relayed-b' });
    await fixture.client.toggleScreenSharing();
    assert.equal(media.screens.at(-1)[0], shared.video, 'the screen track is shared through the relay');
    assert(media.screens.at(-1)[1].frameRate <= 8, 'relayed screen sharing uses a low frame rate');
    assert.equal(fixture.client.getState().screenSharing, true);
    assert.equal(fixture.client.getState().screenTransport, 'server');
    assert.equal(fixture.counts().shareLeaves, 0, 'no SFU room is used on the relay path');
    await fixture.client.toggleScreenSharing();
    assert.equal(media.screens.at(-1)[0], null, 'stopping ends the relayed share');
    assert(shared.video.stopped);
    assert.equal(fixture.signals.at(-1).kind, 'screen_stopped');
    assert.equal(fixture.client.getState().screenSharing, false);
    const remote = { id: 'remote-screen', getVideoTracks: () => [], removeTrack() {} };
    media.options.onPeerScreen('b', remote);
    assert.equal(fixture.client.getState().remoteVideoStream, remote, 'the peer\'s relayed share is shown');
    media.options.onPeerScreen('b', null);
    assert.equal(fixture.client.getState().remoteVideoStream, null);
    fixture.client.hangup();
  }
  // TURN configured but never connects: switch to the relay after about 8 s.
  const turnAndRelay = { ...turnConfig, media_relay: true };
  {
    const relay = fakeServerMedia();
    const fixture = directFixture(turnAndRelay, { serverMedia: relay.factory });
    await fixture.client.start({ id: 'conversation', peer_id: 'b' });
    assert.equal(fixture.pcs[0].configuration.iceServers.length, 1, 'TURN is tried first');
    await fixture.client.handleSignal(answerSignal);
    assert.equal(relay.instances.length, 0, 'the relay waits while TURN may still connect');
    const fallback = fixture.timers.find(timer => timer.ms === 8_000 && !timer.cleared);
    assert(fallback, 'a relay fallback timer runs while TURN connects');
    fallback.fn();
    assert.equal(relay.instances.length, 1, 'an unconnected TURN call switches to the server relay');
    assert.equal(fixture.client.getState().status, 'connecting', 'the call UI keeps showing connecting');
    relay.instances[0].options.onPeerStream('b', { id: 'relayed' });
    assert.equal(fixture.client.getState().status, 'connected');
    fixture.client.hangup();
  }
  // TURN connects in time: the relay is never used.
  {
    const relay = fakeServerMedia();
    const fixture = directFixture(turnAndRelay, { serverMedia: relay.factory });
    await fixture.client.start({ id: 'conversation', peer_id: 'b' });
    await fixture.client.handleSignal(answerSignal);
    const pc = fixture.pcs[0];
    pc.connectionState = 'connected'; pc.onconnectionstatechange();
    const fallback = fixture.timers.find(timer => timer.ms === 8_000);
    if (fallback && !fallback.cleared) fallback.fn();
    assert.equal(relay.instances.length, 0, 'a connected TURN call never joins the relay');
    assert.equal(fixture.client.getState().mediaPath, 'turn');
    fixture.client.hangup();
  }
  // TURN fails outright (no relay candidates, or ICE failure): switch at once instead of ending the call.
  for (const failure of ['gathering', 'failed']) {
    const relay = fakeServerMedia();
    const fixture = directFixture(turnAndRelay, { serverMedia: relay.factory });
    await fixture.client.handleSignal(relayOffer);
    await fixture.client.accept();
    const pc = fixture.pcs[0];
    if (failure === 'gathering') { pc.iceGatheringState = 'complete'; pc.onicegatheringstatechange(); }
    else { pc.connectionState = 'failed'; pc.onconnectionstatechange(); }
    assert.equal(relay.instances.length, 1, `a TURN ${failure} switches to the relay`);
    assert.equal(fixture.client.getState().status, 'connecting');
    assert.deepEqual(fixture.errors, []);
    fixture.client.hangup();
  }

  // Direct call lifecycle: accept racing a caller hangup, double accept, stuck connect.
  // Calls are relay-only since 4.0.50, so the lifecycle fixtures need an authenticated TURN relay.
  const plain = { turn_available: true, ice_servers: [{ urls: ['turn:relay.fixture:3478?transport=udp'], username: 'fixture', credential: 'fixture' }] };
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

  // A busy callee answers a second call with an automatic hangup. In
  // invisible mode that reply would reveal it is online, so the second
  // caller must see exactly what an offline callee gives: nothing.
  for (const invisible of [false, true]) {
    for (const busy of ['incoming', 'call']) {
      const fixture = directFixture(plain, { invisible });
      if (busy === 'incoming') await fixture.client.handleSignal(offerSignal);
      else await fixture.client.start({ id: 'conversation', peer_id: 'b' });
      const before = fixture.signals.length;
      await fixture.client.handleSignal({ conversation_id: 'other', from_user_id: 'c', kind: 'offer', payload: { type: 'offer', sdp: 'fixture' } });
      const replies = fixture.signals.slice(before).filter(signal => signal.peerId === 'c');
      if (invisible) assert.deepEqual(replies, [], `invisible and busy (${busy}): the second caller gets no reply`);
      else assert.deepEqual(replies.map(signal => signal.kind), ['hangup'], `busy (${busy}): the second caller is told`);
      fixture.client.hangup();
    }
  }
  // Declining tells the caller we are here; an offline callee says nothing
  // and the caller rings to its own timeout. Invisible must look the same.
  for (const invisible of [false, true]) {
    const fixture = directFixture(plain, { invisible });
    await fixture.client.handleSignal(offerSignal);
    const before = fixture.signals.length;
    fixture.client.decline();
    assert.equal(fixture.client.getState().status, 'idle', 'a declined call stops ringing here');
    const replies = fixture.signals.slice(before).map(signal => signal.kind);
    if (invisible) assert.deepEqual(replies, [], 'an invisible decline sends the caller nothing');
    else assert.deepEqual(replies, ['hangup'], 'a visible decline tells the caller once');
  }
  // Invisible mode still receives the call and can answer it.
  {
    const fixture = directFixture(plain, { invisible: true });
    await fixture.client.handleSignal(offerSignal);
    assert.equal(fixture.client.getState().status, 'incoming', 'an invisible callee still rings');
    await fixture.client.accept();
    assert.equal(fixture.signals.filter(signal => signal.kind === 'answer').length, 1, 'an invisible callee can answer');
    fixture.client.hangup();
  }

  // The same caller re-sends its offer while accept() probes the relay: the
  // pending accept continues with the newest offer instead of doing nothing.
  {
    const fixture = directFixture(plain, { deferConfig: true });
    await fixture.client.handleSignal(offerSignal);
    const pending = fixture.client.accept();
    await fixture.client.handleSignal({ ...offerSignal, payload: { type: 'offer', sdp: 'renewed' } });
    fixture.releaseConfig();
    await pending;
    assert.equal(fixture.pcs.length, 1, 'the accept opens the call after a duplicate offer');
    assert.equal(fixture.pcs[0].remoteDescription.sdp, 'renewed', 'the newest offer is answered');
    assert.equal(fixture.signals.filter(signal => signal.kind === 'answer').length, 1);
    fixture.client.hangup();
  }
  // An offer from someone else meanwhile still cancels the pending accept.
  {
    const fixture = directFixture(plain, { deferConfig: true });
    await fixture.client.handleSignal(offerSignal);
    const pending = fixture.client.accept();
    await fixture.client.handleSignal({ conversation_id: 'conversation', from_user_id: 'b', kind: 'hangup' });
    await fixture.client.handleSignal({ ...offerSignal, conversation_id: 'other', from_user_id: 'c' });
    fixture.releaseConfig();
    await pending;
    assert.equal(fixture.pcs.length, 0, 'an accept never answers a different caller');
  }

  // Cancelling the picker of a direct share refresh keeps the adaptive
  // quality monitor running for the share that continues.
  {
    const screenStream = () => {
      const track = { kind: 'video', stop() {}, applyConstraints: async () => {}, getSettings: () => ({}) };
      return { getTracks: () => [track], getVideoTracks: () => [track], getAudioTracks: () => [] };
    };
    let cancel = false;
    const fixture = directFixture(plain, { screen: async () => { if (cancel) { const error = new Error('cancelled'); error.name = 'AbortError'; throw error; } return screenStream(); } });
    await fixture.client.start({ id: 'conversation', peer_id: 'b' });
    fixture.pcs[0].connectionState = 'connected'; fixture.pcs[0].onconnectionstatechange();
    await fixture.client.toggleScreenSharing();
    const running = () => fixture.intervals.filter(item => !item.cleared).length;
    const before = running();
    assert.ok(before >= 1, 'sharing starts the share monitor');
    cancel = true;
    await fixture.client.refreshScreenSharing();
    assert.equal(running(), before, 'a cancelled refresh leaves the share monitor running');
    fixture.client.hangup();
  }

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

  const recovering = directFixture(plain);
  await recovering.client.start({ id: 'conversation', peer_id: 'b' });
  const recoveringPeer = recovering.pcs[0];
  recoveringPeer.setRemoteDescription = async () => {
    recoveringPeer.connectionState = 'connected'; recoveringPeer.onconnectionstatechange();
    recoveringPeer.connectionState = 'disconnected'; recoveringPeer.onconnectionstatechange();
  };
  await recovering.client.handleSignal({ conversation_id: 'conversation', from_user_id: 'b', kind: 'answer', payload: { type: 'answer', sdp: 'fixture' } });
  recovering.timers.filter(timer => timer.ms === 30_000 && !timer.cleared).forEach(timer => timer.fn());
  assert.equal(recovering.client.getState().status, 'reconnecting', 'A call that already connected is not hung up by the connect timeout');
  assert(!recovering.signals.some(signal => signal.kind === 'hangup'));
  recovering.client.hangup();

  const flaky = directFixture(plain);
  await flaky.client.handleSignal(offerSignal);
  flaky.failSettingsOnce();
  await assert.rejects(flaky.client.accept(), /settings unavailable/);
  await flaky.client.accept();
  assert.equal(flaky.pcs.length, 1, 'accept works again after a settings failure');
  flaky.client.hangup();

  const closedMidSignal = directFixture(plain);
  await closedMidSignal.client.start({ id: 'conversation', peer_id: 'b' });
  closedMidSignal.pcs[0].setRemoteDescription = async () => { closedMidSignal.client.hangup(); throw new Error('peer connection closed'); };
  await closedMidSignal.client.handleSignal({ conversation_id: 'conversation', from_user_id: 'b', kind: 'answer', payload: { type: 'answer', sdp: 'fixture' } });
  assert.equal(closedMidSignal.client.getState().status, 'idle', 'A call closed during a signal await does not make handleSignal throw');

  // Stopping our own GhostShare must not drop the peer's share we are watching.
  {
    let shareApi; let leaves = 0; let sharing = false; let remote = null;
    const remoteStream = { getVideoTracks: () => [] };
    const fixture = directFixture({ ...plain, group_voice_available: true }, { shareFactory: options => {
      shareApi = options;
      return {
        watch: async () => { remote = remoteStream; shareApi.onRemoteStream(remoteStream); },
        start: async () => { sharing = true; shareApi.onLocalStream({ getTracks: () => [] }); },
        stop: async () => { sharing = false; shareApi.onLocalStream(null); },
        leave: async () => { leaves++; remote = null; shareApi.onLocalStream(null); shareApi.onRemoteStream(null); },
        getState: () => ({ connected: true, sharing, localStream: null, remoteStream: remote }),
      };
    } });
    await fixture.client.start({ id: 'conversation', peer_id: 'b' });
    fixture.pcs[0].connectionState = 'connected'; fixture.pcs[0].onconnectionstatechange();
    await fixture.client.handleSignal({ conversation_id: 'conversation', from_user_id: 'b', kind: 'ghostshare_started' });
    assert.equal(fixture.client.getState().remoteVideoStream, remoteStream);
    await fixture.client.toggleScreenSharing();
    assert.equal(fixture.client.getState().screenSharing, true);
    const stoppedBefore = fixture.signals.filter(signal => signal.kind === 'screen_stopped').length;
    await fixture.client.toggleScreenSharing();
    assert.equal(fixture.signals.filter(signal => signal.kind === 'screen_stopped').length - stoppedBefore, 1, 'stopping our share tells the peer once');
    assert.equal(fixture.client.getState().screenSharing, false);
    assert.equal(fixture.client.getState().remoteVideoStream, remoteStream, "stopping our share keeps showing the peer's share");
    assert.equal(leaves, 0, 'the SFU room stays connected while the peer is still sharing');
    fixture.client.hangup();
    assert.equal(leaves, 1, 'hangup still leaves the SFU room');
  }

  // A peer that announced ghostshare_started but whose track has not been
  // subscribed yet must keep the SFU room alive when we stop our own share.
  {
    let shareApi; let leaves = 0; let sharing = false;
    const fixture = directFixture({ ...plain, group_voice_available: true }, { shareFactory: options => {
      shareApi = options;
      return {
        watch: async () => {},
        start: async () => { sharing = true; shareApi.onLocalStream({ getTracks: () => [] }); },
        stop: async () => { sharing = false; shareApi.onLocalStream(null); },
        leave: async () => { leaves++; shareApi.onLocalStream(null); shareApi.onRemoteStream(null); },
        getState: () => ({ connected: true, sharing, localStream: null, remoteStream: null }),
      };
    } });
    await fixture.client.start({ id: 'conversation', peer_id: 'b' });
    fixture.pcs[0].connectionState = 'connected'; fixture.pcs[0].onconnectionstatechange();
    await fixture.client.toggleScreenSharing();
    await fixture.client.handleSignal({ conversation_id: 'conversation', from_user_id: 'b', kind: 'ghostshare_started' });
    await fixture.client.toggleScreenSharing();
    assert.equal(fixture.client.getState().screenSharing, false);
    assert.equal(leaves, 0, 'a pending peer share keeps the SFU room connected');
    await fixture.client.handleSignal({ conversation_id: 'conversation', from_user_id: 'b', kind: 'screen_stopped' });
    await fixture.client.toggleScreenSharing();
    await fixture.client.toggleScreenSharing();
    assert.equal(leaves, 1, 'once the peer stopped, stopping our share leaves the room');
    fixture.client.hangup();
  }

  // A microphone opened for a call that ended while the capture was pending
  // (device switch, unmute, or a hangup during replaceTrack) must be released.
  const micStream = () => {
    const track = { kind: 'audio', enabled: true, stopped: false, stop() { this.stopped = true; }, getSettings: () => ({}) };
    return { track, getAudioTracks: () => [track], getTracks: () => [track] };
  };
  for (const action of ['setMicrophoneDevice', 'toggleMicrophone', 'replaceTrack']) {
    const fixture = directFixture(plain);
    await fixture.client.start({ id: 'conversation', peer_id: 'b' });
    const opened = micStream();
    let pending;
    if (action === 'replaceTrack') {
      fixture.setMic(async () => opened);
      fixture.pcs[0].transceivers[0].sender.replaceTrack = async () => { fixture.client.hangup(); };
      pending = fixture.client.setMicrophoneDevice('next-mic');
    } else {
      let release; fixture.setMic(() => new Promise(resolve => { release = () => resolve(opened); }));
      pending = fixture.client[action](action === 'setMicrophoneDevice' ? 'next-mic' : undefined);
      await new Promise(resolve => setImmediate(resolve));
      fixture.client.hangup();
      release();
    }
    await pending.catch(() => {});
    assert.equal(opened.track.stopped, true, `${action}: a microphone captured for an ended call is stopped`);
    assert.equal(fixture.client.getState().status, 'idle');
  }

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

  // Leaving a group while its join is still fetching the token or connecting
  // must not leave a hidden room connected (listening) afterwards.
  for (const stage of ['token', 'connect']) {
    const fixture = livekitFixture();
    vm.runInNewContext(fs.readFileSync(path.join(ui, 'group-voice-client.js'), 'utf8'), fixture.context);
    let release; const gate = new Promise(resolve => { release = resolve; });
    if (stage === 'connect') fixture.window.LivekitClient.Room.prototype.connect = async function (url, token, options) { this.connectOptions = options; await gate; };
    const group = fixture.window.GhostChatGroupVoice.create({
      getToken: async () => { if (stage === 'token') await gate; return { url: 'wss://fixture', token: 'fixture' }; }, getMembers: async () => [],
      getSettings: () => ({}), getUserId: () => 'a', getDisplayName: () => 'Fixture', getAvatar: () => null,
      isPeerMuted: () => false, onChange() {}, onAudioBlocked() {},
    });
    const pendingJoin = group.join({ id: 'group' });
    await new Promise(resolve => setImmediate(resolve));
    await group.leave();
    release();
    const outcome = await pendingJoin.catch(error => ({ error }));
    assert.equal(outcome?.cancelled, true, `a join superseded during ${stage} reports that it was cancelled`);
    assert.equal(group.getState().status, 'idle', `leave during ${stage} keeps the group idle`);
    assert(fixture.rooms.every(item => item.disconnected), `leave during ${stage} leaves no connected room`);
  }

  // Group calls through the server relay: LiveKit missing, LiveKit never
  // connecting, and LiveKit connecting (relay unused).
  function relayGroupFixture({ livekit = 'configured', connect = null } = {}) {
    const fixture = livekitFixture();
    const timers = []; const elements = []; const captures = [];
    Object.assign(fixture.context, {
      setTimeout: (fn, ms) => { timers.push({ fn, ms, cleared: false }); return timers.length; },
      clearTimeout: id => { if (timers[id - 1]) timers[id - 1].cleared = true; },
      document: {
        querySelector: () => ({ append(element) { element.mounted = true; } }),
        createElement: tag => { const element = { tag, dataset: {}, sinks: [], children: [], append(...items) { element.children.push(...items); }, async setSinkId(id) { element.sinks.push(id); }, play: async () => {}, pause() {}, load() {}, removeAttribute() {}, remove() { element.removed = true; } }; elements.push(element); return element; },
      },
    });
    // The relay sends the audio engine's processed microphone.
    fixture.window.GhostChatAudio = { open: async engineSettings => {
      const mic = fakeMicrophone();
      const session = { stream: mic, rawStream: mic, status: { state: 'running' }, muted: [], setMuted(value) { session.muted.push(value); }, async close() { session.closed = true; mic.track.stop(); } };
      captures.push({ settings: engineSettings, mic, session });
      return session;
    } };
    if (connect) fixture.window.LivekitClient.Room.prototype.connect = connect;
    vm.runInNewContext(fs.readFileSync(path.join(ui, 'group-voice-client.js'), 'utf8'), fixture.context);
    const relay = fakeServerMedia();
    const muted = new Set();
    const settings = { volume: 80, speakerId: 'speaker-1' };
    const group = fixture.window.GhostChatGroupVoice.create({
      getToken: async () => {
        if (livekit === 'missing') throw Object.assign(new Error('Group voice is not configured on this server.'), { status: 503 });
        return { url: 'wss://livekit.fixture', token: 'fixture' };
      },
      getMembers: async () => [{ id: 'b', display_name: 'Bob' }, { id: 'c', display_name: 'Carol' }],
      getSettings: () => settings, getUserId: () => 'a', getDisplayName: () => 'Alice', getAvatar: () => null,
      isPeerMuted: id => muted.has(id), onChange() {}, onAudioBlocked() {}, translate: text => text,
      createServerMedia: relay.factory,
    });
    return { ...fixture, group, relay, timers, elements, captures, settings };
  }
  {
    const fixture = relayGroupFixture({ livekit: 'missing' });
    await fixture.group.join({ id: 'group' });
    assert.equal(fixture.rooms.length, 0, 'without LiveKit no SFU room is created');
    const media = fixture.relay.instances[0];
    assert(media, 'without LiveKit the group uses the server relay from the start');
    assert.equal(media.options.conversationId, 'group');
    let state = fixture.group.getState();
    assert.equal(state.status, 'connected');
    assert.equal(state.mediaPath, 'server');
    assert.equal(state.micMuted, true, 'joining a group never publishes the microphone automatically');
    assert.equal(media.sending.length, 0);
    media.options.onPeerStream('b', { id: 'stream-b' });
    const audio = fixture.elements.find(element => element.tag === 'audio');
    assert.equal(audio.srcObject.id, 'stream-b', 'each relayed peer plays through its own audio element');
    assert.equal(audio.volume, 0.8, 'the call volume applies to relayed peers');
    assert.deepEqual(audio.sinks, ['speaker-1'], 'the selected speaker applies to relayed peers');
    assert.equal(audio.muted, false);
    state = fixture.group.getState();
    assert.equal(JSON.stringify(state.participants.map(row => [row.id, row.name, row.local])), JSON.stringify([['a', 'Alice', true], ['b', 'Bob', false]]));
    fixture.group.setPeerMuted('b', true);
    assert.equal(audio.muted, true, 'muting a peer silences only that peer');
    assert.equal(fixture.group.getState().participants[1].mutedForYou, true);
    await fixture.group.setSpeakerDevice('speaker-2');
    assert.equal(audio.sinks.at(-1), 'speaker-2');
    // Microphone: on, mute, device change.
    await fixture.group.toggleMicrophone();
    assert.equal(fixture.captures.length, 1);
    assert.equal(media.inputs.at(-1), fixture.captures[0].mic, 'the engine output feeds the relay');
    assert.equal(fixture.captures[0].session.muted.at(-1), false);
    assert.equal(fixture.group.getState().audioProcessing.state, 'running', 'the engine status is shown on the relay path');
    assert.equal(media.sending.at(-1), true);
    assert.equal(fixture.group.getState().micMuted, false);
    await fixture.group.setMicrophoneDevice('mic-2');
    assert.equal(fixture.captures.length, 2);
    assert.equal(fixture.captures[1].settings.microphoneId, 'mic-2');
    assert.equal(media.inputs.at(-1), fixture.captures[1].mic, 'a new microphone feeds the relay');
    assert(fixture.captures[0].mic.track.stopped, 'the old microphone is released');
    await fixture.group.toggleMicrophone();
    assert.equal(media.sending.at(-1), false, 'mute stops relayed audio');
    assert.equal(fixture.group.getState().micMuted, true);
    const shared = fakeScreen();
    fixture.context.navigator.mediaDevices.getDisplayMedia = async () => shared;
    await fixture.group.toggleScreenSharing();
    assert.equal(media.screens.at(-1)[0], shared.video, 'a group share goes through the relay');
    assert(media.screens.at(-1)[1].frameRate <= 8);
    assert.equal(fixture.group.getState().screenSharing, true);
    await fixture.group.toggleScreenSharing();
    assert.equal(media.screens.at(-1)[0], null);
    assert(shared.video.stopped);
    assert.equal(fixture.group.getState().screenSharing, false);
    media.options.onPeerScreen('b', { id: 'screen-b' });
    const tile = fixture.elements.find(element => element.tag === 'article');
    assert(tile?.mounted, 'a relayed group share gets a screen tile');
    assert.equal(fixture.elements.find(element => element.tag === 'video').srcObject.id, 'screen-b');
    assert.equal(fixture.group.getState().participants.find(row => row.id === 'b').screenSharing, true);
    media.options.onPeerScreen('b', null);
    assert(tile.removed, 'the tile goes when the share stops');
    assert.equal(fixture.group.getState().participants.find(row => row.id === 'b').screenSharing, false);
    media.options.onPeerLeft('b');
    assert(audio.removed, 'a peer that left loses its audio element');
    await fixture.group.leave();
    assert(media.closed, 'leaving closes the relay');
    assert(fixture.captures[1].mic.track.stopped, 'leaving releases the microphone');
    assert.equal(fixture.group.getState().status, 'idle');
  }
  {
    // Push-to-talk on the relay path.
    const fixture = relayGroupFixture({ livekit: 'missing' });
    fixture.settings.pushToTalk = true;
    await fixture.group.join({ id: 'group' });
    const media = fixture.relay.instances[0];
    await fixture.group.toggleMicrophone();
    assert.notEqual(media.sending.at(-1), true, 'push-to-talk sends nothing until the key is held');
    await fixture.group.setPushToTalk(true);
    assert.equal(media.sending.at(-1), true);
    await fixture.group.setPushToTalk(false);
    assert.equal(media.sending.at(-1), false);
    await fixture.group.leave();
  }
  {
    let rejectConnect;
    const fixture = relayGroupFixture({ connect: async function () { await new Promise((_, reject) => { rejectConnect = reject; }); } });
    const joining = fixture.group.join({ id: 'group' });
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(fixture.rooms.length, 1, 'LiveKit is tried first when it is configured');
    assert.equal(fixture.relay.instances.length, 0);
    const fallback = fixture.timers.find(timer => timer.ms === 8_000 && !timer.cleared);
    assert(fallback, 'a relay fallback timer runs while LiveKit connects');
    fallback.fn();
    await joining;
    assert(fixture.rooms[0].disconnected, 'the unconnected LiveKit room is abandoned');
    assert.equal(fixture.relay.instances.length, 1, 'an unconnected LiveKit room switches to the server relay');
    assert.equal(fixture.group.getState().status, 'connected');
    assert.equal(fixture.group.getState().mediaPath, 'server');
    rejectConnect(new Error('late failure'));
    await fixture.group.leave();
  }
  {
    const fixture = relayGroupFixture();
    await fixture.group.join({ id: 'group' });
    const fallback = fixture.timers.find(timer => timer.ms === 8_000);
    if (fallback && !fallback.cleared) fallback.fn();
    assert.equal(fixture.relay.instances.length, 0, 'a connected LiveKit room never uses the relay');
    assert.equal(fixture.group.getState().mediaPath, 'livekit');
    await fixture.group.leave();
  }
  {
    // A LiveKit connect error with the relay available switches at once.
    const fixture = relayGroupFixture({ connect: async () => { throw new Error('No TURN candidates'); } });
    await fixture.group.join({ id: 'group' });
    assert.equal(fixture.relay.instances.length, 1);
    assert.equal(fixture.group.getState().status, 'connected');
    await fixture.group.leave();
  }

  // leave() cleans up even when the room's disconnect() rejects.
  {
    const fixture = livekitFixture();
    vm.runInNewContext(fs.readFileSync(path.join(ui, 'group-voice-client.js'), 'utf8'), fixture.context);
    const changes = [];
    const group = fixture.window.GhostChatGroupVoice.create({
      getToken: async () => ({ url: 'wss://fixture', token: 'fixture' }), getMembers: async () => [],
      getSettings: () => ({ volume: 100 }), getUserId: () => 'a', getDisplayName: () => 'Fixture', getAvatar: () => null,
      isPeerMuted: () => false, onChange: value => changes.push(value), onAudioBlocked() {},
    });
    await group.join({ id: 'group' });
    let removed = 0;
    const element = { dataset: {}, muted: false, volume: 1, play: async () => {}, remove() { removed++; }, setSinkId: async () => {} };
    const track = { sid: 'TR_peer', kind: 'audio', attach: () => element, detach() {} };
    const participant = { identity: 'peer-device', metadata: JSON.stringify({ account_id: 'peer' }), isMicrophoneEnabled: true, trackPublications: new Map([['TR_peer', { track }]]) };
    fixture.rooms[0].remoteParticipants.set('peer-device', participant);
    fixture.rooms[0].events.get('TrackSubscribed')(track, {}, participant);
    fixture.rooms[0].disconnect = async () => { throw new Error('signal connection lost'); };
    await group.leave().catch(() => {});
    assert.equal(removed, 1, 'a rejecting disconnect still removes the audio elements');
    assert.equal(group.getState().status, 'idle', 'a rejecting disconnect still resets the state');
    assert.equal(changes.at(-1).status, 'idle', 'the idle state is published');
  }

  // A device picked before first unmute must reach the shared engine,
  // without allowing LiveKit to capture an unprocessed microphone.
  {
    const fixture = livekitFixture();
    vm.runInNewContext(fs.readFileSync(path.join(ui, 'group-voice-client.js'), 'utf8'), fixture.context);
    const group = fixture.window.GhostChatGroupVoice.create({
      getToken: async () => ({ url: 'wss://fixture', token: 'fixture' }), getMembers: async () => [],
      getSettings: () => ({}), getUserId: () => 'a', getDisplayName: () => 'Fixture', getAvatar: () => null,
      isPeerMuted: () => false, onChange() {}, onAudioBlocked() {},
    });
    await group.join({ id: 'group' });
    await group.setMicrophoneDevice('next-mic');
    let selected;
    fixture.window.GhostChatAudio = { open: async settings => {
      selected = settings.microphoneId;
      throw new Error('fixture capture unavailable');
    } };
    await assert.rejects(group.toggleMicrophone(), /fixture capture unavailable/);
    assert.equal(selected, 'next-mic', 'first unmute uses the selected device');
    assert.equal(fixture.rooms[0].switches, undefined, 'SDK capture is never used');
    await group.leave();
  }

  // Switching the speaker must keep a peer the user muted for themself muted.
  {
    const fixture = livekitFixture();
    vm.runInNewContext(fs.readFileSync(path.join(ui, 'group-voice-client.js'), 'utf8'), fixture.context);
    const group = fixture.window.GhostChatGroupVoice.create({
      getToken: async () => ({ url: 'wss://fixture', token: 'fixture' }), getMembers: async () => [],
      getSettings: () => ({ volume: 100 }), getUserId: () => 'a', getDisplayName: () => 'Fixture', getAvatar: () => null,
      isPeerMuted: () => false, onChange() {}, onAudioBlocked() {},
    });
    await group.join({ id: 'group' });
    const element = { dataset: {}, muted: false, volume: 1, play: async () => {}, remove() {}, setSinkId: async () => {} };
    const track = { sid: 'TR_peer', kind: 'audio', attach: () => element, detach() {} };
    const participant = { identity: 'peer-device', metadata: JSON.stringify({ account_id: 'peer' }), isMicrophoneEnabled: true, trackPublications: new Map([['TR_peer', { track }]]) };
    fixture.rooms[0].remoteParticipants.set('peer-device', participant);
    fixture.rooms[0].events.get('TrackSubscribed')(track, {}, participant);
    group.setPeerMuted('peer', true);
    assert.equal(element.muted, true);
    await group.setSpeakerDevice('next-speaker');
    assert.equal(element.muted, true, 'a speaker change keeps a locally muted peer muted');
    await group.leave();
  }

  // The ringtone stops when the incoming call is accepted, declined or hung up,
  // including while its AudioContext is still resuming.
  {
    let releaseResume; let started = 0; const intervals = new Set(); let nextId = 1;
    class FakeAudio {
      state = 'suspended'; currentTime = 0; destination = {};
      resume() { return new Promise(resolve => { releaseResume = () => { this.state = 'running'; resolve(); }; }); }
      createOscillator() { return { frequency: {}, connect() {}, disconnect() {}, start() { started++; }, stop() {} }; }
      createGain() { const param = { setValueAtTime() {}, linearRampToValueAtTime() {}, exponentialRampToValueAtTime() {} }; return { gain: param, connect() {}, disconnect() {} }; }
    }
    const window = { AudioContext: FakeAudio, addEventListener() {} };
    vm.runInNewContext(fs.readFileSync(path.join(ui, 'call-ringtone.js'), 'utf8'), {
      window, console, document: { addEventListener() {} },
      setInterval: () => { const id = nextId++; intervals.add(id); return id; }, clearInterval: id => intervals.delete(id),
    });
    window.GhostChatRingtone.setIncoming(true);
    assert.equal(intervals.size, 1);
    window.GhostChatRingtone.setIncoming(false);
    releaseResume(); await new Promise(resolve => setImmediate(resolve));
    assert.equal(started, 0, 'no ring plays after the call stopped ringing during resume');
    assert.equal(intervals.size, 0, 'the ring repeat timer is cleared');
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
  console.log(`Media relay checks passed: mandatory relay admission, no direct fallback, no SFU downgrade, group/share failures, reconnect state and vendored LiveKit ${SDK.version} RTC merges.`);
}
run().catch(error => { console.error(error); process.exitCode = 1; });
