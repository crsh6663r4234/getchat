// Server media relay transport: call audio through the GhostChat server's
// authenticated WebSocket (/v1/media/ws) when TURN or LiveKit (UDP) cannot be
// reached, for example behind an HTTP-only tunnel.
//
// Microphone -> AudioWorklet (20 ms frames) -> WebCodecs AudioEncoder (Opus)
// -> binary WebSocket frames. Incoming frames -> one AudioDecoder and one
// playback AudioWorklet per peer -> a MediaStream per peer, which the caller
// plays through its usual <audio> element (speaker device, volume, mutes).
//
// Like TURN and LiveKit, the server relays the media: frames are not
// end-to-end encrypted.
//
// Frame format sent by this client: [kind: u8][sequence: u32 BE][data].
// The server prefixes [sender id length: u8][sender id] when forwarding.
// Kinds: 1 Opus audio; 2 VP8 screen-share fragment, whose data starts with
// [frame id: u16][fragment index: u8][fragment count: u8][flags: u8, 1 = key];
// 3 screen share stopped.
(() => {
  "use strict";
  const KIND_AUDIO = 1;
  const KIND_VIDEO = 2;
  const KIND_VIDEO_STOP = 3;
  // Fragments stay well under the server's 64 KiB frame limit.
  const VIDEO_FRAGMENT_BYTES = 48 * 1024;
  const VIDEO_HEADER_BYTES = 5;
  // A new screen frame is only encoded while the socket is nearly drained;
  // while a share runs, audio tolerates the queue one video frame leaves.
  const VIDEO_START_BUFFERED_BYTES = 16 * 1024;
  const AUDIO_BUFFERED_WITH_VIDEO = 128 * 1024;
  const KEY_FRAME_EVERY_US = 2_000_000;
  const SAMPLE_RATE = 48_000;
  const FRAME_SAMPLES = 960;
  const FRAME_MICROSECONDS = 20_000;
  const JOIN_TIMEOUT_MS = 8_000;
  // About 1 s of Opus at 32 kb/s: beyond this the connection is congested,
  // and sending more would only add delay.
  const MAX_BUFFERED_BYTES = 4 * 1024;
  const WORKLET_URL = "server-media-worklet.js";
  const REFUSALS = {
    unauthorized: "The call relay refused this session. Sign in again and call again.",
    forbidden: "You are not allowed to join this call through the server relay.",
    rate_limited: "Too many call relay connections. Wait a minute and try again.",
    bad_request: "The call relay did not understand this client. Update GhostChat.",
  };

  function supported() {
    return typeof WebSocket === "function"
      && typeof AudioEncoder === "function"
      && typeof AudioDecoder === "function"
      && typeof AudioData === "function"
      && typeof EncodedAudioChunk === "function"
      && typeof AudioWorkletNode === "function"
      && typeof audioContextType() === "function";
  }

  function screenSupported() {
    return typeof MediaStreamTrackProcessor === "function"
      && typeof VideoEncoder === "function"
      && typeof VideoDecoder === "function"
      && typeof EncodedVideoChunk === "function";
  }

  function audioContextType() {
    return window.AudioContext || (typeof AudioContext === "function" ? AudioContext : undefined);
  }

  function readPeerId(bytes, length) {
    if (typeof TextDecoder === "function") return new TextDecoder().decode(bytes.subarray(1, 1 + length));
    return String.fromCharCode(...bytes.subarray(1, 1 + length));
  }

  function parseRelayed(buffer) {
    const bytes = new Uint8Array(buffer);
    const length = bytes[0];
    if (bytes.length < 1 + length + 5) return null;
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    return {
      peerId: readPeerId(bytes, length),
      kind: bytes[1 + length],
      sequence: view.getUint32(2 + length),
      data: bytes.subarray(6 + length),
    };
  }

  function create(options) {
    const { url, token, conversationId } = options;
    const onPeerStream = options.onPeerStream || (() => {});
    const onPeerLeft = options.onPeerLeft || (() => {});
    const onPeers = options.onPeers || (() => {});
    const onClosed = options.onClosed || (() => {});
    const onPeerScreen = options.onPeerScreen || (() => {});
    let socket = null;
    let context = null;
    let ready = false;
    let closed = false;
    let sending = false;
    let encoder = null;
    let source = null;
    let capture = null;
    let sequence = 0;
    let captureTimestamp = 0;
    let screen = null;
    const peers = new Map();

    function socketOpen() {
      return socket && socket.readyState === (WebSocket.OPEN ?? 1);
    }

    function audioBufferLimit() {
      return screen ? AUDIO_BUFFERED_WITH_VIDEO : MAX_BUFFERED_BYTES;
    }

    function sendFrame(kind, data, limit = audioBufferLimit()) {
      if (!socketOpen() || socket.bufferedAmount > limit) return;
      const frame = new Uint8Array(5 + data.byteLength);
      frame[0] = kind;
      new DataView(frame.buffer).setUint32(1, sequence);
      sequence = (sequence + 1) >>> 0;
      frame.set(data, 5);
      socket.send(frame.buffer);
    }

    function makeEncoder() {
      const next = new AudioEncoder({
        output: chunk => {
          const bytes = new Uint8Array(chunk.byteLength);
          chunk.copyTo(bytes);
          sendFrame(KIND_AUDIO, bytes);
        },
        error: error => {
          console.debug("Relay audio encoder failed; restarting it.", error);
          if (encoder === next) encoder = null;
        },
      });
      next.configure({ codec: "opus", sampleRate: SAMPLE_RATE, numberOfChannels: 1, bitrate: 32_000 });
      return next;
    }

    function encodePcm(pcm) {
      if (!sending || closed || !socketOpen() || socket.bufferedAmount > audioBufferLimit()) return;
      if (!encoder || encoder.state === "closed") encoder = makeEncoder();
      const audio = new AudioData({
        format: "f32-planar",
        sampleRate: SAMPLE_RATE,
        numberOfFrames: pcm.length,
        numberOfChannels: 1,
        timestamp: captureTimestamp,
        data: pcm,
      });
      captureTimestamp += FRAME_MICROSECONDS * (pcm.length / FRAME_SAMPLES);
      try { encoder.encode(audio); } finally { audio.close?.(); }
    }

    function makeDecoder(peer) {
      const decoder = new AudioDecoder({
        output: data => {
          try {
            const pcm = new Float32Array(data.numberOfFrames);
            try { data.copyTo(pcm, { planeIndex: 0, format: "f32-planar" }); }
            catch { data.copyTo(pcm, { planeIndex: 0 }); }
            peer.node.port.postMessage(pcm, [pcm.buffer]);
          } finally { data.close?.(); }
        },
        error: error => {
          console.debug("Relay audio decoder failed; restarting it.", error);
          if (peer.decoder === decoder) peer.decoder = null;
        },
      });
      decoder.configure({ codec: "opus", sampleRate: SAMPLE_RATE, numberOfChannels: 1 });
      return decoder;
    }

    function stopPeerVideo(peerId, peer) {
      const video = peer?.video;
      if (!video) return;
      peer.video = null;
      try { video.decoder?.close(); } catch {}
      try { video.stream?.getTracks?.().forEach(track => track.stop()); } catch {}
      if (video.stream) onPeerScreen(peerId, null);
    }

    function makeVideoDecoder(peerId, peer, video) {
      const decoder = new VideoDecoder({
        output: frame => {
          try {
            if (peer.video !== video) return;
            if (!video.canvas) {
              video.canvas = document.createElement("canvas");
              video.draw = video.canvas.getContext("2d");
            }
            if (video.canvas.width !== frame.displayWidth) video.canvas.width = frame.displayWidth;
            if (video.canvas.height !== frame.displayHeight) video.canvas.height = frame.displayHeight;
            video.draw.drawImage(frame, 0, 0);
            if (!video.stream) {
              video.stream = video.canvas.captureStream();
              onPeerScreen(peerId, video.stream);
            }
          } finally { frame.close?.(); }
        },
        error: error => {
          // A closed decoder is rebuilt on the next fragment; the canvas and
          // its stream stay, so the viewer keeps the tile.
          console.debug("A relayed screen frame could not be decoded; waiting for the next key frame.", error);
          video.waitingKey = true;
        },
      });
      decoder.configure({ codec: "vp8", optimizeForLatency: true });
      return decoder;
    }

    function peerVideo(peerId, peer) {
      if (peer.video) {
        if (peer.video.decoder.state === "closed") {
          peer.video.decoder = makeVideoDecoder(peerId, peer, peer.video);
          peer.video.waitingKey = true;
        }
        return peer.video;
      }
      const video = { parts: new Map(), waitingKey: true, decoder: null, canvas: null, draw: null, stream: null };
      peer.video = video;
      video.decoder = makeVideoDecoder(peerId, peer, video);
      return video;
    }

    function handleVideoFragment(peerId, peer, frame) {
      if (!screenSupported() || frame.data.length < VIDEO_HEADER_BYTES) return;
      const view = new DataView(frame.data.buffer, frame.data.byteOffset, frame.data.byteLength);
      const id = view.getUint16(0);
      const index = frame.data[2];
      const count = frame.data[3];
      const key = (frame.data[4] & 1) === 1;
      if (!count || index >= count) return;
      const video = peerVideo(peerId, peer);
      let entry = video.parts.get(id);
      if (!entry) {
        entry = { count, key, chunks: new Array(count), received: 0 };
        video.parts.set(id, entry);
        // Keep only a few frames in flight; older incomplete ones are lost.
        while (video.parts.size > 4) video.parts.delete(video.parts.keys().next().value);
      }
      if (entry.chunks[index]) return;
      entry.chunks[index] = frame.data.slice(VIDEO_HEADER_BYTES);
      entry.received += 1;
      if (entry.received < entry.count) return;
      video.parts.delete(id);
      if (video.waitingKey && !entry.key) return;
      const length = entry.chunks.reduce((sum, chunk) => sum + chunk.length, 0);
      const data = new Uint8Array(length);
      let offset = 0;
      for (const chunk of entry.chunks) { data.set(chunk, offset); offset += chunk.length; }
      video.waitingKey = false;
      try {
        video.decoder.decode(new EncodedVideoChunk({ type: entry.key ? "key" : "delta", timestamp: frame.sequence * 1000, data }));
      } catch (error) {
        console.debug("A relayed screen frame was refused by the decoder.", error);
        video.waitingKey = true;
      }
    }

    function sendVideoChunk(chunk) {
      if (!screen) return;
      const bytes = new Uint8Array(chunk.byteLength);
      chunk.copyTo(bytes);
      const id = screen.frameId = (screen.frameId + 1) & 0xffff;
      const count = Math.max(1, Math.ceil(bytes.length / VIDEO_FRAGMENT_BYTES));
      if (count > 255) { screen.forceKey = true; return; }
      for (let index = 0; index < count; index++) {
        const part = bytes.subarray(index * VIDEO_FRAGMENT_BYTES, (index + 1) * VIDEO_FRAGMENT_BYTES);
        const data = new Uint8Array(VIDEO_HEADER_BYTES + part.length);
        new DataView(data.buffer).setUint16(0, id);
        data[2] = index;
        data[3] = count;
        data[4] = chunk.type === "key" ? 1 : 0;
        data.set(part, VIDEO_HEADER_BYTES);
        sendFrame(KIND_VIDEO, data, Infinity);
      }
    }

    async function pumpScreen(active) {
      const reader = active.reader;
      for (;;) {
        let result;
        try { result = await reader.read(); } catch { break; }
        if (result.done || screen !== active) { result.value?.close?.(); break; }
        const frame = result.value;
        try {
          const interval = 1_000_000 / active.frameRate;
          if (active.lastTimestamp !== null && frame.timestamp - active.lastTimestamp < interval * 0.9) continue;
          if (!socketOpen() || socket.bufferedAmount > VIDEO_START_BUFFERED_BYTES || active.encoder.encodeQueueSize > 2) {
            // A dropped frame breaks the delta chain: restart with a key frame.
            active.forceKey = true;
            continue;
          }
          const width = Math.max(2, frame.displayWidth - (frame.displayWidth % 2));
          const height = Math.max(2, frame.displayHeight - (frame.displayHeight % 2));
          if (active.width !== width || active.height !== height) {
            active.encoder.configure({ codec: "vp8", width, height, bitrate: active.bitrate, framerate: active.frameRate, latencyMode: "realtime" });
            active.width = width; active.height = height; active.forceKey = true;
          }
          const keyFrame = active.forceKey || active.lastKey === null || frame.timestamp - active.lastKey >= KEY_FRAME_EVERY_US;
          if (keyFrame) { active.lastKey = frame.timestamp; active.forceKey = false; }
          active.lastTimestamp = frame.timestamp;
          active.encoder.encode(frame, { keyFrame });
        } catch (error) {
          console.debug("A screen frame could not be encoded for the relay.", error);
          active.forceKey = true;
        } finally { frame.close?.(); }
      }
    }

    // Share a screen track through the relay at a low frame rate; null stops.
    function setScreenTrack(track, { frameRate = 5, bitrate = 900_000 } = {}) {
      const previous = screen;
      screen = null;
      if (previous) {
        try { void previous.reader.cancel?.(); } catch {}
        try { previous.encoder.close(); } catch {}
        sendFrame(KIND_VIDEO_STOP, new Uint8Array([0]), Infinity);
      }
      if (!track || closed) return;
      if (!screenSupported()) throw new Error("Screen sharing through the server needs a newer Microsoft Edge WebView2 Runtime.");
      const active = {
        reader: new MediaStreamTrackProcessor({ track }).readable.getReader(),
        encoder: null, frameRate: Math.max(1, Math.min(15, frameRate)), bitrate,
        width: 0, height: 0, frameId: 0, forceKey: true, lastKey: null, lastTimestamp: null,
      };
      active.encoder = new VideoEncoder({
        output: chunk => { if (screen === active) sendVideoChunk(chunk); },
        error: error => { console.debug("The relay screen encoder failed.", error); if (screen === active) screen = null; },
      });
      screen = active;
      void pumpScreen(active);
    }

    function ensurePeer(peerId) {
      if (!peerId || peers.has(peerId)) return peers.get(peerId) || null;
      const node = new AudioWorkletNode(context, "ghostchat-relay-playback", {
        numberOfInputs: 0, numberOfOutputs: 1, outputChannelCount: [1],
      });
      const destination = context.createMediaStreamDestination();
      node.connect(destination);
      const peer = { node, destination, decoder: null, stream: destination.stream };
      peer.decoder = makeDecoder(peer);
      peers.set(peerId, peer);
      onPeerStream(peerId, peer.stream);
      onPeers([...peers.keys()]);
      return peer;
    }

    function removePeer(peerId) {
      const peer = peers.get(peerId);
      if (!peer) return;
      peers.delete(peerId);
      stopPeerVideo(peerId, peer);
      try { peer.decoder?.close(); } catch {}
      try { peer.node.disconnect(); peer.destination.disconnect?.(); } catch {}
      onPeerLeft(peerId);
      onPeers([...peers.keys()]);
    }

    function handleFrame(buffer) {
      const frame = parseRelayed(buffer);
      if (!frame || !frame.data.length) return;
      const peer = ensurePeer(frame.peerId);
      if (!peer) return;
      if (frame.kind === KIND_VIDEO) { handleVideoFragment(frame.peerId, peer, frame); return; }
      if (frame.kind === KIND_VIDEO_STOP) { stopPeerVideo(frame.peerId, peer); return; }
      if (frame.kind !== KIND_AUDIO) return;
      if (!peer.decoder || peer.decoder.state === "closed") peer.decoder = makeDecoder(peer);
      try {
        peer.decoder.decode(new EncodedAudioChunk({
          type: "key",
          timestamp: frame.sequence * FRAME_MICROSECONDS,
          data: frame.data,
        }));
      } catch (error) {
        console.debug("A relayed audio frame could not be decoded.", error);
      }
    }

    function handleText(raw) {
      let message;
      try { message = JSON.parse(raw); } catch { return; }
      if (message.type === "peer_joined") {
        ensurePeer(message.user_id);
        if (screen) screen.forceKey = true;
      }
      else if (message.type === "peer_left") removePeer(message.user_id);
    }

    async function connect() {
      if (!supported()) {
        throw new Error("This WebView cannot relay calls through the server. Update the Microsoft Edge WebView2 Runtime.");
      }
      const ContextType = audioContextType();
      context = new ContextType({ latencyHint: "interactive", sampleRate: SAMPLE_RATE });
      await context.audioWorklet.addModule(WORKLET_URL);
      void context.resume?.()?.catch?.(() => {});
      if (closed) throw new Error("The call ended.");
      return new Promise((resolve, reject) => {
        let settled = false;
        const finish = error => {
          if (settled) return;
          settled = true;
          clearTimeout(timer);
          if (error) { close(); reject(error); } else resolve();
        };
        const timer = setTimeout(() => finish(new Error("The call relay on the GhostChat server did not answer.")), JOIN_TIMEOUT_MS);
        socket = new WebSocket(url);
        socket.binaryType = "arraybuffer";
        socket.onopen = () => socket.send(JSON.stringify({ token, conversation_id: conversationId }));
        socket.onmessage = event => {
          if (typeof event.data !== "string") { if (ready) handleFrame(event.data); return; }
          if (ready) { handleText(event.data); return; }
          let message;
          try { message = JSON.parse(event.data); } catch { return; }
          if (message.type === "ready") {
            ready = true;
            for (const peerId of Array.isArray(message.peers) ? message.peers : []) ensurePeer(peerId);
            onPeers([...peers.keys()]);
            finish();
          } else if (message.type === "error") {
            finish(new Error(REFUSALS[message.code] || "The call relay refused the connection."));
          }
        };
        socket.onerror = () => {};
        socket.onclose = () => {
          if (!settled) { finish(new Error("GhostChat could not reach the call relay on the server.")); return; }
          if (!closed) { closed = true; release(); onClosed("The call relay connection was lost."); }
        };
      });
    }

    function setInputStream(stream) {
      if (source) { try { source.disconnect(); } catch {} source = null; }
      const track = stream?.getAudioTracks?.()[0];
      if (!track || !context || closed) return;
      if (!capture) {
        capture = new AudioWorkletNode(context, "ghostchat-relay-capture", { numberOfInputs: 1, numberOfOutputs: 0 });
        capture.port.onmessage = event => encodePcm(event.data);
      }
      if (!encoder) encoder = makeEncoder();
      source = context.createMediaStreamSource(stream);
      source.connect(capture);
    }

    function setSending(value) {
      sending = !!value;
    }

    function release() {
      if (screen) {
        try { void screen.reader.cancel?.(); } catch {}
        try { screen.encoder.close(); } catch {}
        screen = null;
      }
      ready = false;
      sending = false;
      try { source?.disconnect(); } catch {}
      try { capture?.disconnect(); } catch {}
      source = null; capture = null;
      try { encoder?.close(); } catch {}
      encoder = null;
      for (const peerId of [...peers.keys()]) removePeer(peerId);
      try { void context?.close?.(); } catch {}
    }

    function close() {
      const wasClosed = closed;
      closed = true;
      const active = socket;
      socket = null;
      if (active) {
        active.onclose = null;
        try { active.close(); } catch {}
      }
      if (!wasClosed || context?.state !== "closed") release();
    }

    return {
      connect,
      setInputStream,
      setSending,
      setScreenTrack,
      close,
      peers: () => new Set(peers.keys()),
      isConnected: () => ready && !closed,
    };
  }

  window.GhostChatServerMedia = { create, supported, screenSupported };
})();
