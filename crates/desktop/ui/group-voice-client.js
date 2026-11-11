(function () {
  "use strict";

  const LK = window.LivekitClient;
  const idleState = () => ({
    status: "idle",
    conversationId: null,
    participants: [],
    micMuted: true,
    micAvailable: false,
    screenSharing: false,
    screenStarting: false,
    error: "",
    // "livekit" or "server" (the GhostChat server media relay).
    mediaPath: null,
  });
  // LiveKit needs UDP, which an HTTP-only tunnel to the GhostChat server
  // cannot carry. A room that has not connected after this long switches to
  // the server media relay.
  const RELAY_FALLBACK_MS = 8_000;

  function create(api) {
    let room = null;
    let conversation = null;
    let members = new Map();
    let activeSpeakers = new Set();
    let state = idleState();
    let micManuallyMuted = true;
    let pushToTalkPressed = false;
    let screenOperation = null;
    // Bumped by leave() and by every join so a join still awaiting its token
    // or connect cannot attach a room after the user left.
    let joinGeneration = 0;
    const audioElements = new Map();
    const screenElements = new Map();
    const mutedAccounts = new Set();
    // Server media relay session (when LiveKit is missing or unreachable).
    let relay = null;
    const relayPeers = new Set();
    let relayMicStream = null;
    let relaySending = false;

    function relayAvailable() {
      return typeof api.createServerMedia === "function";
    }

    function relayParticipants() {
      const local = {
        id: api.getUserId(), identity: api.getUserId(), name: api.getDisplayName(), avatar: api.getAvatar(),
        local: true, speaking: false, micMuted: !relaySending, micManuallyMuted, pushToTalkPressed,
        screenSharing: false, mutedForYou: false,
      };
      return [local, ...[...relayPeers].map(id => {
        const profile = members.get(id);
        return {
          id, identity: id, name: profile?.display_name || "GhostChat member", avatar: profile?.avatar_data || null,
          local: false, speaking: false, micMuted: false, micManuallyMuted: false, pushToTalkPressed: false,
          screenSharing: false, mutedForYou: mutedAccounts.has(id) || api.isPeerMuted(id),
        };
      })];
    }

    function identityAccountId(participant) {
      try {
        const metadata = JSON.parse(participant.metadata || "{}");
        if (typeof metadata.account_id === "string") return metadata.account_id;
      } catch {}
      const identity = String(participant.identity || "");
      for (const id of members.keys()) if (identity.startsWith(`${id}-`)) return id;
      return identity;
    }

    function participants() {
      if (relay) return relayParticipants();
      if (!room) return [];
      const local = room.localParticipant;
      const rows = [local, ...room.remoteParticipants.values()].map(participant => {
        const accountId = participant === local ? api.getUserId() : identityAccountId(participant);
        const profile = members.get(accountId);
        const isLocal = participant === local;
        return {
          id: accountId,
          identity: participant.identity,
          name: isLocal ? api.getDisplayName() : (profile?.display_name || participant.name || "GhostChat member"),
          avatar: isLocal ? api.getAvatar() : (profile?.avatar_data || null),
          local: isLocal,
          speaking: activeSpeakers.has(participant.identity) || participant.isSpeaking,
          micMuted: !participant.isMicrophoneEnabled,
          micManuallyMuted: isLocal && micManuallyMuted,
          pushToTalkPressed: isLocal && pushToTalkPressed,
          screenSharing: !!participant.isScreenShareEnabled,
          mutedForYou: !isLocal && (mutedAccounts.has(accountId) || api.isPeerMuted(accountId)),
        };
      });
      return rows;
    }

    function publishState(patch = {}) {
      state = { ...state, ...patch, participants: participants() };
      api.onChange({ ...state });
    }

    function applyAudioPreferences(element, accountId) {
      const settings = api.getSettings();
      element.muted = settings.speakerMuted || mutedAccounts.has(accountId) || api.isPeerMuted(accountId);
      element.volume = Math.max(0, Math.min(100, Number(settings.volume) || 0)) / 100;
      if (settings.speakerId && typeof element.setSinkId === "function") {
        void element.setSinkId(settings.speakerId).catch(() => {});
      }
    }

    function removeTrack(track) {
      const sid = track.sid;
      const audio = audioElements.get(sid);
      if (audio) {
        track.detach(audio);
        audio.remove();
        audioElements.delete(sid);
      }
      const screen = screenElements.get(sid);
      if (screen) {
        api.onScreenRemoved?.(screen.video.srcObject);
        track.detach(screen.video);
        try { screen.video.pause(); } catch {}
        screen.video.srcObject = null;
        screen.video.removeAttribute("src");
        try { screen.video.load(); } catch {}
        screen.tile.remove();
        screenElements.delete(sid);
      }
    }

    function addTrack(track, participant) {
      // Re-subscriptions may deliver the same SID after a reconnect.
      // Detach the old tile before mounting its replacement.
      if (audioElements.has(track.sid) || screenElements.has(track.sid)) removeTrack(track);
      const accountId = identityAccountId(participant);
      if (track.kind === LK.Track.Kind.Audio) {
        const element = track.attach();
        element.autoplay = true;
        element.playsInline = true;
        element.dataset.accountId = accountId;
        element.className = "group-voice-audio";
        applyAudioPreferences(element, accountId);
        document.querySelector("#groupVoiceMedia").append(element);
        audioElements.set(track.sid, element);
        void element.play().catch(() => api.onAudioBlocked());
      } else if (track.kind === LK.Track.Kind.Video && track.source === LK.Track.Source.ScreenShare) {
        const tile = document.createElement("article");
        tile.className = "group-screen-tile";
        const title = document.createElement("span");
        title.className = "screen-share-badge";
        title.textContent = `${members.get(accountId)?.display_name || participant.name || "Member"} · ${api.translate("Screen share")}`;
        const video = track.attach();
        video.autoplay = true;
        video.playsInline = true;
        video.muted = true;
        video.className = "group-screen-video";
        const expand = document.createElement("button");
        expand.className = "screen-expand-button";
        expand.type = "button";
        expand.textContent = api.translate("Expand");
        expand.onclick = () => api.onScreenExpand?.(video.srcObject);
        video.ondblclick = expand.onclick;
        tile.append(title, video, expand);
        document.querySelector("#groupVoiceMedia").append(tile);
        screenElements.set(track.sid, { tile, video });
        void video.play().catch(() => {});
      }
    }

    function addRelayAudio(accountId, stream) {
      removeRelayAudio(accountId);
      const element = document.createElement("audio");
      element.autoplay = true;
      element.playsInline = true;
      element.dataset.accountId = accountId;
      element.className = "group-voice-audio";
      element.srcObject = stream;
      applyAudioPreferences(element, accountId);
      document.querySelector("#groupVoiceMedia").append(element);
      audioElements.set(`relay:${accountId}`, element);
      void element.play().catch(() => api.onAudioBlocked());
    }

    function removeRelayAudio(accountId) {
      const element = audioElements.get(`relay:${accountId}`);
      if (!element) return;
      element.srcObject = null;
      element.remove();
      audioElements.delete(`relay:${accountId}`);
    }

    function stopRelayMicrophone() {
      relaySending = false;
      relay?.setSending(false);
      relay?.setInputStream(null);
      relayMicStream?.getTracks().forEach(track => track.stop());
      relayMicStream = null;
    }

    function teardownRelay() {
      const active = relay;
      relay = null;
      stopRelayMicrophone();
      try { active?.close(); } catch {}
      for (const id of [...relayPeers]) removeRelayAudio(id);
      relayPeers.clear();
    }

    async function captureRelayMicrophone(deviceId = null) {
      const audio = captureOptions();
      if (deviceId) audio.deviceId = deviceId;
      return navigator.mediaDevices.getUserMedia({ audio, video: false });
    }

    // The capture stays open while muted (as LiveKit does), so unmute and
    // push-to-talk start sending without a new device prompt or delay.
    async function setRelaySending(enabled) {
      const media = relay;
      if (!media) return;
      if (enabled && !relayMicStream) {
        const stream = await captureRelayMicrophone();
        if (relay !== media) { stream.getTracks().forEach(track => track.stop()); return; }
        relayMicStream = stream;
        media.setInputStream(stream);
      }
      relaySending = !!enabled && !!relayMicStream;
      media.setSending(relaySending);
    }

    async function replaceRelayMicrophone(deviceId = null) {
      const media = relay;
      if (!media || !relayMicStream) return;
      const stream = await captureRelayMicrophone(deviceId);
      if (relay !== media) { stream.getTracks().forEach(track => track.stop()); return; }
      const previous = relayMicStream;
      relayMicStream = stream;
      media.setInputStream(stream);
      previous.getTracks().forEach(track => track.stop());
    }

    async function joinRelay(ticket, nextConversation) {
      const media = api.createServerMedia({
        conversationId: nextConversation.id,
        onPeerStream: (accountId, stream) => {
          if (relay !== media) return;
          relayPeers.add(accountId);
          addRelayAudio(accountId, stream);
          publishState();
        },
        onPeerLeft: accountId => {
          if (relay !== media) return;
          relayPeers.delete(accountId);
          removeRelayAudio(accountId);
          publishState();
        },
        onClosed: () => {
          if (relay !== media) return;
          teardownRelay();
          conversation = null;
          publishState({ ...idleState(), error: "The group call relay connection to the GhostChat server was lost." });
        },
      });
      relay = media;
      try {
        await media.connect();
      } catch (error) {
        if (relay === media) teardownRelay(); else media.close();
        if (ticket !== joinGeneration) return { cancelled: true };
        const message = error?.message || "The call relay on the GhostChat server could not be reached.";
        publishState({ ...idleState(), error: message });
        throw new Error(message);
      }
      if (ticket !== joinGeneration || relay !== media) { media.close(); return { cancelled: true }; }
      publishState({ status: "connected", micMuted: true, micManuallyMuted: true, pushToTalkPressed: false, micAvailable: true, screenSharing: false, screenStarting: false, error: "", mediaPath: "server" });
    }

    function withRelayDeadline(promise) {
      if (!relayAvailable()) return promise;
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => reject(new Error("LiveKit did not connect in time.")), RELAY_FALLBACK_MS);
        promise.then(value => { clearTimeout(timer); resolve(value); }, error => { clearTimeout(timer); reject(error); });
      });
    }

    function captureOptions() {
      const settings = api.getSettings();
      const supported = navigator.mediaDevices?.getSupportedConstraints?.() || {};
      const options = {
        ...(settings.microphoneId ? { deviceId: settings.microphoneId } : {}),
        echoCancellation: !settings.nvidiaBroadcast && !!settings.echoCancellation,
        noiseSuppression: !settings.nvidiaBroadcast && !!settings.noiseSuppression,
        autoGainControl: !settings.nvidiaBroadcast && !!settings.autoGainControl,
      };
      // Keep the SFU capture speech-shaped as well: mono avoids needless
      // room noise bandwidth and the isolation hint is used when WebView2
      // exposes it. LiveKit falls back to browser defaults when unsupported.
      if (supported.channelCount) options.channelCount = { ideal: 1 };
      if (supported.sampleRate) options.sampleRate = { ideal: 48_000 };
      if (supported.latency) options.latency = { ideal: 0.01 };
      if (settings.voiceIsolation && supported.voiceIsolation) options.voiceIsolation = true;
      return options;
    }

    function bindRoom(activeRoom) {
      const E = LK.RoomEvent;
      activeRoom
        .on(E.TrackSubscribed, (track, _publication, participant) => { addTrack(track, participant); publishState(); })
        .on(E.TrackUnsubscribed, track => { removeTrack(track); publishState(); })
        .on(E.TrackPublished, () => publishState())
        .on(E.TrackUnpublished, publication => { if (publication.track) removeTrack(publication.track); publishState(); })
        .on(E.ParticipantConnected, () => publishState())
        .on(E.ParticipantDisconnected, () => publishState())
        .on(E.ActiveSpeakersChanged, speakers => {
          activeSpeakers = new Set(speakers.map(speaker => speaker.identity));
          publishState();
        })
        .on(E.LocalTrackPublished, (publication, participant) => { if (publication.track?.kind === LK.Track.Kind.Video && publication.track.source === LK.Track.Source.ScreenShare) addTrack(publication.track, participant); publishState({ micMuted: !activeRoom.localParticipant.isMicrophoneEnabled, screenSharing: !!activeRoom.localParticipant.isScreenShareEnabled, screenStarting: !!screenOperation }); })
        .on(E.LocalTrackUnpublished, publication => { if (publication.track) removeTrack(publication.track); publishState({ micMuted: !activeRoom.localParticipant.isMicrophoneEnabled, screenSharing: !!activeRoom.localParticipant.isScreenShareEnabled, screenStarting: !!screenOperation }); })
        .on(E.Reconnecting, () => publishState({ status: "reconnecting" }))
        .on(E.Reconnected, () => publishState({ status: "connected" }))
        .on(E.Disconnected, () => {
          if (room !== activeRoom) return;
          api.onScreenRemoved?.();
          for (const [sid, element] of audioElements) element.remove();
          for (const screen of screenElements.values()) screen.tile.remove();
          audioElements.clear(); screenElements.clear();
          room = null; screenOperation = null; activeSpeakers.clear();
          publishState(idleState());
        })
        .on(E.AudioPlaybackStatusChanged, canPlayback => { if (!canPlayback) api.onAudioBlocked(); });
    }

    // A join that a leave() or a newer join superseded resolves to
    // { cancelled: true } (not an error, and distinguishable from success).
    async function join(nextConversation) {
      if (!LK?.Room && !relayAvailable()) throw new Error("The group voice module did not load. Restart GhostChat and try again.");
      if (room || relay) {
        if (conversation?.id === nextConversation.id) return;
        await leave();
      }
      const ticket = ++joinGeneration;
      let activeRoom = null;
      conversation = nextConversation;
      mutedAccounts.clear();
      micManuallyMuted = true;
      pushToTalkPressed = false;
      state = { ...idleState(), status: "connecting", conversationId: nextConversation.id };
      api.onChange({ ...state });
      try {
        const [credentials, memberRows] = await Promise.all([
          // 503: LiveKit is not configured (or is still revoking a member);
          // the server relay carries the call instead.
          api.getToken(nextConversation.id).catch(error => {
            if (error?.status === 503 && relayAvailable()) return null;
            throw error;
          }),
          api.getMembers(nextConversation.id),
        ]);
        if (ticket !== joinGeneration) return { cancelled: true };
        members = new Map(memberRows.map(member => [member.id, member]));
        if (!credentials || !LK?.Room) return await joinRelay(ticket, nextConversation);
        const settings = api.getSettings();
        activeRoom = new LK.Room({
          adaptiveStream: true,
          dynacast: true,
          audioCaptureDefaults: captureOptions(),
          audioOutput: settings.speakerId ? { deviceId: settings.speakerId } : undefined,
        });
        room = activeRoom;
        bindRoom(activeRoom);
        // Connect-time RTC overrides also feed LiveKit's ICE restart/full
        // reconnect configuration. Never enable host/srflx transport fallback.
        await withRelayDeadline(activeRoom.connect(credentials.url, credentials.token, {
          rtcConfig: { iceTransportPolicy: "relay" },
          peerConnectionTimeout: 15_000,
        }));
        if (room !== activeRoom) return { cancelled: true };
        await activeRoom.startAudio().catch(() => {});
        let micAvailable = true;
        try { micAvailable = (await LK.Room.getLocalDevices("audioinput", false)).length > 0; } catch {}
        // Joining a group never publishes the microphone automatically.
        await activeRoom.localParticipant.setMicrophoneEnabled(false);
        if (room !== activeRoom) return { cancelled: true };
        publishState({ status: "connected", micMuted: true, micManuallyMuted: true, pushToTalkPressed: false, micAvailable, screenSharing: false, screenStarting: false, error: "", mediaPath: "livekit" });
      } catch (error) {
        const superseded = ticket !== joinGeneration || (activeRoom && room !== activeRoom);
        if (activeRoom && room === activeRoom) room = null;
        if (activeRoom) { try { await activeRoom.disconnect(); } catch {} }
        if (superseded) return { cancelled: true };
        // LiveKit failed or did not connect in time: use the server relay.
        if (activeRoom && relayAvailable()) {
          state = { ...idleState(), status: "connecting", conversationId: nextConversation.id };
          return joinRelay(ticket, nextConversation);
        }
        const message = "Group voice could not establish its required relay connection. Verify the media server and its authenticated TURN service.";
        publishState({ ...idleState(), error: message });
        throw new Error(message);
      }
    }

    async function leave() {
      joinGeneration += 1;
      api.onScreenRemoved?.();
      const activeRoom = room;
      room = null;
      screenOperation = null;
      teardownRelay();
      // Clean up even if disconnect() rejects; the error still reaches the caller.
      try {
        if (activeRoom) await activeRoom.disconnect();
      } finally {
        conversation = null;
        members.clear();
        activeSpeakers.clear();
        for (const element of audioElements.values()) element.remove();
        for (const screen of screenElements.values()) screen.tile.remove();
        audioElements.clear(); screenElements.clear(); mutedAccounts.clear();
        state = idleState();
        api.onChange({ ...state });
      }
    }

    async function toggleMicrophone() {
      if (relay) {
        try {
          if (api.getSettings().pushToTalk) {
            micManuallyMuted = !micManuallyMuted;
            if (micManuallyMuted) await setRelaySending(false);
          } else {
            const enabled = !relaySending;
            await setRelaySending(enabled);
            micManuallyMuted = !enabled;
          }
          publishState({ micMuted: !relaySending, micManuallyMuted, micAvailable: true });
        } catch (error) {
          micManuallyMuted = true;
          stopRelayMicrophone();
          publishState({ micMuted: true, micManuallyMuted, micAvailable: false });
          throw new Error(`Microphone stayed off: ${error.message || "device unavailable"}`);
        }
        return;
      }
      if (!room) return;
      if (api.getSettings().pushToTalk) {
        micManuallyMuted = !micManuallyMuted;
        if (micManuallyMuted && room.localParticipant.isMicrophoneEnabled) await room.localParticipant.setMicrophoneEnabled(false);
        publishState({ micMuted: !room.localParticipant.isMicrophoneEnabled, micManuallyMuted });
        return;
      }
      const enabled = !room.localParticipant.isMicrophoneEnabled;
      try {
        await room.localParticipant.setMicrophoneEnabled(enabled, captureOptions());
        micManuallyMuted = !enabled;
        publishState({ micMuted: !room.localParticipant.isMicrophoneEnabled, micManuallyMuted, micAvailable: true });
      } catch (error) {
        micManuallyMuted = true;
        publishState({ micMuted: true, micManuallyMuted, micAvailable: false });
        throw new Error(`Microphone stayed off: ${error.message || "device unavailable"}`);
      }
    }

    async function setPushToTalk(pressed) {
      if (relay) {
        pushToTalkPressed = pressed;
        try {
          await setRelaySending(pressed && !micManuallyMuted);
          publishState({ micMuted: !relaySending, pushToTalkPressed, micManuallyMuted, micAvailable: true });
        } catch (error) {
          micManuallyMuted = true;
          stopRelayMicrophone();
          publishState({ micMuted: true, micManuallyMuted, pushToTalkPressed: false, micAvailable: false });
          throw new Error(`Microphone stayed off: ${error.message || "device unavailable"}`);
        }
        return;
      }
      if (!room) return;
      pushToTalkPressed = pressed;
      const enabled = pressed && !micManuallyMuted;
      try {
        if (room.localParticipant.isMicrophoneEnabled !== enabled) {
          await room.localParticipant.setMicrophoneEnabled(enabled, captureOptions());
          publishState({ micMuted: !enabled, pushToTalkPressed, micManuallyMuted, micAvailable: true });
        } else {
          publishState({ pushToTalkPressed, micManuallyMuted });
        }
      } catch (error) {
        micManuallyMuted = true;
        publishState({ micMuted: true, micManuallyMuted, pushToTalkPressed: false, micAvailable: false });
        throw new Error(`Microphone stayed off: ${error.message || "device unavailable"}`);
      }
    }

    function screenShareOptions(enabled) {
      const settings = api.getSettings();
      const profiles = {
        detail: { width: 1920, height: 1080, frameRate: 12, maxBitrate: 3_200_000, hint: "text" },
        balanced: { width: 1280, height: 720, frameRate: 15, maxBitrate: 1_800_000, hint: "detail" },
        motion: { width: 1280, height: 720, frameRate: 24, maxBitrate: 3_800_000, hint: "motion" },
        saver: { width: 960, height: 540, frameRate: 10, maxBitrate: 850_000, hint: "detail" },
      };
      const profile = profiles[settings.shareQuality] || profiles.balanced;
      return {
        audio: enabled && settings.shareSystemAudio,
        systemAudio: enabled && settings.shareSystemAudio ? "include" : "exclude",
        contentHint: profile.hint,
        resolution: { width: profile.width, height: profile.height, frameRate: profile.frameRate },
        screenShareEncoding: { maxBitrate: profile.maxBitrate, maxFramerate: profile.frameRate },
        simulcast: true,
      };
    }

    async function runScreenOperation(activeRoom, operation) {
      if (screenOperation) return screenOperation;
      const task = Promise.resolve().then(async () => {
        publishState({ screenStarting: true });
        try {
          await operation();
        } finally {
          if (screenOperation === task) screenOperation = null;
          if (room === activeRoom) {
            publishState({
              screenSharing: !!activeRoom.localParticipant.isScreenShareEnabled,
              screenStarting: false,
            });
          }
        }
      });
      screenOperation = task;
      return task;
    }

    async function toggleScreenSharing() {
      if (relay) throw new Error("Screen sharing is not available yet on group calls relayed through the GhostChat server.");
      const activeRoom = room;
      if (!activeRoom) return;
      if (screenOperation) return screenOperation;
      const enabled = !activeRoom.localParticipant.isScreenShareEnabled;
      return runScreenOperation(activeRoom, () => activeRoom.localParticipant.setScreenShareEnabled(enabled, screenShareOptions(enabled)));
    }

    async function refreshScreenSharing() {
      const activeRoom = room;
      if (!activeRoom?.localParticipant.isScreenShareEnabled) return;
      if (screenOperation) return screenOperation;
      return runScreenOperation(activeRoom, async () => {
        await activeRoom.localParticipant.setScreenShareEnabled(false);
        if (room !== activeRoom) return;
        await activeRoom.localParticipant.setScreenShareEnabled(true, screenShareOptions(true));
      });
    }

    // LiveKit records the device for an unpublished or muted microphone and
    // reacquires it on unmute, so switch even while the microphone is off.
    async function setMicrophoneDevice(deviceId) {
      if (relay) { await replaceRelayMicrophone(deviceId || null); return; }
      if (room && deviceId) await room.switchActiveDevice("audioinput", deviceId);
    }

    // LiveKit reads capture defaults while publishing the microphone. Re-publish
    // an active track so the processing choices apply without leaving the room.
    async function applyAudioSettings() {
      if (relay) { await replaceRelayMicrophone(); publishState(); return; }
      if (!room?.localParticipant.isMicrophoneEnabled) { publishState(); return; }
      const settings = api.getSettings();
      const shouldEnable = !settings.pushToTalk || (pushToTalkPressed && !micManuallyMuted);
      await room.localParticipant.setMicrophoneEnabled(false);
      if (shouldEnable) await room.localParticipant.setMicrophoneEnabled(true, captureOptions());
      publishState({ micMuted: !shouldEnable, micManuallyMuted, pushToTalkPressed, micAvailable: true });
    }

    async function setSpeakerDevice(deviceId) {
      if (room && deviceId) await room.switchActiveDevice("audiooutput", deviceId);
      for (const [sid, element] of audioElements) {
        const track = room?.remoteParticipants ? [...room.remoteParticipants.values()].flatMap(p => [...p.trackPublications.values()].map(x => x.track)).find(item => item?.sid === sid) : null;
        // LiveKit tracks carry no participant reference; the element records
        // the account so per-peer mutes survive a speaker change.
        if (track) applyAudioPreferences(element, element.dataset.accountId || "");
        else if (typeof element.setSinkId === "function" && deviceId) await element.setSinkId(deviceId).catch(() => {});
      }
    }

    function setPeerMuted(accountId, muted) {
      if (muted) mutedAccounts.add(accountId); else mutedAccounts.delete(accountId);
      for (const element of audioElements.values()) if (element.dataset.accountId === accountId) applyAudioPreferences(element, accountId);
      publishState();
    }

    function applyPlaybackSettings() {
      for (const element of audioElements.values()) applyAudioPreferences(element, element.dataset.accountId || "");
    }

    return {
      join, leave, toggleMicrophone, setPushToTalk, toggleScreenSharing, refreshScreenSharing, applyAudioSettings,
      setMicrophoneDevice, setSpeakerDevice, setPeerMuted, applyPlaybackSettings,
      getState: () => ({ ...state, participants: participants() }),
    };
  }

  window.GhostChatGroupVoice = { create };
})();
