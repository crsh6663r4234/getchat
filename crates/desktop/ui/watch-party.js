(() => {
  "use strict";

  const MAX_POSITION = 86_400;
  const MAX_REVISION = 1_000_000_000;
  const ACTIONS = new Set(["source", "play", "pause", "seek", "request", "state"]);

  function validateSource(value) {
    if (typeof value !== "string" || !value.trim() || value.length > 2048) throw new Error("Enter a direct HTTPS video link.");
    let url;
    try { url = new URL(value.trim()); } catch { throw new Error("Enter a valid video link."); }
    if (url.protocol !== "https:" || url.username || url.password || !/\.(mp4|webm|ogv|ogg|mov|m4v)$/i.test(url.pathname)) {
      throw new Error("Watch Party needs a direct HTTPS video file (MP4 or WebM recommended). Platform pages and protected streams are not supported.");
    }
    // A received video link must not probe loopback or private IPv4 services.
    const host = url.hostname.toLowerCase().replace(/^\[|\]$/g, "");
    if (host === "localhost" || host.endsWith(".localhost") || host === "::1" || host.startsWith("fe80:") || host.startsWith("fc") && host.includes(":") || host.startsWith("fd") && host.includes(":")) {
      throw new Error("Use a publicly reachable HTTPS video link.");
    }
    const ipv4 = /^(\d+)\.(\d+)\.(\d+)\.(\d+)$/.exec(host);
    if (ipv4) {
      const first = Number(ipv4[1]); const second = Number(ipv4[2]);
      if (first === 0 || first === 10 || first === 127 || first >= 224 || first === 169 && second === 254 || first === 172 && second >= 16 && second <= 31 || first === 192 && second === 168 || first === 100 && second >= 64 && second <= 127) {
        throw new Error("Use a publicly reachable HTTPS video link.");
      }
    }
    return url.href;
  }

  function create(api) {
    let binding = "";
    let awaitingState = false;
    let state = { source: "", playing: false, position: 0, revision: 0, actor: "", updatedAt: Date.now(), remote: false, action: "state" };
    let anchor = performance.now();
    const currentCall = () => {
      const call = api.getCall?.();
      return call && ["connected", "reconnecting"].includes(call.status) && call.conversationId && call.peerId ? call : null;
    };
    const clampPosition = value => {
      if (typeof value !== "number" || !Number.isFinite(value) || value < 0 || value > MAX_POSITION) throw new Error("The video position is outside the supported range.");
      return value;
    };
    function snapshot() {
      const elapsed = state.playing ? Math.max(0, (performance.now() - anchor) / 1000) : 0;
      return { ...state, position: Math.min(MAX_POSITION, state.position + elapsed) };
    }
    function notify() { api.onChange?.(snapshot()); }
    function reset() {
      binding = "";
      awaitingState = false;
      state = { source: "", playing: false, position: 0, revision: 0, actor: "", updatedAt: Date.now(), remote: false, action: "state" };
      anchor = performance.now();
      notify();
    }
    function bind(call) {
      const next = `${call.conversationId}:${call.peerId}`;
      if (binding && binding !== next) reset();
      binding = next;
    }
    function send(action, call = currentCall()) {
      if (!call) return;
      const value = snapshot();
      api.sendSignal({
        conversationId: call.conversationId, peerId: call.peerId, kind: "watch_party",
        payload: { source: value.source, playing: value.playing, position: value.position, revision: value.revision, sentAt: Date.now(), action },
      });
    }
    function publish(action, patch = {}) {
      const call = currentCall();
      if (!call) throw new Error("Join a direct call before starting Watch Party.");
      bind(call);
      const revision = state.revision + 1;
      if (revision > MAX_REVISION) throw new Error("Restart Watch Party to continue syncing.");
      const previousState = state;
      const previousAnchor = anchor;
      state = { ...snapshot(), ...patch, revision, actor: String(api.getUserId?.() || ""), updatedAt: Date.now(), remote: false, action };
      anchor = performance.now();
      try { send(action, call); }
      catch (error) {
        state = previousState;
        anchor = previousAnchor;
        throw error;
      }
      notify();
      return snapshot();
    }
    function setSource(source) {
      return publish("source", { source: validateSource(source), playing: false, position: 0 });
    }
    function stop() {
      return publish("source", { source: "", playing: false, position: 0 });
    }
    function update(patch = {}) {
      const call = currentCall();
      if (!call) throw new Error("Join a direct call before starting Watch Party.");
      // Clear the previous call's source before validating this operation.
      // Otherwise a late play/seek could reuse stale media in a new call.
      bind(call);
      if (!state.source) throw new Error("Choose a video before starting playback.");
      const position = patch.position === undefined ? snapshot().position : clampPosition(patch.position);
      const playing = patch.playing === undefined ? state.playing : !!patch.playing;
      const action = patch.playing === undefined ? "seek" : playing ? "play" : "pause";
      return publish(action, { position, playing });
    }
    function handleSignal(event) {
      const call = currentCall();
      if (!call || event?.kind !== "watch_party" || event.conversation_id !== call.conversationId || event.from_user_id !== call.peerId) return false;
      const payload = event.payload;
      if (!payload || !ACTIONS.has(payload.action) || typeof payload.playing !== "boolean" || !Number.isSafeInteger(payload.revision) || payload.revision < 1 || payload.revision > MAX_REVISION || !Number.isSafeInteger(payload.sentAt) || payload.sentAt < 0) return false;
      let source;
      let position;
      try {
        source = payload.source === "" ? "" : validateSource(payload.source);
        position = clampPosition(payload.position);
      } catch { return false; }
      bind(call);
      if (payload.action === "request") {
        // Requests do not change playback and responses never elicit a reply.
        if (state.revision > 0) send("state", call);
        return true;
      }
      // A viewer may load a matching local file after already receiving this
      // revision. Its explicit resync request needs the current timeline even
      // though the revision/source did not change. Consume only one response
      // so ordinary repeated state packets remain idempotent.
      const resync = awaitingState && payload.action === "state" && payload.revision === state.revision && source === state.source;
      if (payload.revision < state.revision || !resync && payload.revision === state.revision && event.from_user_id <= state.actor) return false;
      awaitingState = false;
      // Clock differences cannot trigger an unbounded seek. Compensate only
      // a small plausible transit delay, then anchor to our monotonic clock.
      const transit = payload.playing ? Math.max(0, Math.min(1.5, (Date.now() - payload.sentAt) / 1000)) : 0;
      state = {
        source, position: Math.min(MAX_POSITION, position + transit), playing: !!source && payload.playing,
        revision: payload.revision, actor: resync ? state.actor : event.from_user_id, updatedAt: Date.now(), remote: true, action: payload.action,
      };
      anchor = performance.now();
      notify();
      return true;
    }
    function requestState() {
      const call = currentCall();
      if (!call) return;
      bind(call);
      const previousAwaitingState = awaitingState;
      awaitingState = true;
      try {
        api.sendSignal({ conversationId: call.conversationId, peerId: call.peerId, kind: "watch_party", payload: {
          source: "", playing: false, position: 0, revision: Math.max(1, state.revision), sentAt: Date.now(), action: "request",
        } });
      } catch (error) { awaitingState = previousAwaitingState; throw error; }
    }
    return {
      getState: snapshot, setSource, stop, update, handleSignal, reset, requestState,
      play: position => update({ playing: true, position }),
      pause: position => update({ playing: false, position }),
      seek: position => update({ position }),
    };
  }
  window.GhostChatWatchParty = { create, validateSource };
})();
