// Node vm tests for the server media relay transport
// (crates/desktop/ui/server-media-client.js and server-media-worklet.js)
// with fakes for WebSocket, WebCodecs and Web Audio.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ui = path.resolve(__dirname, '../crates/desktop/ui');

function relayed(peerId, kind, seq, data) {
  const id = Buffer.from(peerId, 'utf8');
  const out = new Uint8Array(1 + id.length + 5 + data.length);
  out[0] = id.length; out.set(id, 1);
  out[1 + id.length] = kind;
  new DataView(out.buffer).setUint32(2 + id.length, seq);
  out.set(data, 6 + id.length);
  return out.buffer;
}

function fixture({ webCodecs = true } = {}) {
  const sockets = []; const encoders = []; const decoders = []; const nodes = []; const contexts = []; const timers = [];
  class FakeSocket {
    static OPEN = 1;
    readyState = 0; sent = []; bufferedAmount = 0;
    constructor(url) { this.url = url; sockets.push(this); }
    send(data) { this.sent.push(data); }
    close() { this.readyState = 3; this.closed = true; this.onclose?.({ code: 1000 }); }
    open() { this.readyState = 1; this.onopen?.(); }
    text(value) { this.onmessage?.({ data: JSON.stringify(value) }); }
    binary(buffer) { this.onmessage?.({ data: buffer }); }
  }
  class Node {
    constructor(context, name, options) { this.name = name; this.options = options; this.port = { posted: [], postMessage(value) { this.posted.push(value); } }; this.connections = []; nodes.push(this); }
    connect(target) { this.connections.push(target); return target; }
    disconnect() { this.connections = []; this.disconnected = true; }
  }
  class Context {
    state = 'running'; modules = []; sources = [];
    constructor(options) { this.options = options; contexts.push(this); this.audioWorklet = { addModule: async url => { this.modules.push(url); } }; }
    createMediaStreamSource(stream) { const source = new Node(this, 'source'); source.stream = stream; this.sources.push(source); return source; }
    createMediaStreamDestination() { const node = new Node(this, 'destination'); node.stream = { id: `stream-${nodes.length}` }; return node; }
    async resume() {}
    async close() { this.state = 'closed'; }
  }
  class Encoder {
    encoded = []; state = 'unconfigured';
    constructor(init) { this.init = init; encoders.push(this); }
    configure(config) { this.config = config; this.state = 'configured'; }
    encode(data) { this.encoded.push(data); this.init.output({ byteLength: 3, copyTo(target) { target.set([7, 8, 9]); } }); }
    close() { this.state = 'closed'; }
  }
  class Decoder {
    chunks = []; state = 'unconfigured';
    constructor(init) { this.init = init; decoders.push(this); }
    configure(config) { this.config = config; this.state = 'configured'; }
    decode(chunk) {
      this.chunks.push(chunk);
      this.init.output({ numberOfFrames: 960, format: 'f32', copyTo(target) { target.fill(0.5); }, close() { this.closed = true; } });
    }
    close() { this.state = 'closed'; }
  }
  const window = {};
  const context = {
    window, console, TextDecoder, Uint8Array, DataView, Float32Array,
    WebSocket: FakeSocket, AudioContext: Context, AudioWorkletNode: Node,
    AudioData: class { constructor(init) { Object.assign(this, init); } close() { this.closed = true; } },
    EncodedAudioChunk: class { constructor(init) { Object.assign(this, init); } },
    setTimeout: (fn, ms) => { timers.push({ fn, ms, cleared: false }); return timers.length; },
    clearTimeout: id => { if (timers[id - 1]) timers[id - 1].cleared = true; },
  };
  if (webCodecs) { context.AudioEncoder = Encoder; context.AudioDecoder = Decoder; }
  vm.runInNewContext(fs.readFileSync(path.join(ui, 'server-media-client.js'), 'utf8'), context);
  return { window, context, sockets, encoders, decoders, nodes, contexts, timers };
}

function track(id = 'mic') { return { kind: 'audio', id }; }
function stream(id = 'mic') { const t = track(id); return { id, getAudioTracks: () => [t] }; }

async function join(env, options = {}) {
  const events = { streams: [], left: [], peers: [], closed: [] };
  const media = env.window.GhostChatServerMedia.create({
    url: 'wss://chat.fixture/v1/media/ws', token: 'session-token', conversationId: 'conversation',
    onPeerStream: (id, s) => events.streams.push([id, s]), onPeerLeft: id => events.left.push(id),
    onPeers: ids => events.peers.push(ids), onClosed: reason => events.closed.push(reason), ...options,
  });
  const connecting = media.connect();
  await new Promise(resolve => setImmediate(resolve));
  return { media, connecting, events, socket: env.sockets.at(-1) };
}

async function run() {
  // Joining: the hello carries the session and the room; ready lists peers.
  {
    const env = fixture();
    const { media, connecting, events, socket } = await join(env);
    assert.equal(socket.url, 'wss://chat.fixture/v1/media/ws');
    assert.equal(socket.binaryType, 'arraybuffer');
    assert.deepEqual(env.contexts[0].modules, ['server-media-worklet.js'], 'the worklet loads from the app, not a blob: URL');
    socket.open();
    assert.deepEqual(JSON.parse(socket.sent[0]), { token: 'session-token', conversation_id: 'conversation' });
    socket.text({ type: 'ready', user_id: 'me', peers: ['peer-b'] });
    await connecting;
    assert.deepEqual(events.streams.map(([id]) => id), ['peer-b'], 'every peer gets its own playback stream');
    assert.deepEqual([...media.peers()], ['peer-b']);
    assert.equal(env.decoders[0].config.codec, 'opus');

    // Incoming frames: the sender id selects the peer's decoder and worklet.
    socket.binary(relayed('peer-b', 1, 5, new Uint8Array([1, 2, 3])));
    const chunk = env.decoders[0].chunks[0];
    assert.deepEqual([...chunk.data], [1, 2, 3]);
    assert.equal(chunk.type, 'key');
    const playback = env.nodes.find(node => node.name === 'ghostchat-relay-playback');
    assert.equal(playback.port.posted.length, 1);
    assert.equal(playback.port.posted[0].length, 960);
    assert.equal(playback.port.posted[0][0], 0.5);

    // A new peer joins; an unknown sender's frame also opens its decoder.
    socket.text({ type: 'peer_joined', user_id: 'peer-c' });
    assert.deepEqual(events.streams.map(([id]) => id), ['peer-b', 'peer-c']);
    socket.text({ type: 'peer_left', user_id: 'peer-b' });
    assert.deepEqual(events.left, ['peer-b']);
    assert.equal(env.decoders[0].state, 'closed', 'a peer that left releases its decoder');
    assert(playback.disconnected);

    // Capture: nothing is sent until sending is on; mute stops frames.
    media.setInputStream(stream('mic-1'));
    const capture = env.nodes.find(node => node.name === 'ghostchat-relay-capture');
    assert(capture, 'the capture worklet is created');
    assert.equal(env.contexts[0].sources[0].stream.id, 'mic-1');
    const sentBefore = socket.sent.length;
    capture.port.onmessage({ data: new Float32Array(960) });
    assert.equal(socket.sent.length, sentBefore, 'a muted microphone sends nothing');
    media.setSending(true);
    capture.port.onmessage({ data: new Float32Array(960) });
    assert.equal(socket.sent.length, sentBefore + 1);
    const frame = new Uint8Array(socket.sent.at(-1));
    assert.equal(frame[0], 1, 'audio frames are tagged as Opus audio');
    assert.deepEqual([...frame.slice(5)], [7, 8, 9]);
    assert.equal(env.encoders[0].config.codec, 'opus');
    assert.equal(env.encoders[0].config.sampleRate, 48000);
    assert.equal(env.encoders[0].encoded[0].numberOfFrames, 960);
    capture.port.onmessage({ data: new Float32Array(960) });
    const seq = frame => new DataView(new Uint8Array(frame).buffer).getUint32(1);
    assert.equal(seq(socket.sent.at(-1)), seq(socket.sent.at(-2)) + 1, 'frames are numbered');
    // A congested connection drops new audio instead of queueing delay.
    socket.bufferedAmount = 1 << 20;
    const queued = socket.sent.length;
    capture.port.onmessage({ data: new Float32Array(960) });
    assert.equal(socket.sent.length, queued);
    socket.bufferedAmount = 0;
    media.setSending(false);
    capture.port.onmessage({ data: new Float32Array(960) });
    assert.equal(socket.sent.length, queued, 'mute or a released push-to-talk key stops frames');

    // Microphone device change swaps the source without a new socket.
    media.setInputStream(stream('mic-2'));
    assert(env.contexts[0].sources[0].disconnected, 'the old microphone is disconnected');
    assert.equal(env.contexts[0].sources[1].stream.id, 'mic-2');
    assert.equal(env.sockets.length, 1);
    media.setInputStream(null);
    assert(env.contexts[0].sources[1].disconnected);

    // The server closing the socket is reported once; close() releases all.
    socket.onclose({ code: 1006 });
    assert.equal(events.closed.length, 1);
    media.close();
    assert.equal(env.contexts[0].state, 'closed');
    assert.equal(env.encoders[0].state, 'closed');
  }

  // Refusals and timeouts reject the join with a readable reason.
  {
    const env = fixture();
    const { connecting, socket } = await join(env);
    socket.open();
    socket.text({ type: 'error', code: 'forbidden' });
    await assert.rejects(connecting, /not allowed to join/);
    assert(socket.closed);
  }
  {
    const env = fixture();
    const { connecting, socket } = await join(env);
    socket.open();
    const timer = env.timers.find(item => item.ms === 8000 && !item.cleared);
    assert(timer, 'joining is bounded by a timeout');
    timer.fn();
    await assert.rejects(connecting, /did not answer/);
    assert(socket.closed);
  }
  {
    const env = fixture();
    const { connecting, socket } = await join(env);
    socket.onclose({ code: 1006 });
    await assert.rejects(connecting, /could not reach/);
  }
  {
    const env = fixture({ webCodecs: false });
    assert.equal(env.window.GhostChatServerMedia.supported(), false);
    const media = env.window.GhostChatServerMedia.create({ url: 'wss://x', token: 't', conversationId: 'c' });
    await assert.rejects(media.connect(), /WebView2/);
    assert.equal(env.sockets.length, 0);
  }

  // A suspended AudioContext plays nothing: the relay resumes it when the
  // call starts, and on the next click or key press if Windows or the
  // WebView refused (for example a call accepted from a notification).
  {
    const env = fixture();
    const listeners = new Map();
    env.window.addEventListener = (type, fn) => { listeners.set(type, fn); };
    env.window.removeEventListener = (type, fn) => { if (listeners.get(type) === fn) listeners.delete(type); };
    let allowResume = false; let resumes = 0;
    env.context.AudioContext = class extends env.context.AudioContext {
      state = 'suspended';
      async resume() { resumes++; if (allowResume) { this.state = 'running'; this.onstatechange?.(); } }
    };
    const suspended = [];
    const { media, connecting, socket } = await join(env, { onAudioSuspended: () => suspended.push(true) });
    socket.open();
    socket.text({ type: 'ready', user_id: 'me', peers: ['peer-b'] });
    await connecting;
    await new Promise(resolve => setImmediate(resolve));
    assert(resumes >= 1, 'the relay resumes its AudioContext when the call starts');
    assert.equal(suspended.length, 1, 'the app is told the call audio is paused');
    for (const type of ['pointerdown', 'keydown']) assert(listeners.has(type), `a ${type} resumes the call audio`);
    allowResume = true;
    listeners.get('pointerdown')({ type: 'pointerdown' });
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(env.contexts[0].state, 'running', 'the first user gesture resumes the call audio');
    assert.equal(listeners.size, 0, 'the gesture listeners are removed once audio runs');
    // Windows can suspend the context again (device change, sleep).
    const before = resumes;
    env.contexts[0].state = 'suspended'; env.contexts[0].onstatechange();
    await new Promise(resolve => setImmediate(resolve));
    assert(resumes > before, 'a context suspended mid-call is resumed again');
    assert.equal(env.contexts[0].state, 'running');
    assert.equal(typeof media.resumeAudio, 'function', 'callers can resume the audio from a click');
    allowResume = false;
    env.contexts[0].state = 'suspended';
    await media.resumeAudio();
    assert(listeners.has('pointerdown'));
    media.close();
    assert.equal(listeners.size, 0, 'closing the relay removes the gesture listeners');
  }

  // Screen sharing over the relay: low frame rate VP8 through WebCodecs,
  // fragmented under the server's frame limit, reassembled per peer.
  {
    const env = fixture();
    const sentKeys = [];
    const videoEncoders = []; const videoDecoders = []; const canvases = [];
    let pushFrame; let endFrames;
    Object.assign(env.context, {
      MediaStreamTrackProcessor: class {
        constructor({ track }) {
          this.track = track;
          const queue = []; let wake = null;
          pushFrame = frame => { queue.push(frame); wake?.(); };
          endFrames = () => { queue.push(null); wake?.(); };
          this.readable = { getReader: () => ({
            read: async () => {
              while (!queue.length) await new Promise(resolve => { wake = resolve; });
              const value = queue.shift();
              return value ? { value, done: false } : { value: undefined, done: true };
            },
            cancel: async () => { queue.push(null); wake?.(); },
          }) };
        }
      },
      VideoEncoder: class {
        state = 'unconfigured'; encodeQueueSize = 0;
        constructor(init) { this.init = init; videoEncoders.push(this); }
        configure(config) { this.config = config; this.state = 'configured'; }
        encode(frame, options) {
          sentKeys.push(!!options?.keyFrame);
          const size = options?.keyFrame ? 100_000 : 2_000;
          this.init.output({ type: options?.keyFrame ? 'key' : 'delta', timestamp: frame.timestamp, byteLength: size, copyTo(target) { target.fill(options?.keyFrame ? 1 : 2); } });
        }
        close() { this.state = 'closed'; }
      },
      VideoDecoder: class {
        state = 'unconfigured'; chunks = [];
        constructor(init) { this.init = init; videoDecoders.push(this); }
        configure(config) { this.config = config; this.state = 'configured'; }
        decode(chunk) { this.chunks.push(chunk); this.init.output({ displayWidth: 640, displayHeight: 360, close() { this.closed = true; } }); }
        close() { this.state = 'closed'; }
      },
      EncodedVideoChunk: class { constructor(init) { Object.assign(this, init); } },
      document: { createElement: tag => { const canvas = { tag, drawn: 0, getContext: () => ({ drawImage: () => { canvas.drawn++; } }), captureStream: () => ({ id: `canvas-${canvases.length}`, getTracks: () => [{ stop() {} }] }) }; canvases.push(canvas); return canvas; } },
    });
    const screens = [];
    const { media, connecting, socket } = await join(env, { onPeerScreen: (id, value) => screens.push([id, value]) });
    socket.open();
    socket.text({ type: 'ready', user_id: 'me', peers: ['peer-b'] });
    await connecting;
    assert.equal(env.window.GhostChatServerMedia.screenSupported(), true);
    media.setScreenTrack({ kind: 'video', id: 'screen' }, { frameRate: 5 });
    const tick = () => new Promise(resolve => setImmediate(resolve));
    const before = socket.sent.length;
    pushFrame({ timestamp: 0, displayWidth: 1281, displayHeight: 721, close() {} });
    await tick();
    assert.equal(videoEncoders[0].config.codec, 'vp8');
    assert.equal(videoEncoders[0].config.width % 2, 0, 'encoder dimensions are even');
    assert.equal(sentKeys[0], true, 'a share starts with a key frame');
    const keyFragments = socket.sent.slice(before).map(frame => new Uint8Array(frame));
    assert.equal(keyFragments.length, 3, 'a 100 KB key frame is split under the server limit');
    assert(keyFragments.every(frame => frame.length <= 48 * 1024 + 10 && frame[0] === 2));
    assert.deepEqual(keyFragments.map(frame => [frame[7], frame[8], frame[9]]), [[0, 3, 1], [1, 3, 1], [2, 3, 1]], 'fragments carry index, count and the key flag');
    pushFrame({ timestamp: 50_000, displayWidth: 1281, displayHeight: 721, close() {} });
    await tick();
    assert.equal(sentKeys.length, 1, 'frames above the share frame rate are dropped');
    pushFrame({ timestamp: 200_000, displayWidth: 1281, displayHeight: 721, close() {} });
    await tick();
    assert.deepEqual(sentKeys, [true, false]);
    socket.text({ type: 'peer_joined', user_id: 'peer-c' });
    pushFrame({ timestamp: 400_000, displayWidth: 1281, displayHeight: 721, close() {} });
    await tick();
    assert.equal(sentKeys.at(-1), true, 'a new viewer gets a key frame at once');
    // Receiving: a delta before any key frame is dropped; a reassembled key frame plays.
    const fragment = (id, index, count, key, bytes) => {
      const data = new Uint8Array(5 + bytes.length);
      new DataView(data.buffer).setUint16(0, id); data[2] = index; data[3] = count; data[4] = key ? 1 : 0; data.set(bytes, 5);
      return relayed('peer-b', 2, id, data);
    };
    socket.binary(fragment(7, 0, 1, false, new Uint8Array([9])));
    assert.equal(videoDecoders.length ? videoDecoders[0].chunks.length : 0, 0, 'a delta frame cannot start a picture');
    socket.binary(fragment(8, 1, 2, true, new Uint8Array([3, 4])));
    socket.binary(fragment(8, 0, 2, true, new Uint8Array([1, 2])));
    assert.equal(videoDecoders[0].config.codec, 'vp8');
    assert.deepEqual([...videoDecoders[0].chunks[0].data], [1, 2, 3, 4], 'fragments are reassembled in order');
    assert.equal(videoDecoders[0].chunks[0].type, 'key');
    assert.equal(screens.length, 1);
    assert.equal(screens[0][0], 'peer-b');
    assert.equal(screens[0][1].id.startsWith('canvas-'), true, 'the decoded share is a canvas stream');
    assert.equal(canvases[0].drawn, 1);
    socket.binary(fragment(9, 0, 1, false, new Uint8Array([5])));
    assert.equal(videoDecoders[0].chunks.at(-1).type, 'delta');
    // The sharer stops: the viewer's tile goes away.
    socket.binary(relayed('peer-b', 3, 0, new Uint8Array([0])));
    assert.deepEqual(screens.at(-1), ['peer-b', null]);
    assert.equal(videoDecoders[0].state, 'closed');
    const sentBeforeStop = socket.sent.length;
    media.setScreenTrack(null);
    await tick();
    const stop = new Uint8Array(socket.sent.at(-1));
    assert.equal(socket.sent.length, sentBeforeStop + 1);
    assert.equal(stop[0], 3, 'stopping a share tells the viewers');
    assert.equal(videoEncoders[0].state, 'closed');
    media.close();
  }
  {
    const env = fixture();
    assert.equal(env.window.GhostChatServerMedia.screenSupported(), false);
    const { media, connecting, socket } = await join(env);
    socket.open(); socket.text({ type: 'ready', user_id: 'me', peers: [] }); await connecting;
    assert.throws(() => media.setScreenTrack({ kind: 'video' }), /WebView2/);
    media.close();
  }
  // Worklets: capture packs 20 ms frames; playback buffers before playing.
  {
    const processors = {};
    const context = {
      AudioWorkletProcessor: class { constructor() { this.port = { posted: [], postMessage(value) { this.posted.push(value); } }; } },
      registerProcessor: (name, type) => { processors[name] = type; }, Float32Array,
    };
    vm.runInNewContext(fs.readFileSync(path.join(ui, 'server-media-worklet.js'), 'utf8'), context);
    const capture = new processors['ghostchat-relay-capture']();
    for (let index = 0; index < 8; index++) capture.process([[new Float32Array(128).fill(index)]]);
    assert.equal(capture.port.posted.length, 1, '8 x 128 samples hold one 960-sample frame');
    assert.equal(capture.port.posted[0].length, 960);
    assert.equal(capture.port.posted[0][959], 7);
    const playback = new processors['ghostchat-relay-playback']();
    const out = () => { const output = [new Float32Array(128)]; playback.process([], [output]); return output[0]; };
    playback.port.onmessage({ data: new Float32Array(960).fill(0.25) });
    assert.equal(out()[0], 0, 'one frame is not enough cushion to start');
    playback.port.onmessage({ data: new Float32Array(960).fill(0.25) });
    playback.port.onmessage({ data: new Float32Array(960).fill(0.25) });
    assert.equal(out()[0], 0.25, 'playback starts after 60 ms are buffered');
    for (let index = 0; index < 30; index++) playback.port.onmessage({ data: new Float32Array(960).fill(0.75) });
    assert(playback.buffered - playback.offset <= 960 * 10, 'the jitter buffer never holds more than 200 ms');
  }
  console.log('server media relay transport tests passed');
}

run().catch(error => { console.error(error); process.exitCode = 1; });
