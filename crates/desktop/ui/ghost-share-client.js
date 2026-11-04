(() => {
  "use strict";

  // A deliberately small LiveKit wrapper for direct-call screen sharing. It
  // never publishes a microphone and it owns its tracks completely, so a
  // screen reconnect cannot renegotiate or interrupt the direct audio call.
  function create(api) {
    const LK = window.LivekitClient;
    let room = null;
    let conversationId = null;
    let localStream = null;
    let remoteStream = null;
    let remoteTrack = null;
    let remoteMuteTimer = null;

    function emit() {
      api.onState?.({ connected: !!room, sharing: !!room?.localParticipant?.isScreenShareEnabled });
    }

    function clearRemote() {
      if (remoteMuteTimer) clearTimeout(remoteMuteTimer);
      remoteMuteTimer = null;
      if (remoteTrack) {
        remoteTrack.onended = null;
        remoteTrack.onmute = null;
        remoteTrack.onunmute = null;
      }
      remoteTrack = null;
      remoteStream = null;
      api.onRemoteStream?.(null);
    }

    function bind(activeRoom) {
      const E = LK.RoomEvent;
      activeRoom
        .on(E.TrackSubscribed, (track) => {
          if (room !== activeRoom) return;
          if (track.kind !== LK.Track.Kind.Video || track.source !== LK.Track.Source.ScreenShare) return;
          clearRemote();
          remoteTrack = track.mediaStreamTrack;
          remoteStream = new MediaStream([track.mediaStreamTrack]);
          track.mediaStreamTrack.onended = clearRemote;
          // A transient mute happens during an SFU layer switch.  Do not blank
          // the share immediately, but remove a genuinely stopped source soon
          // enough that WebView2 cannot keep a frozen final frame.
          track.mediaStreamTrack.onmute = () => {
            if (remoteMuteTimer) clearTimeout(remoteMuteTimer);
            remoteMuteTimer = setTimeout(() => {
              if (remoteTrack === track.mediaStreamTrack && remoteTrack.muted) api.onState?.({ connected: true, sharing: !!localStream, reconnecting: true });
            }, 1200);
          };
          track.mediaStreamTrack.onunmute = () => {
            if (remoteMuteTimer) clearTimeout(remoteMuteTimer);
            remoteMuteTimer = null;
            if (remoteTrack === track.mediaStreamTrack) api.onRemoteStream?.(remoteStream);
          };
          api.onRemoteStream?.(remoteStream);
        })
        .on(E.TrackUnsubscribed, track => {
          if (room === activeRoom && track.mediaStreamTrack === remoteTrack) clearRemote();
        })
        .on(E.LocalTrackPublished, publication => {
          if (room !== activeRoom) return;
          const track = publication.track;
          if (track?.kind !== LK.Track.Kind.Video || track.source !== LK.Track.Source.ScreenShare) return;
          localStream = new MediaStream([track.mediaStreamTrack]);
          track.mediaStreamTrack.onended = () => { localStream = null; api.onLocalStream?.(null); };
          api.onLocalStream?.(localStream);
          emit();
        })
        .on(E.LocalTrackUnpublished, publication => {
          if (room !== activeRoom) return;
          if (publication.track?.kind === LK.Track.Kind.Video && publication.track.source === LK.Track.Source.ScreenShare) {
            localStream = null;
            api.onLocalStream?.(null);
            emit();
          }
        })
        .on(E.Disconnected, () => {
          if (room !== activeRoom) return;
          room = null; conversationId = null; localStream = null;
          clearRemote(); api.onLocalStream?.(null); emit();
        });
    }

    async function ensure(nextConversationId) {
      if (!LK?.Room) throw new Error("GhostShare could not load the media module.");
      if (room && conversationId === nextConversationId) return room;
      await leave();
      const credentials = await api.getToken(nextConversationId);
      const activeRoom = new LK.Room({ adaptiveStream: true, dynacast: true });
      room = activeRoom;
      conversationId = nextConversationId;
      bind(activeRoom);
      try {
        // LiveKit preserves this RTC override for resume and full reconnects.
        // TURN credentials are delivered by its authenticated signaling service.
        await activeRoom.connect(credentials.url, credentials.token, {
          rtcConfig: { iceTransportPolicy: "relay" },
          peerConnectionTimeout: 15_000,
        });
        emit();
        return activeRoom;
      } catch (error) {
        if (room === activeRoom) { room = null; conversationId = null; }
        try { await activeRoom.disconnect(); } catch {}
        throw new Error("GhostShare could not establish its required relay connection. Verify the media server and its authenticated TURN service.");
      }
    }

    async function start(nextConversationId, options) {
      const activeRoom = await ensure(nextConversationId);
      await activeRoom.localParticipant.setScreenShareEnabled(true, options);
      emit();
    }

    async function watch(nextConversationId) {
      await ensure(nextConversationId);
    }

    async function stop() {
      const activeRoom = room;
      if (!activeRoom) return;
      if (activeRoom.localParticipant.isScreenShareEnabled) await activeRoom.localParticipant.setScreenShareEnabled(false);
      localStream = null;
      api.onLocalStream?.(null);
      emit();
    }

    async function leave() {
      const activeRoom = room;
      room = null; conversationId = null; localStream = null;
      clearRemote(); api.onLocalStream?.(null);
      if (activeRoom) await activeRoom.disconnect();
      emit();
    }

    return { start, watch, stop, leave, getState: () => ({ connected: !!room, sharing: !!room?.localParticipant?.isScreenShareEnabled, localStream, remoteStream }) };
  }

  window.GhostChatShare = { create };
})();
