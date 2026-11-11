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
  });

  function create(api) {
    let room = null;
    let generation = 0;
    let microphone = null;
    let microphoneTrack = null;
    let audioQueue = Promise.resolve();
    let conversation = null;
    let members = new Map();
    let activeSpeakers = new Set();
    let state = idleState();
    let micManuallyMuted = true;
    let pushToTalkPressed = false;
    let screenOperation = null;
    const audioElements = new Map();
    const screenElements = new Map();
    const mutedAccounts = new Set();

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
      state = { ...state, ...patch, audioProcessing: microphone?.status || null, localAudioStream: microphone?.stream || null, participants: participants() };
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
      prepared = await window.GhostChatAudio.open(settings, { onStatus() {
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
          generation++;
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

    async function join(nextConversation) {
      if (!LK?.Room) throw new Error("The group voice module did not load. Restart GhostChat and try again.");
      if (room) {
        if (conversation?.id === nextConversation.id) return;
        await leave();
      }
      const joining = ++generation;
      conversation = nextConversation;
      mutedAccounts.clear();
      micManuallyMuted = true;
      pushToTalkPressed = false;
      state = { ...idleState(), status: "connecting", conversationId: nextConversation.id };
      api.onChange({ ...state });
      try {
        const [credentials, memberRows] = await Promise.all([api.getToken(nextConversation.id), api.getMembers(nextConversation.id)]);
        if (joining !== generation) return;
        members = new Map(memberRows.map(member => [member.id, member]));
        const settings = api.getSettings();
        const activeRoom = new LK.Room({
          adaptiveStream: true,
          dynacast: true,
          audioOutput: settings.speakerId ? { deviceId: settings.speakerId } : undefined,
        });
        room = activeRoom;
        bindRoom(activeRoom);
        // Connect-time RTC overrides also feed LiveKit's ICE restart/full
        // reconnect configuration. Never enable host/srflx transport fallback.
        await activeRoom.connect(credentials.url, credentials.token, {
          rtcConfig: { iceTransportPolicy: "relay" },
          peerConnectionTimeout: 15_000,
        });
        if (joining !== generation || room !== activeRoom) { await activeRoom.disconnect(); return; }
        await activeRoom.startAudio().catch(() => {});
        let micAvailable = true;
        try { micAvailable = (await LK.Room.getLocalDevices("audioinput", false)).length > 0; } catch {}
        if (joining !== generation || room !== activeRoom) return;
        // Joining a group never publishes the microphone automatically.
        // No SDK microphone capture: only our processed user-provided track is published.
        publishState({ status: "connected", micMuted: true, micManuallyMuted: true, pushToTalkPressed: false, micAvailable, screenSharing: false, screenStarting: false, error: "" });
      } catch (error) {
        if (joining !== generation) return;
        if (room) {
          try { await room.disconnect(); } catch {}
          room = null;
        }
        const message = "Group voice could not establish its required relay connection. Verify the media server and its authenticated TURN service.";
        publishState({ ...idleState(), error: message });
        throw new Error(message);
      }
    }

    async function leave() {
      generation++;
      const activeRoom = room;
      room = null;
      const previousMicrophone = microphone;
      microphone = null; microphoneTrack = null;
      await previousMicrophone?.close();
      api.onScreenRemoved?.();
      screenOperation = null;
      if (activeRoom) await activeRoom.disconnect();
      conversation = null;
      members.clear();
      activeSpeakers.clear();
      for (const element of audioElements.values()) element.remove();
      for (const screen of screenElements.values()) screen.tile.remove();
      audioElements.clear(); screenElements.clear(); mutedAccounts.clear();
      state = idleState();
      api.onChange({ ...state });
    }

    async function toggleMicrophone() {
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

    function setMicrophoneDevice(deviceId) {
      const activeRoom = room;
      if (!activeRoom || !microphone) return Promise.resolve();
      return queueAudio(activeRoom, () => prepareMicrophone(activeRoom, { ...api.getSettings(), microphoneId: deviceId || "" }));
    }

    function applyAudioSettings() {
      const activeRoom = room;
      if (!activeRoom || !microphone) { publishState(); return Promise.resolve(); }
      if (!wantsMicrophone()) microphone.setMuted(true);
      return queueAudio(activeRoom, () => prepareMicrophone(activeRoom, api.getSettings()));
    }

    async function setSpeakerDevice(deviceId) {
      if (room && deviceId) await room.switchActiveDevice("audiooutput", deviceId);
      for (const [sid, element] of audioElements) {
        const track = room?.remoteParticipants ? [...room.remoteParticipants.values()].flatMap(p => [...p.trackPublications.values()].map(x => x.track)).find(item => item?.sid === sid) : null;
        if (track) applyAudioPreferences(element, identityAccountId(track.participant || {}));
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
