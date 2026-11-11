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
(() => {
  "use strict";
  const KIND_AUDIO = 1;
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
    const peers = new Map();

    function socketOpen() {
      return socket && socket.readyState === (WebSocket.OPEN ?? 1);
    }

    function sendFrame(kind, data) {
      if (!socketOpen() || socket.bufferedAmount > MAX_BUFFERED_BYTES) return;
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
      if (!sending || closed || !socketOpen() || socket.bufferedAmount > MAX_BUFFERED_BYTES) return;
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
      try { peer.decoder?.close(); } catch {}
      try { peer.node.disconnect(); peer.destination.disconnect?.(); } catch {}
      onPeerLeft(peerId);
      onPeers([...peers.keys()]);
    }

    function handleFrame(buffer) {
      const frame = parseRelayed(buffer);
      if (!frame || frame.kind !== KIND_AUDIO || !frame.data.length) return;
      const peer = ensurePeer(frame.peerId);
      if (!peer) return;
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
      if (message.type === "peer_joined") ensurePeer(message.user_id);
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
      close,
      peers: () => new Set(peers.keys()),
      isConnected: () => ready && !closed,
    };
  }

  window.GhostChatServerMedia = { create, supported };
})();
