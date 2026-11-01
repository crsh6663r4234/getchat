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

    function emit() {
      api.onState?.({ connected: !!room, sharing: !!room?.localParticipant?.isScreenShareEnabled });
    }

    function clearRemote() {
      remoteStream = null;
      api.onRemoteStream?.(null);
    }

    function bind(activeRoom) {
      const E = LK.RoomEvent;
      activeRoom
        .on(E.TrackSubscribed, (track) => {
          if (track.kind !== LK.Track.Kind.Video || track.source !== LK.Track.Source.ScreenShare) return;
          remoteStream = new MediaStream([track.mediaStreamTrack]);
          track.mediaStreamTrack.onended = clearRemote;
          track.mediaStreamTrack.onmute = clearRemote;
          api.onRemoteStream?.(remoteStream);
        })
        .on(E.TrackUnsubscribed, track => {
          if (track.kind === LK.Track.Kind.Video && track.source === LK.Track.Source.ScreenShare) clearRemote();
        })
        .on(E.LocalTrackPublished, publication => {
          const track = publication.track;
          if (track?.kind !== LK.Track.Kind.Video || track.source !== LK.Track.Source.ScreenShare) return;
          localStream = new MediaStream([track.mediaStreamTrack]);
          track.mediaStreamTrack.onended = () => { localStream = null; api.onLocalStream?.(null); };
          api.onLocalStream?.(localStream);
          emit();
        })
        .on(E.LocalTrackUnpublished, publication => {
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
        await activeRoom.connect(credentials.url, credentials.token);
        emit();
        return activeRoom;
      } catch (error) {
        if (room === activeRoom) { room = null; conversationId = null; }
        try { await activeRoom.disconnect(); } catch {}
        throw error;
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
