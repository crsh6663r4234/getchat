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
    let microphone = null;
    let microphoneTrack = null;
    let selectedMicrophoneId;
    let audioQueue = Promise.resolve();
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
    let relayMicrophone = null;
    let relaySending = false;
    let relayScreenStream = null;
    const RELAY_SCREEN_FPS = 5;
    // While on LiveKit: a standby watch of the relay room (see followRelay).
    let relayWatch = null;
    let relayWatchTimer = null;
    const RELAY_WATCH_RETRY_MS = 2_000;

    function relayAvailable() {
      return typeof api.createServerMedia === "function";
    }

    function relayParticipants() {
      const local = {
        id: api.getUserId(), identity: api.getUserId(), name: api.getDisplayName(), avatar: api.getAvatar(),
        local: true, speaking: false, micMuted: !relaySending, micManuallyMuted, pushToTalkPressed,
        screenSharing: !!relayScreenStream, mutedForYou: false,
      };
      return [local, ...[...relayPeers].map(id => {
        const profile = members.get(id);
        return {
          id, identity: id, name: profile?.display_name || "GhostChat member", avatar: profile?.avatar_data || null,
          local: false, speaking: false, micMuted: false, micManuallyMuted: false, pushToTalkPressed: false,
          screenSharing: screenElements.has(`relay:${id}`), mutedForYou: mutedAccounts.has(id) || api.isPeerMuted(id),
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
      const mic = microphone || relayMicrophone;
      state = { ...state, ...patch, audioProcessing: mic?.status || null, localAudioStream: mic?.stream || null, participants: participants() };
      if (microphone && microphone.status.state !== 'running') { state.micAvailable = false; state.micMuted = true; }
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
        mountScreenTile(track.sid, members.get(accountId)?.display_name || participant.name || "Member", track.attach());
      }
    }

    function mountScreenTile(key, name, video) {
      const tile = document.createElement("article");
      tile.className = "group-screen-tile";
      const title = document.createElement("span");
      title.className = "screen-share-badge";
      title.textContent = `${name} · ${api.translate("Screen share")}`;
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
      screenElements.set(key, { tile, video });
      void video.play().catch(() => {});
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

    function addRelayScreen(accountId, stream) {
      removeRelayScreen(accountId);
      const video = document.createElement("video");
      video.srcObject = stream;
      mountScreenTile(`relay:${accountId}`, members.get(accountId)?.display_name || "Member", video);
    }

    function removeRelayScreen(accountId) {
      const key = `relay:${accountId}`;
      const screen = screenElements.get(key);
      if (!screen) return;
      api.onScreenRemoved?.(screen.video.srcObject);
      try { screen.video.pause(); } catch {}
      screen.video.srcObject = null;
      screen.tile.remove();
      screenElements.delete(key);
    }

    function stopRelayScreen() {
      const stream = relayScreenStream;
      relayScreenStream = null;
      if (stream) relay?.setScreenTrack(null);
      stream?.getTracks().forEach(track => { track.onended = null; track.stop(); });
    }

    async function toggleRelayScreen() {
      const media = relay;
      if (relayScreenStream) { stopRelayScreen(); publishState({ screenSharing: false }); return; }
      if (!navigator.mediaDevices?.getDisplayMedia) throw new Error("Screen capture is unavailable in this WebView. Update the Microsoft Edge WebView2 Runtime.");
      const options = screenShareOptions(true);
      publishState({ screenStarting: true });
      let stream = null;
      try {
        stream = await navigator.mediaDevices.getDisplayMedia({
          video: { width: { ideal: options.resolution.width }, height: { ideal: options.resolution.height }, frameRate: { ideal: RELAY_SCREEN_FPS } },
          audio: false,
        });
        if (relay !== media) { stream.getTracks().forEach(track => track.stop()); return; }
        const track = stream.getVideoTracks()[0];
        if (!track) throw new Error("The screen capture returned no picture.");
        media.setScreenTrack(track, { frameRate: RELAY_SCREEN_FPS, bitrate: Math.min(options.screenShareEncoding.maxBitrate, 1_200_000) });
        relayScreenStream = stream;
        track.onended = () => {
          if (relayScreenStream !== stream) return;
          stopRelayScreen();
          publishState({ screenSharing: false });
        };
        publishState({ screenSharing: true, screenStarting: false });
      } catch (error) {
        if (relayScreenStream !== stream) stream?.getTracks().forEach(track => track.stop());
        publishState({ screenStarting: false });
        if (error?.name === "NotAllowedError" || error?.name === "AbortError") return;
        throw error;
      }
    }

    function stopRelayMicrophone() {
      relaySending = false;
      relay?.setSending(false);
      relay?.setInputStream(null);
      const session = relayMicrophone;
      relayMicrophone = null;
      void session?.close();
    }

    function teardownRelay() {
      const active = relay;
      stopRelayScreen();
      for (const id of [...relayPeers]) removeRelayScreen(id);
      relay = null;
      stopRelayMicrophone();
      try { active?.close(); } catch {}
      for (const id of [...relayPeers]) removeRelayAudio(id);
      relayPeers.clear();
    }

    // The relay sends the audio engine's processed microphone, like LiveKit.
    async function openRelayMicrophone(deviceId) {
      if (!window.GhostChatAudio) throw new Error("The audio engine did not load. Restart GhostChat.");
      const settings = api.getSettings();
      const microphoneId = deviceId ?? selectedMicrophoneId ?? settings.microphoneId;
      return window.GhostChatAudio.open({ ...settings, microphoneId }, { onStatus() { if (relay) publishState(); } });
    }

    // The capture stays open while muted (as on LiveKit), so unmute and
    // push-to-talk start sending without a new device prompt or delay.
    async function setRelaySending(enabled) {
      const media = relay;
      if (!media) return;
      if (enabled && !relayMicrophone) {
        const session = await openRelayMicrophone();
        if (relay !== media) { await session.close(); return; }
        relayMicrophone = session;
        media.setInputStream(session.stream);
      }
      relaySending = !!enabled && !!relayMicrophone;
      relayMicrophone?.setMuted(!relaySending);
      media.setSending(relaySending);
    }

    async function replaceRelayMicrophone(deviceId) {
      const media = relay;
      if (!media || !relayMicrophone) return;
      const session = await openRelayMicrophone(deviceId);
      if (relay !== media) { await session.close(); return; }
      const previous = relayMicrophone;
      relayMicrophone = session;
      session.setMuted(!relaySending);
      media.setInputStream(session.stream);
      await previous.close();
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
        onPeerScreen: (accountId, stream) => {
          if (relay !== media) return;
          if (stream) addRelayScreen(accountId, stream); else removeRelayScreen(accountId);
          publishState();
        },
        onPeerLeft: accountId => {
          if (relay !== media) return;
          relayPeers.delete(accountId);
          removeRelayAudio(accountId);
          removeRelayScreen(accountId);
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

    // Members can end up on different paths: one member's LiveKit did not
    // connect within RELAY_FALLBACK_MS and it moved to the relay, another's
    // did. Each would then hear only the members on its own path. A member
    // on LiveKit therefore watches the relay room (standby: no audio, sends
    // nothing) and follows the first member that carries the call there, so
    // the whole group meets on the relay.
    function closeRelayWatch() {
      clearTimeout(relayWatchTimer); relayWatchTimer = null;
      const watch = relayWatch;
      relayWatch = null;
      try { watch?.close(); } catch {}
    }

    function watchRelay(ticket, activeRoom, activeConversation) {
      if (typeof api.watchServerMedia !== "function" || ticket !== joinGeneration || room !== activeRoom) return;
      closeRelayWatch();
      const stillWatching = () => relayWatch === watch && ticket === joinGeneration && room === activeRoom;
      const retry = () => {
        if (!stillWatching()) return;
        relayWatch = null;
        // Tunnels close idle sockets; reopen the watch while on LiveKit.
        relayWatchTimer = setTimeout(() => { relayWatchTimer = null; watchRelay(ticket, activeRoom, activeConversation); }, RELAY_WATCH_RETRY_MS);
      };
      const watch = api.watchServerMedia({
        conversationId: activeConversation.id,
        onPeerActive: () => { if (stillWatching()) void followRelay(ticket, activeRoom, activeConversation); },
        onClosed: retry,
      });
      relayWatch = watch;
      // A refused or unreachable watch (an older server) is not retried.
      Promise.resolve().then(() => watch.connect()).catch(() => { if (stillWatching()) relayWatch = null; });
    }

    async function followRelay(ticket, activeRoom, activeConversation) {
      if (ticket !== joinGeneration || room !== activeRoom || relay) return;
      closeRelayWatch();
      // Detach first so the room's Disconnected handler does not end the call.
      room = null;
      const previousMicrophone = microphone;
      microphone = null; microphoneTrack = null;
      screenOperation = null;
      api.onScreenRemoved?.();
      for (const element of audioElements.values()) element.remove();
      for (const screen of screenElements.values()) screen.tile.remove();
      audioElements.clear(); screenElements.clear(); activeSpeakers.clear();
      publishState({ status: "connecting", screenSharing: false, screenStarting: false });
      try { await activeRoom.disconnect(); } catch {}
      await previousMicrophone?.close();
      if (ticket !== joinGeneration) return;
      try {
        const result = await joinRelay(ticket, activeConversation);
        if (result?.cancelled || relay === null) return;
        // A member who was talking keeps talking on the relay.
        if (wantsMicrophone()) await setRelaySending(true);
        publishState({ micMuted: !relaySending, micManuallyMuted, pushToTalkPressed, micAvailable: true });
      } catch (error) {
        console.debug("Could not follow the group to the server relay.", error);
      }
    }

    function withRelayDeadline(promise) {
      if (!relayAvailable()) return promise;
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => reject(new Error("LiveKit did not connect in time.")), RELAY_FALLBACK_MS);
        promise.then(value => { clearTimeout(timer); resolve(value); }, error => { clearTimeout(timer); reject(error); });
      });
    }

    function wantsMicrophone() {
      return !micManuallyMuted && (!api.getSettings().pushToTalk || pushToTalkPressed);
    }

    function queueAudio(activeRoom, operation) {
      const task = audioQueue.catch(() => {}).then(async () => {
        if (room !== activeRoom) return;
        await operation();
      });
      audioQueue = task;
      return task;
    }

    async function syncMicrophone(activeRoom) {
      if (room !== activeRoom || !microphoneTrack) return;
      const track = microphoneTrack;
      if (wantsMicrophone()) await track.unmute();
      else await track.mute();
      if (room !== activeRoom || track !== microphoneTrack) return;
      // The current intent wins even if a mute/PTT event arrived during await.
      microphone.setMuted(!wantsMicrophone());
      publishState({ micMuted: !wantsMicrophone(), micManuallyMuted, pushToTalkPressed, micAvailable: true });
    }

    async function prepareMicrophone(activeRoom, settings) {
      if (!window.GhostChatAudio) throw new Error("The audio engine did not load. Restart GhostChat.");
      let prepared;
      prepared = await window.GhostChatAudio.open({ ...settings, microphoneId: selectedMicrophoneId ?? settings.microphoneId }, { onStatus() {
        if (room === activeRoom && microphone === prepared) publishState();
      } });
      if (room !== activeRoom) { await prepared.close(); return; }
      const previous = microphone;
      let track = microphoneTrack;
      try {
        if (track) {
          await track.replaceTrack(prepared.stream.getAudioTracks()[0], { userProvidedTrack: true });
        } else {
          track = new LK.LocalAudioTrack(prepared.stream.getAudioTracks()[0], undefined, true);
          track.stopOnMute = false;
          await track.mute();
          await activeRoom.localParticipant.publishTrack(track, { source: LK.Track.Source.Microphone, dtx: true, red: true });
        }
        if (room !== activeRoom) {
          try { await activeRoom.localParticipant.unpublishTrack(track); } catch {}
          await prepared.close(); return;
        }
        microphone = prepared; microphoneTrack = track;
        await previous?.close();
        await syncMicrophone(activeRoom);
      } catch (error) {
        await prepared.close();
        throw error;
      }
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
          joinGeneration++;
          closeRelayWatch();
          void microphone?.close(); microphone = null; microphoneTrack = null;
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
        if (ticket !== joinGeneration || room !== activeRoom) { await activeRoom.disconnect(); return { cancelled: true }; }
        await activeRoom.startAudio().catch(() => {});
        let micAvailable = true;
        try { micAvailable = (await LK.Room.getLocalDevices("audioinput", false)).length > 0; } catch {}
        if (ticket !== joinGeneration || room !== activeRoom) return { cancelled: true };
        // Joining a group never publishes the microphone automatically.
        // Only the shared processed track may publish microphone audio.
        if (room !== activeRoom) return { cancelled: true };
        publishState({ status: "connected", micMuted: true, micManuallyMuted: true, pushToTalkPressed: false, micAvailable, screenSharing: false, screenStarting: false, error: "", mediaPath: "livekit" });
        watchRelay(ticket, activeRoom, nextConversation);
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
      const activeRoom = room;
      room = null;
      const previousMicrophone = microphone;
      microphone = null; microphoneTrack = null;
      await previousMicrophone?.close();
      api.onScreenRemoved?.();
      screenOperation = null;
      closeRelayWatch();
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
      const activeRoom = room;
      if (!activeRoom) return;
      micManuallyMuted = microphone && microphone.status.state !== 'running' ? false : !micManuallyMuted;
      if (micManuallyMuted) microphone?.setMuted(true);
      return queueAudio(activeRoom, async () => {
        try {
          if ((!microphone || microphone.status.state !== 'running') && !micManuallyMuted) await prepareMicrophone(activeRoom, api.getSettings());
          else await syncMicrophone(activeRoom);
          if (room === activeRoom) publishState({ micManuallyMuted, micMuted: !wantsMicrophone() });
        } catch (error) {
          if (room === activeRoom) {
            micManuallyMuted = true; microphone?.setMuted(true);
            publishState({ micMuted: true, micManuallyMuted, micAvailable: !!microphone, error: error.message });
          }
          throw error;
        }
      });
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
      const activeRoom = room;
      if (!activeRoom || !api.getSettings().pushToTalk) return;
      pushToTalkPressed = !!pressed;
      if (!pressed) microphone?.setMuted(true);
      return queueAudio(activeRoom, () => syncMicrophone(activeRoom));
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
      if (relay) return toggleRelayScreen();
      const activeRoom = room;
      if (!activeRoom) return;
      if (screenOperation) return screenOperation;
      const enabled = !activeRoom.localParticipant.isScreenShareEnabled;
      return runScreenOperation(activeRoom, () => activeRoom.localParticipant.setScreenShareEnabled(enabled, screenShareOptions(enabled)));
    }

    async function refreshScreenSharing() {
      if (relay) {
        if (!relayScreenStream) return;
        stopRelayScreen();
        return toggleRelayScreen();
      }
      const activeRoom = room;
      if (!activeRoom?.localParticipant.isScreenShareEnabled) return;
      if (screenOperation) return screenOperation;
      return runScreenOperation(activeRoom, async () => {
        await activeRoom.localParticipant.setScreenShareEnabled(false);
        if (room !== activeRoom) return;
        await activeRoom.localParticipant.setScreenShareEnabled(true, screenShareOptions(true));
      });
    }

    function setMicrophoneDevice(deviceId) {
      selectedMicrophoneId = deviceId || "";
      if (relay) return replaceRelayMicrophone(deviceId || "");
      const activeRoom = room;
      if (!activeRoom || !microphone) return Promise.resolve();
      return queueAudio(activeRoom, () => prepareMicrophone(activeRoom, { ...api.getSettings(), microphoneId: deviceId || "" }));
    }

    function applyAudioSettings() {
      if (relay) return replaceRelayMicrophone().then(() => publishState());
      const activeRoom = room;
      if (!activeRoom || !microphone) { publishState(); return Promise.resolve(); }
      if (!wantsMicrophone()) microphone.setMuted(true);
      return queueAudio(activeRoom, () => prepareMicrophone(activeRoom, api.getSettings()));
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
