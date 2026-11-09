const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ui = path.resolve(__dirname, '../crates/desktop/ui');

function directFixture(config, { failShare = false, relayOnly = false, relayResponds = true, deferConfig = false, shareFactory = null, invisible = false, screen = null } = {}) {
  const intervals = [];
  const pcs = []; const timers = []; const errors = []; let releaseConfig; let settingsFailures = 0; const configGate = new Promise(resolve => { releaseConfig = resolve; }); const probes = []; const signals = []; let micRequests = 0; let micSource = null; let screenRequests = 0; let shareLeaves = 0;
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
  vm.runInNewContext(fs.readFileSync(path.join(ui, 'voice-client.js'), 'utf8'), context);
  const client = window.GhostChatVoice.create({
    getSettings: () => { if (settingsFailures > 0) { settingsFailures--; throw new Error('settings unavailable'); } return { relayOnly }; }, getIceConfiguration: async () => { if (deferConfig) await configGate; return config; },
    getUserId: () => 'a', getShareToken: async () => ({}), sendSignal: signal => signals.push(signal),
    onChange() {}, onError: message => errors.push(message), onIncoming() {}, isInvisible: () => invisible,
  });
  return { client, pcs, probes, signals, timers, intervals, errors, releaseConfig, failSettingsOnce: () => { settingsFailures = 1; }, counts: () => ({ micRequests, screenRequests, shareLeaves }), setMic: fn => { micSource = fn; } };
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

  // Changing the microphone while muted must still reach LiveKit: an unmute
  // only reacquires the track when a device change is pending.
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
    assert.deepEqual(fixture.rooms[0].switches, [['audioinput', 'next-mic']], 'a muted group member still switches microphone');
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
  console.log(`Media relay checks passed: strict relay-only admission, relay probe with direct fallback, no SFU downgrade, group/share failures, reconnect state and vendored LiveKit ${SDK.version} RTC merges.`);
}
run().catch(error => { console.error(error); process.exitCode = 1; });
