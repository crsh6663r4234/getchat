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
        track.detach(screen.video);
        screen.tile.remove();
        screenElements.delete(sid);
      }
    }

    function addTrack(track, participant) {
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
        tile.append(title, video);
        document.querySelector("#groupVoiceMedia").append(tile);
        screenElements.set(track.sid, { tile, video });
        void video.play().catch(() => {});
      }
    }

    function captureOptions() {
      const settings = api.getSettings();
      const supported = navigator.mediaDevices?.getSupportedConstraints?.() || {};
      const options = {
        ...(settings.microphoneId ? { deviceId: settings.microphoneId } : {}),
        echoCancellation: !!settings.echoCancellation,
        noiseSuppression: !!settings.noiseSuppression,
        autoGainControl: !!settings.autoGainControl,
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
      conversation = nextConversation;
      mutedAccounts.clear();
      micManuallyMuted = true;
      pushToTalkPressed = false;
      state = { ...idleState(), status: "connecting", conversationId: nextConversation.id };
      api.onChange({ ...state });
      try {
        const [credentials, memberRows] = await Promise.all([api.getToken(nextConversation.id), api.getMembers(nextConversation.id)]);
        members = new Map(memberRows.map(member => [member.id, member]));
        const settings = api.getSettings();
        const activeRoom = new LK.Room({
          adaptiveStream: true,
          dynacast: true,
          audioCaptureDefaults: captureOptions(),
          audioOutput: settings.speakerId ? { deviceId: settings.speakerId } : undefined,
        });
        room = activeRoom;
        bindRoom(activeRoom);
        await activeRoom.connect(credentials.url, credentials.token);
        await activeRoom.startAudio().catch(() => {});
        let micAvailable = true;
        try { micAvailable = (await LK.Room.getLocalDevices("audioinput", false)).length > 0; } catch {}
        // Joining a group never publishes the microphone automatically.
        await activeRoom.localParticipant.setMicrophoneEnabled(false);
        publishState({ status: "connected", micMuted: true, micManuallyMuted: true, pushToTalkPressed: false, micAvailable, screenSharing: false, screenStarting: false, error: "" });
      } catch (error) {
        if (room) {
          try { await room.disconnect(); } catch {}
          room = null;
        }
        publishState({ ...idleState(), error: error.message || "Could not join group voice." });
        throw error;
      }
    }

    async function leave() {
      const activeRoom = room;
      room = null;
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

    async function setMicrophoneDevice(deviceId) {
      if (room && room.localParticipant.isMicrophoneEnabled && deviceId) await room.switchActiveDevice("audioinput", deviceId);
    }

    // LiveKit reads capture defaults while publishing the microphone. Re-publish
    // an active track so the processing choices apply without leaving the room.
    async function applyAudioSettings() {
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
