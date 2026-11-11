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
  return { window, sockets, encoders, decoders, nodes, contexts, timers };
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
