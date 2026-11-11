(() => {
  function createVoiceClient(api) {
    let call = null;
    let callGeneration = 0;
    let starting = false;
    let incoming = null;
    let ringTimer = null;
    let latestState = { status: "idle" };

    const SHARE_PROFILES = {
      detail: [
        { id: "sharp", label: "Sharp text", width: 1920, height: 1080, frameRate: 12, maxBitrate: 3_200_000, hint: "text" },
        { id: "balanced", label: "Balanced", width: 1280, height: 720, frameRate: 15, maxBitrate: 1_800_000, hint: "detail" },
        { id: "saving", label: "Data saver", width: 960, height: 540, frameRate: 10, maxBitrate: 850_000, hint: "detail" },
      ],
      balanced: [
        { id: "balanced", label: "Balanced", width: 1280, height: 720, frameRate: 15, maxBitrate: 1_800_000, hint: "detail" },
        { id: "saving", label: "Data saver", width: 960, height: 540, frameRate: 10, maxBitrate: 850_000, hint: "detail" },
        { id: "recovery", label: "Recovery", width: 640, height: 360, frameRate: 8, maxBitrate: 450_000, hint: "detail" },
      ],
      motion: [
        { id: "motion", label: "Smooth motion", width: 1280, height: 720, frameRate: 24, maxBitrate: 3_800_000, hint: "motion" },
        { id: "balanced", label: "Balanced", width: 1280, height: 720, frameRate: 15, maxBitrate: 1_800_000, hint: "motion" },
        { id: "recovery", label: "Recovery", width: 640, height: 360, frameRate: 10, maxBitrate: 600_000, hint: "motion" },
      ],
      saver: [
        { id: "saving", label: "Data saver", width: 960, height: 540, frameRate: 10, maxBitrate: 850_000, hint: "detail" },
        { id: "recovery", label: "Recovery", width: 640, height: 360, frameRate: 8, maxBitrate: 450_000, hint: "detail" },
      ],
    };

    function snapshot() {
      latestState = {
        status: call?.status || (incoming ? "incoming" : "idle"),
        conversationId: call?.conversationId || incoming?.conversationId || "",
        peerId: call?.peerId || incoming?.peerId || "",
        // Report processing failures separately from the user's mute intent.
        micMuted: call ? (!call.localAudioTrack || call.audioSession?.status.state !== "running" || !!call.manualMute || (!!api.getSettings().pushToTalk && !call.pushToTalkPressed)) : false,
        micAvailable: !!call?.localAudioTrack && call?.audioSession?.status.state === "running",
        micManuallyMuted: !!call?.manualMute,
        pushToTalkPressed: !!call?.pushToTalkPressed,
        micUnavailableReason: call?.micUnavailableReason || "",
        pushToTalk: !!api.getSettings().pushToTalk,
        speakerMuted: !!api.getSettings().speakerMuted,
        screenSharing: !!call?.screenStream,
        screenStarting: !!call?.screenStarting,
        screenQuality: call?.screenQuality || null,
        screenTransport: call?.screenTransport || null,
        networkQuality: call?.networkQuality || null,
        // "turn" (WebRTC through TURN) or "server" (the GhostChat server
        // media relay, used when TURN is missing or does not connect).
        mediaPath: call?.mediaPath || null,
        callNote: call?.callNote || "",
        localAudioStream: call?.localStream || null,
        localScreenStream: call?.screenStream || null,
        remoteAudioStream: call?.remoteAudioStream || null,
        remoteVideoStream: call?.remoteVideoStream || null,
        audioProcessing: call?.audioProcessing || null,
        voiceGateOpen: true,
      };
      return latestState;
    }

    function notify() {
      api.onChange(snapshot());
    }

    function explainMediaError(error) {
      if (error?.name === "NotAllowedError" || error?.name === "PermissionDeniedError") {
        return "Microphone or screen permission was denied. Allow access in the Windows permission prompt and try again.";
      }
      if (error?.name === "NotFoundError" || error?.name === "OverconstrainedError") {
        return "The selected microphone is unavailable. Choose another device in Settings → Voice & screen.";
      }
      if (error?.name === "NotReadableError") return "The microphone is busy in another app. Close that app and try again.";
      if (error?.name === "SecurityError") return "This server context cannot access media. Use the GhostChat desktop app and an HTTPS server.";
      return error?.message || "The voice connection could not be started.";
    }

    function send(conversationId, peerId, kind, payload = null) {
      api.sendSignal({ conversationId, peerId, kind, payload });
    }

    function flushLocalIce(activeCall) {
      activeCall.signalReady = true;
      for (const candidate of activeCall.localIceQueue.splice(0)) {
        send(activeCall.conversationId, activeCall.peerId, "ice", candidate);
      }
    }

    function syncOutgoingAudio(activeCall) {
      const settings = api.getSettings();
      const muted = !!activeCall.manualMute || (!!settings.pushToTalk && !activeCall.pushToTalkPressed);
      activeCall.audioSession?.setMuted(muted);
      // The relay sends nothing while muted or without a microphone.
      activeCall.serverMedia?.setSending(!!activeCall.localAudioTrack && !muted);
    }

    async function tryCaptureMicrophone(settings) {
      try {
        if (!window.GhostChatAudio) throw new Error("The audio engine did not load. Restart GhostChat.");
        let session;
        session = await window.GhostChatAudio.open(settings, { onStatus(status) {
          if (call?.audioSession === session && session) { call.audioProcessing = status; notify(); }
        } });
        return { session, stream: session.stream, error: "" };
      } catch (error) {
        return { session: null, stream: null, error: explainMediaError(error) + " You joined muted and can still listen." };
      }
    }

    function findAudioTransceiver(activeCall) {
      return activeCall.audioTransceiver || activeCall.pc?.getTransceivers().find(item => item.receiver?.track?.kind === "audio") || null;
    }

    function findVideoTransceiver(activeCall) {
      return activeCall.videoTransceiver || activeCall.pc?.getTransceivers().find(item => item.receiver?.track?.kind === "video") || null;
    }

    // replaceTrack(null) is not reported consistently by every WebView2
    // build. This explicit cleanup also prevents a final frozen frame from
    // staying visible when the other caller stops sharing.
    function clearRemoteScreen(activeCall) {
      if (activeCall?.remoteScreenTimer) clearTimeout(activeCall.remoteScreenTimer);
      if (activeCall) activeCall.remoteScreenTimer = null;
      if (activeCall) activeCall.remoteScreenWatched = null;
      const stream = activeCall?.remoteVideoStream;
      if (!stream) return;
      stream.oninactive = null;
      for (const track of stream.getVideoTracks()) {
        track.onended = null;
        track.onmute = null;
        try { stream.removeTrack(track); } catch {}
      }
      activeCall.remoteVideoStream = null;
      notify();
    }

    // Direct WebRTC and GhostShare/SFU do not always deliver the same end
    // event. Watch both the stream and its track, then clear the video source
    // ourselves so a stopped share never leaves a frozen last frame behind.
    function setRemoteScreen(activeCall, stream) {
      if (activeCall?.remoteScreenWatched === stream) return;
      if (activeCall?.remoteVideoStream !== stream) clearRemoteScreen(activeCall);
      if (!stream) return;
      activeCall.remoteVideoStream = stream;
      activeCall.remoteScreenWatched = stream;
      const clear = () => {
        if (call === activeCall && activeCall.remoteVideoStream === stream) clearRemoteScreen(activeCall);
      };
      stream.oninactive = clear;
      for (const track of stream.getVideoTracks()) {
        track.onended = clear;
        track.onmute = () => {
          // Short transport hiccups can mute a remote track briefly; avoid
          // tearing down a healthy share for that, but never retain a freeze.
          if (activeCall.remoteScreenTimer) clearTimeout(activeCall.remoteScreenTimer);
          // Transport mute is recoverable: retain the track so onunmute can
          // restore the same share without requiring a fresh subscription.
          activeCall.remoteScreenTimer = setTimeout(() => { if (call === activeCall && track.muted) notify(); }, 900);
        };
        track.onunmute = () => { if (activeCall.remoteScreenTimer) clearTimeout(activeCall.remoteScreenTimer); activeCall.remoteScreenTimer = null; notify(); };
      }
      notify();
    }

    function shareProfiles(settings = api.getSettings()) {
      return SHARE_PROFILES[settings.shareQuality] || SHARE_PROFILES.balanced;
    }

    async function tuneScreenSender(sender, profile = shareProfiles()[0]) {
      if (!sender?.getParameters || !sender?.setParameters) return;
      try {
        const parameters = sender.getParameters();
        if (!parameters.encodings?.length) return;
        parameters.degradationPreference = "balanced";
        parameters.encodings[0].maxBitrate = profile.maxBitrate;
        parameters.encodings[0].maxFramerate = profile.frameRate;
        await sender.setParameters(parameters);
      } catch (error) {
        console.debug("Could not apply the low-latency screen profile; WebRTC defaults will be used.", error);
      }
    }

    async function captureScreen(settings) {
      const profile = shareProfiles(settings)[0];
      const stream = await navigator.mediaDevices.getDisplayMedia({
        preferCurrentTab: false,
        selfBrowserSurface: "exclude",
        surfaceSwitching: "include",
        video: {
          width: { ideal: profile.width },
          height: { ideal: profile.height },
          frameRate: { ideal: profile.frameRate },
        },
        audio: settings.shareSystemAudio,
      });
      const track = stream.getVideoTracks()[0];
      if (track) {
        try { track.contentHint = profile.hint; } catch {}
        try {
          await track.applyConstraints({
            width: { ideal: profile.width },
            height: { ideal: profile.height },
            frameRate: { ideal: profile.frameRate },
          });
        } catch (error) {
          console.debug("Screen capture constraints were not fully supported by this WebView.", error);
        }
      }
      stream.__ghostShareProfiles = shareProfiles(settings);
      return stream;
    }

    function stopShareMonitor(activeCall) {
      if (activeCall?.shareMonitor) clearInterval(activeCall.shareMonitor);
      if (activeCall) activeCall.shareMonitor = null;
    }

    function stopConnectionMonitor(activeCall) {
      if (activeCall?.connectionMonitor) clearInterval(activeCall.connectionMonitor);
      if (activeCall) activeCall.connectionMonitor = null;
    }

    // Keep quality feedback derived from the actual selected ICE path. This is
    // deliberately separate from screen-quality adaptation: a call can be
    // healthy while a high-resolution screen layer is temporarily reduced.
    function startConnectionMonitor(activeCall) {
      if (activeCall?.connectionMonitor) return;
      const sample = async () => {
        if (call !== activeCall || activeCall.pc?.connectionState !== "connected") return stopConnectionMonitor(activeCall);
        try {
          const reports = await activeCall.pc.getStats();
          let pair = null; let inbound = null;
          reports.forEach(report => {
            if (report.type === "candidate-pair" && (report.selected || report.nominated) && report.state === "succeeded") pair = report;
            if (report.type === "remote-inbound-rtp" && (report.kind === "audio" || report.kind === "video")) inbound = report;
          });
          const rtt = Number(inbound?.roundTripTime || pair?.currentRoundTripTime || 0);
          const lost = Number(inbound?.packetsLost || 0);
          const received = Number(inbound?.packetsReceived || 0);
          const previous = activeCall.networkSample || { lost, received };
          const lostDelta = Math.max(0, lost - previous.lost);
          const receivedDelta = Math.max(0, received - previous.received);
          activeCall.networkSample = { lost, received };
          const loss = (lostDelta || receivedDelta) ? lostDelta / Math.max(1, lostDelta + receivedDelta) : 0;
          const level = rtt > 0.42 || loss > 0.06 ? "weak" : rtt > 0.22 || loss > 0.022 ? "fair" : "good";
          const next = { level, rtt: Math.round(rtt * 1000), loss: Math.round(loss * 100), relay: pair?.localCandidateType === "relay" || pair?.remoteCandidateType === "relay" };
          if (JSON.stringify(activeCall.networkQuality) !== JSON.stringify(next)) {
            activeCall.networkQuality = next;
            notify();
          }
        } catch (error) { console.debug("Call quality sampling failed.", error); }
      };
      void sample();
      activeCall.connectionMonitor = setInterval(() => { void sample(); }, 2500);
    }

    // Stop the direct compatibility publisher in one place.  Source tracks
    // emit `ended` while the user is stopping a share, and calling the old
    // toggle function from that event could race a new share/refresh.  Clear
    // state first, then detach every sender so receivers never retain a frame.
    async function stopDirectScreenShare(activeCall, { notifyPeer = true } = {}) {
      if (!activeCall?.screenStream) return;
      if (activeCall.stopScreenPromise) return activeCall.stopScreenPromise;
      const stream = activeCall.screenStream;
      if (activeCall.screenTransport === "server") activeCall.serverMedia?.setScreenTrack(null);
      activeCall.screenStream = null;
      stopShareMonitor(activeCall);
      activeCall.screenQuality = null;
      activeCall.screenTransport = null;
      const task = (async () => {
        for (const track of stream.getTracks()) {
          const sender = activeCall.pc?.getSenders().find(item => item.track === track);
          if (sender && track.kind === "video" && sender === findVideoTransceiver(activeCall)?.sender) {
            await sender.replaceTrack(null);
          } else if (sender) {
            activeCall.pc?.removeTrack(sender);
          }
          track.onended = null;
          track.stop();
        }
        if (notifyPeer && call === activeCall) send(activeCall.conversationId, activeCall.peerId, "screen_stopped", null);
      })();
      activeCall.stopScreenPromise = task;
      try { await task; }
      finally {
        if (activeCall.stopScreenPromise === task) activeCall.stopScreenPromise = null;
        if (call === activeCall) notify();
      }
    }

    async function setShareLevel(activeCall, track, nextLevel, reason = "stable") {
      const profiles = activeCall.shareProfiles || shareProfiles();
      const level = Math.max(0, Math.min(profiles.length - 1, nextLevel));
      const profile = profiles[level];
      if (!track || !profile) return;
      try {
        try { track.contentHint = profile.hint; } catch {}
        await track.applyConstraints({ width: { ideal: profile.width }, height: { ideal: profile.height }, frameRate: { ideal: profile.frameRate } });
        await tuneScreenSender(findVideoTransceiver(activeCall)?.sender, profile);
        activeCall.shareLevel = level;
        activeCall.screenQuality = { profile: profile.id, label: profile.label, state: reason };
        notify();
      } catch (error) {
        console.debug("GhostShare could not change the capture profile.", error);
      }
    }

    function startShareMonitor(activeCall, track, profiles) {
      stopShareMonitor(activeCall);
      activeCall.shareProfiles = profiles || shareProfiles();
      activeCall.shareLevel = 0;
      activeCall.screenQuality = { profile: activeCall.shareProfiles[0].id, label: activeCall.shareProfiles[0].label, state: "stable" };
      let badSamples = 0;
      let goodSamples = 0;
      activeCall.shareMonitor = setInterval(async () => {
        if (call !== activeCall || activeCall.screenStream?.getVideoTracks()[0] !== track) return stopShareMonitor(activeCall);
        try {
          const reports = await activeCall.pc?.getStats?.();
          let pair = null; let remoteInbound = null;
          reports?.forEach(report => {
            if (report.type === "candidate-pair" && (report.selected || report.nominated) && report.state === "succeeded") pair = report;
            if (report.type === "remote-inbound-rtp" && report.kind === "video") remoteInbound = report;
          });
          const loss = remoteInbound && (remoteInbound.packetsLost || remoteInbound.packetsReceived)
            ? Number(remoteInbound.packetsLost || 0) / Math.max(1, Number(remoteInbound.packetsLost || 0) + Number(remoteInbound.packetsReceived || 0)) : 0;
          const rtt = Number(remoteInbound?.roundTripTime || pair?.currentRoundTripTime || 0);
          const capacity = Number(pair?.availableOutgoingBitrate || 0);
          const current = activeCall.shareProfiles[activeCall.shareLevel] || activeCall.shareProfiles[0];
          const congested = loss > 0.045 || rtt > 0.38 || (capacity > 0 && capacity < current.maxBitrate * 1.25);
          if (congested) { badSamples += 1; goodSamples = 0; }
          else { goodSamples += 1; badSamples = 0; }
          if (badSamples >= 2 && activeCall.shareLevel < activeCall.shareProfiles.length - 1) {
            badSamples = 0;
            await setShareLevel(activeCall, track, activeCall.shareLevel + 1, "network-adjusted");
          } else if (goodSamples >= 10 && activeCall.shareLevel > 0) {
            goodSamples = 0;
            await setShareLevel(activeCall, track, activeCall.shareLevel - 1, "recovered");
          }
        } catch (error) { console.debug("GhostShare network sampling failed.", error); }
      }, 1500);
    }

    function ghostShareOptions(settings) {
      const profile = shareProfiles(settings)[0];
      return {
        audio: !!settings.shareSystemAudio,
        systemAudio: settings.shareSystemAudio ? "include" : "exclude",
        contentHint: profile.hint,
        resolution: { width: profile.width, height: profile.height, frameRate: profile.frameRate },
        screenShareEncoding: { maxBitrate: profile.maxBitrate, maxFramerate: profile.frameRate },
        simulcast: true,
      };
    }

    function ensureGhostShare(activeCall) {
      if (activeCall.ghostShare) return activeCall.ghostShare;
      if (!window.GhostChatShare?.create || !api.getShareToken) throw new Error("GhostShare is unavailable in this app build.");
      activeCall.ghostShare = window.GhostChatShare.create({
        getToken: conversationId => api.getShareToken(conversationId),
        onLocalStream: stream => {
          if (call !== activeCall) return;
          const wasSharing = !!activeCall.screenStream;
          activeCall.screenStream = stream;
          activeCall.screenQuality = stream ? { profile: shareProfiles()[0].id, label: shareProfiles()[0].label, state: "sfu" } : null;
          activeCall.screenTransport = stream ? "sfu" : null;
          if (!stream && wasSharing) {
            try { send(activeCall.conversationId, activeCall.peerId, "screen_stopped", null); } catch {}
          }
          notify();
        },
        onRemoteStream: stream => {
          if (call !== activeCall) return;
          setRemoteScreen(activeCall, stream);
        },
      });
      return activeCall.ghostShare;
    }

    async function toggleGhostShare(activeCall) {
      const share = ensureGhostShare(activeCall);
      activeCall.screenStarting = true;
      notify();
      try {
        if (activeCall.screenStream) {
          await share.stop();
          // Both callers can share at once over the same SFU room. Leaving it
          // here would also tear down the peer's share we are still watching,
          // or one announced whose subscription has not arrived yet.
          if (!share.getState?.().remoteStream && !activeCall.peerGhostShareAnnounced) {
            await share.leave();
            activeCall.ghostShare = null;
          }
          // onLocalStream(null) already told the peer when the share module
          // reported the stop; only tell it here if that did not happen.
          const peerNotTold = !!activeCall.screenStream;
          activeCall.screenStream = null;
          activeCall.screenQuality = null;
          activeCall.screenTransport = null;
          if (peerNotTold) send(activeCall.conversationId, activeCall.peerId, "screen_stopped", null);
        } else {
          await share.start(activeCall.conversationId, ghostShareOptions(api.getSettings()));
          send(activeCall.conversationId, activeCall.peerId, "ghostshare_started", null);
        }
      } finally {
        if (call === activeCall) { activeCall.screenStarting = false; notify(); }
      }
    }

    async function watchGhostShare(activeCall) {
      if (!activeCall.ghostShareAvailable) return;
      try { await ensureGhostShare(activeCall).watch(activeCall.conversationId); }
      catch (error) {
        if (call === activeCall) api.onError("GhostShare could not establish its required relay connection. Check the media server's TURN configuration.");
      }
    }

    async function attachMicrophone(activeCall, session, { keepMuted = false } = {}) {
      if (call !== activeCall) { await session?.close(); return false; }
      const previous = activeCall.audioSession;
      const track = session?.stream.getAudioTracks()[0] || null;
      const transceiver = findAudioTransceiver(activeCall) || activeCall.pc.addTransceiver("audio", { direction: "recvonly" });
      activeCall.audioTransceiver = transceiver;
      session?.setMuted(true);
      try {
        await transceiver.sender.replaceTrack(track);
        if (call !== activeCall) { await session?.close(); return false; }
      } catch (error) { await session?.close(); throw error; }
      activeCall.audioSession = session;
      activeCall.localStream = session?.stream || null;
      activeCall.rawStream = session?.rawStream || null;
      activeCall.localAudioTrack = track;
      // The server media relay sends the engine's processed microphone.
      activeCall.serverMedia?.setInputStream(activeCall.localStream);
      activeCall.audioProcessing = session?.status || null;
      activeCall.manualMute = !track || (previous ? activeCall.manualMute : keepMuted);
      transceiver.direction = track ? "sendrecv" : "recvonly";
      if (track) activeCall.micUnavailableReason = "";
      else activeCall.micUnavailableReason ||= "Microphone unavailable. You can still listen.";
      syncOutgoingAudio(activeCall);
      await previous?.close();
      notify();
      return true;
    }

    function replaceMicrophone(settings, { unmute = false } = {}) {
      const activeCall = call;
      if (!activeCall) return Promise.resolve();
      const task = (activeCall.audioSettingsTask || Promise.resolve()).catch(() => {}).then(async () => {
        if (call !== activeCall) return;
        const microphone = await tryCaptureMicrophone(settings);
        if (call !== activeCall) { await microphone.session?.close(); return; }
        if (!microphone.session) {
          activeCall.micUnavailableReason = microphone.error;
          notify();
          throw new Error(microphone.error);
        }
        await attachMicrophone(activeCall, microphone.session, { keepMuted: unmute ? false : !!activeCall.manualMute });
      });
      activeCall.audioSettingsTask = task;
      return task;
    }

    // A call whose ICE never reaches "connected" would otherwise sit in
    // "connecting" forever with no audio and no way out except hanging up.
    const CONNECT_TIMEOUT_MS = 30_000;
    // TURN needs UDP (or TCP to its port), which an HTTP-only tunnel to the
    // GhostChat server cannot carry. A TURN call that has not connected after
    // this long switches to the server media relay.
    const RELAY_FALLBACK_MS = 8_000;
    // A relayed peer whose app crashed or lost power never sends hangup; the
    // server drops its socket, and the call ends if it is not back in time.
    const PEER_LOST_TIMEOUT_MS = 30_000;
    let connectTimer = null;
    let relayFallbackTimer = null;
    let peerLostTimer = null;
    function startConnectTimer(activeCall) {
      clearTimeout(connectTimer);
      connectTimer = setTimeout(() => {
        connectTimer = null;
        if (call === activeCall && !activeCall.everConnected && activeCall.pc?.connectionState !== "connected") {
          closeCall({ notifyPeer: true, message: "The call could not connect. Check your connection and call again." });
        }
      }, CONNECT_TIMEOUT_MS);
      clearTimeout(relayFallbackTimer);
      relayFallbackTimer = null;
      activeCall.connectStarted = true;
      if (activeCall.peerWantsServer && activeCall.serverRelayAvailable) activeCall.mediaPath = "server";
      if (activeCall.mediaPath === "server") startServerMedia(activeCall);
      else if (activeCall.serverRelayAvailable) {
        relayFallbackTimer = setTimeout(() => {
          relayFallbackTimer = null;
          if (call === activeCall && !activeCall.everConnected && activeCall.pc?.connectionState !== "connected") switchToServerRelay(activeCall);
        }, RELAY_FALLBACK_MS);
      }
    }

    function markConnected(activeCall) {
      activeCall.status = "connected";
      activeCall.everConnected = true;
      clearTimeout(ringTimer); ringTimer = null;
      clearTimeout(connectTimer); connectTimer = null;
      clearTimeout(relayFallbackTimer); relayFallbackTimer = null;
      clearTimeout(peerLostTimer); peerLostTimer = null;
    }

    // Both ends meet in the server's relay room for this conversation. The
    // side that switches on its own (timer or TURN failure) tells the other
    // side with a media_path signal, which switches at once: otherwise one
    // side could stay on TURN and audio would flow one way only. The peer
    // connection stays for signaling only.
    function switchToServerRelay(activeCall, { announce = true } = {}) {
      if (call !== activeCall || activeCall.mediaPath === "server" || !activeCall.serverRelayAvailable) return false;
      activeCall.mediaPath = "server";
      clearTimeout(relayFallbackTimer); relayFallbackTimer = null;
      if (announce) {
        try { send(activeCall.conversationId, activeCall.peerId, "media_path", { path: "server" }); } catch {}
      }
      startServerMedia(activeCall);
      notify();
      return true;
    }

    function startServerMedia(activeCall) {
      if (call !== activeCall || activeCall.serverMedia) return;
      const media = api.createServerMedia({
        conversationId: activeCall.conversationId,
        onPeerStream: (peerId, stream) => {
          if (call !== activeCall || activeCall.serverMedia !== media || peerId !== activeCall.peerId) return;
          activeCall.remoteAudioStream = stream;
          markConnected(activeCall);
          notify();
        },
        onPeerLeft: peerId => {
          if (call !== activeCall || activeCall.serverMedia !== media || peerId !== activeCall.peerId) return;
          // The peer's relay socket dropped; its client reconnects or hangs up.
          if (activeCall.everConnected) {
            activeCall.status = "reconnecting";
            clearTimeout(peerLostTimer);
            peerLostTimer = setTimeout(() => {
              peerLostTimer = null;
              if (call === activeCall && activeCall.serverMedia === media && activeCall.status === "reconnecting") {
                closeCall({ notifyPeer: true, message: "The call ended: the other side lost its connection." });
              }
            }, PEER_LOST_TIMEOUT_MS);
          }
          notify();
        },
        onPeerScreen: (peerId, stream) => {
          if (call !== activeCall || activeCall.serverMedia !== media || peerId !== activeCall.peerId) return;
          if (stream) setRemoteScreen(activeCall, stream);
          else clearRemoteScreen(activeCall);
        },
        onClosed: () => {
          if (call !== activeCall || activeCall.serverMedia !== media) return;
          closeCall({ notifyPeer: true, message: "The call relay connection to the GhostChat server was lost. Call again." });
        },
      });
      activeCall.serverMedia = media;
      media.connect().then(() => {
        if (call !== activeCall || activeCall.serverMedia !== media) { media.close(); return; }
        media.setInputStream(activeCall.localStream);
        syncOutgoingAudio(activeCall);
        notify();
      }).catch(error => {
        if (call !== activeCall || activeCall.serverMedia !== media) return;
        closeCall({ notifyPeer: true, message: error?.message || "The call relay on the GhostChat server could not be reached." });
      });
    }

    function closeCall({ notifyPeer = false, message = "" } = {}) {
      callGeneration++; starting = false;
      clearTimeout(ringTimer);
      ringTimer = null;
      clearTimeout(connectTimer);
      connectTimer = null;
      clearTimeout(relayFallbackTimer);
      relayFallbackTimer = null;
      clearTimeout(peerLostTimer);
      peerLostTimer = null;
      if (call && notifyPeer) {
        try { send(call.conversationId, call.peerId, "hangup", null); } catch {}
      }
      if (call) {
        void call.audioSession?.close();
        stopShareMonitor(call);
        stopConnectionMonitor(call);
        void call.ghostShare?.leave?.();
        try { call.serverMedia?.close(); } catch {}
        for (const stream of [call.localStream, call.rawStream, call.screenStream]) {
          stream?.getTracks().forEach(track => { track.onended = null; track.stop(); });
        }
        call.pc?.close();
      }
      call = null;
      incoming = null;
      notify();
      if (message) api.onError(message);
    }

    async function loadIceConfiguration() {
      const config = await api.getIceConfiguration();
      const servers = Array.isArray(config?.ice_servers) ? config.ice_servers : [];
      const turnServers = servers.flatMap(server => {
        if (!server || typeof server.username !== "string" || !server.username.trim() || typeof server.credential !== "string" || !server.credential) return [];
        const urls = (Array.isArray(server.urls) ? server.urls : [server.urls]).filter(url => typeof url === "string" && /^turns?:[^\s]+$/i.test(url));
        return urls.length ? [{ urls, username: server.username, credential: server.credential }] : [];
      });
      // Newer servers relay call media themselves (media_relay). That path
      // never reveals an address either, so it replaces a missing TURN.
      const serverRelayAvailable = !!config?.media_relay && typeof api.createServerMedia === "function";
      if (!config?.turn_available || !turnServers.length) {
        if (!serverRelayAvailable) {
          throw new Error("Calls require an authenticated TURN relay. No direct fallback is allowed because it could reveal your network address.");
        }
        // Signaling-only peer connection: no ICE servers and relay-only
        // policy gather no candidate at all, so no address can leak.
        return {
          mediaPath: "server",
          serverRelayAvailable,
          ghostShareAvailable: !!config?.group_voice_available,
          iceServers: [],
          iceTransportPolicy: "relay",
          bundlePolicy: "max-bundle",
          rtcpMuxPolicy: "require",
        };
      }
      return {
        mediaPath: "turn",
        serverRelayAvailable,
        ghostShareAvailable: !!config?.group_voice_available,
        iceServers: turnServers,
        iceTransportPolicy: "relay",
        bundlePolicy: "max-bundle",
        rtcpMuxPolicy: "require",
      };
    }

    function makePeer(activeCall, configuration) {
      const { mediaPath: _path, serverRelayAvailable: _relay, ghostShareAvailable: _share, ...rtcConfiguration } = configuration;
      const pc = new RTCPeerConnection(rtcConfiguration);
      activeCall.pc = pc;
      activeCall.relayCandidates = 0;
      // Relay-only calls cannot carry media without a TURN allocation. When
      // gathering finishes with none, fail loudly instead of leaving the call
      // stuck in "connecting" with no audio in either direction.
      pc.onicegatheringstatechange = () => {
        if (call !== activeCall || pc.iceGatheringState !== "complete" || activeCall.relayCandidates || activeCall.mediaPath === "server") return;
        if (switchToServerRelay(activeCall)) return;
        closeCall({ notifyPeer: true, message: "The TURN relay server did not respond, so no audio can be sent. Check that coturn is running and that port 3478 (UDP and TCP) and the relay port range are open on the server firewall." });
      };
      pc.onicecandidate = event => {
        if (call !== activeCall || !event.candidate) return;
        const isRelay = event.candidate.type === "relay" || /\btyp relay(?:\s|$)/.test(event.candidate.candidate || "");
        if (!isRelay) return;
        if (isRelay) activeCall.relayCandidates += 1;
        const candidate = event.candidate.toJSON();
        try {
          if (activeCall.signalReady) send(activeCall.conversationId, activeCall.peerId, "ice", candidate);
          else activeCall.localIceQueue.push(candidate);
        } catch (error) {
          closeCall({ message: explainMediaError(error) });
        }
      };
      pc.ontrack = event => {
        const track = event.track;
        if (track.kind === "audio") {
          // On the relay path the call plays the relayed audio only; a TURN
          // track that still connects would play it a second time.
          if (activeCall.mediaPath === "server") return;
          activeCall.remoteAudioStream ||= new MediaStream();
          if (!activeCall.remoteAudioStream.getTracks().some(item => item.id === track.id)) activeCall.remoteAudioStream.addTrack(track);
        } else if (track.kind === "video") {
          activeCall.remoteVideoStream ||= new MediaStream();
          if (!activeCall.remoteVideoStream.getTracks().some(item => item.id === track.id)) activeCall.remoteVideoStream.addTrack(track);
          setRemoteScreen(activeCall, activeCall.remoteVideoStream);
        }
        track.onended ||= notify;
        notify();
      };
      pc.onconnectionstatechange = () => {
        if (!call || call !== activeCall || activeCall.mediaPath === "server") return;
        if (pc.connectionState === "connected") {
          markConnected(activeCall);
          startConnectionMonitor(activeCall);
        }
        else if (pc.connectionState === "disconnected") activeCall.status = "reconnecting";
        else if (pc.connectionState === "failed") {
          if (!activeCall.everConnected && switchToServerRelay(activeCall)) return;
          closeCall({ message: "The required TURN relay connection failed. Check the relay service and network, then call again." });
          return;
        }
        notify();
      };
      pc.onnegotiationneeded = async () => {
        if (call !== activeCall || activeCall.initialNegotiation || activeCall.negotiating || pc.signalingState !== "stable") return;
        try {
          activeCall.negotiating = true;
          activeCall.signalReady = false;
          await pc.setLocalDescription(await pc.createOffer());
          send(activeCall.conversationId, activeCall.peerId, pc.localDescription.type, {
            type: pc.localDescription.type,
            sdp: pc.localDescription.sdp,
          });
          flushLocalIce(activeCall);
        } catch (error) {
          closeCall({ notifyPeer: true, message: explainMediaError(error) });
        } finally {
          activeCall.negotiating = false;
        }
      };
      return pc;
    }

    async function start(conversation) {
      if (call || incoming || starting) throw new Error("Finish the current call before starting another one.");
      if (!conversation?.id || !conversation?.peer_id) throw new Error("Open a private conversation before starting a call.");
      const settings = api.getSettings();
      const generation = ++callGeneration;
      starting = true;
      let configuration, microphone;
      try {
        configuration = await loadIceConfiguration(settings);
        if (generation !== callGeneration) return;
        microphone = await tryCaptureMicrophone(settings);
        if (generation !== callGeneration) { await microphone.session?.close(); return; }
      } finally { if (generation === callGeneration) starting = false; }
      const activeCall = {
        conversationId: conversation.id,
        peerId: conversation.peer_id,
        status: "calling",
        localStream: null,
        rawStream: null,
        localAudioTrack: null,
        micUnavailableReason: microphone.error,
        manualMute: !microphone.stream,
        remoteAudioStream: null,
        remoteVideoStream: null,
        screenStream: null,
        localIceQueue: [],
        pendingRemoteIce: [],
        signalReady: false,
        initialNegotiation: true,
        negotiating: false,
        polite: api.getUserId() > conversation.peer_id,
        ghostShareAvailable: !!configuration.ghostShareAvailable,
        mediaPath: configuration.mediaPath || "turn",
        serverRelayAvailable: !!configuration.serverRelayAvailable,
        serverMedia: null,
      };
      call = activeCall;
      try {
        const pc = makePeer(activeCall, configuration);
        activeCall.audioTransceiver = pc.addTransceiver("audio", { direction: microphone.stream ? "sendrecv" : "recvonly" });
        activeCall.videoTransceiver = pc.addTransceiver("video", { direction: "sendrecv" });
        await tuneScreenSender(activeCall.videoTransceiver.sender);
        if (!await attachMicrophone(activeCall, microphone.session, { keepMuted: !microphone.stream })) return;
        notify();
        const offer = await pc.createOffer();
        await pc.setLocalDescription(offer);
        if (call !== activeCall) return;
        send(activeCall.conversationId, activeCall.peerId, "offer", {
          type: pc.localDescription.type,
          sdp: pc.localDescription.sdp,
        });
        flushLocalIce(activeCall);
        ringTimer = setTimeout(() => {
          if (call === activeCall && activeCall.status === "calling") closeCall({ notifyPeer: true, message: "No answer yet. Try calling again later." });
        }, 45_000);
      } catch (error) {
        if (call === activeCall) closeCall();
        await microphone.session?.close();
        throw new Error(explainMediaError(error));
      }
    }

    async function accept() {
      if (!incoming || call || starting || incoming.accepting) return;
      let request = incoming;
      const settings = api.getSettings();
      const generation = ++callGeneration;
      starting = true;
      request.accepting = true;
      let configuration;
      try { configuration = await loadIceConfiguration(settings); }
      catch (error) { if (incoming) incoming.accepting = false; throw error; }
      finally { if (generation === callGeneration) starting = false; }
      if (generation !== callGeneration || !incoming || call) return;
      if (incoming !== request) {
        if (incoming.conversationId !== request.conversationId || incoming.peerId !== request.peerId) return;
        request = incoming;
        request.accepting = true;
      }
      const activeCall = {
        conversationId: request.conversationId,
        peerId: request.peerId,
        status: "connecting",
        localStream: null,
        rawStream: null,
        localAudioTrack: null,
        micUnavailableReason: "",
        manualMute: false,
        remoteAudioStream: null,
        remoteVideoStream: null,
        screenStream: null,
        localIceQueue: [],
        pendingRemoteIce: request.candidates,
        signalReady: false,
        initialNegotiation: true,
        negotiating: false,
        polite: api.getUserId() > request.peerId,
        ghostShareAvailable: !!configuration.ghostShareAvailable,
        mediaPath: configuration.mediaPath || "turn",
        serverRelayAvailable: !!configuration.serverRelayAvailable,
        serverMedia: null,
        // The caller moved to the server relay while this side was ringing.
        peerWantsServer: !!request.peerWantsServer,
      };
      call = activeCall;
      incoming = null;
      notify();
      try {
        const pc = makePeer(activeCall, configuration);
        await pc.setRemoteDescription(request.offer);
        activeCall.videoTransceiver = findVideoTransceiver(activeCall);
        if (activeCall.videoTransceiver) {
          activeCall.videoTransceiver.direction = "sendrecv";
          await tuneScreenSender(activeCall.videoTransceiver.sender);
        }
        for (const candidate of activeCall.pendingRemoteIce.splice(0)) await pc.addIceCandidate(candidate);
        const microphone = await tryCaptureMicrophone(settings);
        if (call !== activeCall) { await microphone.session?.close(); return; }
        activeCall.micUnavailableReason = microphone.error;
        activeCall.audioTransceiver = findAudioTransceiver(activeCall) || pc.addTransceiver("audio", { direction: "recvonly" });
        if (!await attachMicrophone(activeCall, microphone.session, { keepMuted: !microphone.stream })) return;
        const answer = await pc.createAnswer();
        await pc.setLocalDescription(answer);
        if (call !== activeCall) return;
        send(activeCall.conversationId, activeCall.peerId, "answer", {
          type: pc.localDescription.type,
          sdp: pc.localDescription.sdp,
        });
        activeCall.initialNegotiation = false;
        flushLocalIce(activeCall);
        startConnectTimer(activeCall);
      } catch (error) {
        if (call === activeCall) closeCall({ notifyPeer: true });
        throw new Error(explainMediaError(error));
      }
    }

    async function applyDescription(activeCall, description) {
      const pc = activeCall.pc;
      const readyForOffer = !activeCall.negotiating && (pc.signalingState === "stable" || activeCall.settingRemoteAnswer);
      const offerCollision = description.type === "offer" && !readyForOffer;
      activeCall.ignoreOffer = !activeCall.polite && offerCollision;
      if (activeCall.ignoreOffer) return;
      if (offerCollision && activeCall.polite) await pc.setLocalDescription({ type: "rollback" });
      const firstAnswer = description.type === "answer" && activeCall.initialNegotiation;
      activeCall.settingRemoteAnswer = description.type === "answer";
      await pc.setRemoteDescription(description);
      activeCall.settingRemoteAnswer = false;
      for (const candidate of activeCall.pendingRemoteIce.splice(0)) await pc.addIceCandidate(candidate);
      if (description.type === "answer") {
        activeCall.initialNegotiation = false;
        if (!activeCall.everConnected) activeCall.status = "connecting";
        clearTimeout(ringTimer);
        ringTimer = null;
        if (firstAnswer && !activeCall.everConnected) startConnectTimer(activeCall);
        notify();
      } else if (description.type === "offer") {
        const answer = await pc.createAnswer();
        activeCall.signalReady = false;
        await pc.setLocalDescription(answer);
        if (call !== activeCall) return;
        send(activeCall.conversationId, activeCall.peerId, "answer", {
          type: pc.localDescription.type,
          sdp: pc.localDescription.sdp,
        });
        activeCall.initialNegotiation = false;
        flushLocalIce(activeCall);
      }
    }

    // A busy device tells a second caller with an automatic hangup. In
    // invisible mode it stays silent, so that caller sees exactly what an
    // offline callee gives (ringing until its own timeout).
    function rejectBusy(conversationId, peerId) {
      if (api.isInvisible?.()) return;
      try { send(conversationId, peerId, "hangup", null); } catch {}
    }

    async function handleSignal(event) {
      if (!event?.conversation_id || !event?.from_user_id) return;
      const conversationId = event.conversation_id;
      const peerId = event.from_user_id;
      if (event.kind === "hangup") {
        if (call?.conversationId === conversationId && call.peerId === peerId) closeCall();
        else if (incoming?.conversationId === conversationId && incoming.peerId === peerId) { incoming = null; notify(); }
        return;
      }
      if (event.kind === "ice" && incoming?.conversationId === conversationId && incoming.peerId === peerId) {
        if (event.payload?.candidate) incoming.candidates.push(event.payload);
        return;
      }
      // A ringing callee never joins the relay room early (it would reveal
      // an invisible or unanswered callee); the request applies at accept.
      if (event.kind === "media_path" && event.payload?.path === "server" && incoming?.conversationId === conversationId && incoming.peerId === peerId) {
        incoming.peerWantsServer = true;
        return;
      }
      if (event.kind === "offer" && !call) {
        if (incoming && (incoming.conversationId !== conversationId || incoming.peerId !== peerId)) {
          rejectBusy(conversationId, peerId);
          return;
        }
        incoming = {
          conversationId,
          peerId,
          offer: event.payload,
          candidates: incoming?.candidates || [],
        };
        notify();
        api.onIncoming({ conversationId, peerId });
        return;
      }
      if (event.kind === "offer" && call && (call.conversationId !== conversationId || call.peerId !== peerId)) {
        rejectBusy(conversationId, peerId);
        return;
      }
      if (!call || call.conversationId !== conversationId || call.peerId !== peerId) return;
      try {
        if (event.kind === "screen_stopped") {
          call.peerGhostShareAnnounced = false;
          clearRemoteScreen(call);
          // The shared SFU room may still carry our own publication. A peer
          // stopping their screen must not disconnect our active publisher.
        } else if (event.kind === "ghostshare_started") {
          call.peerGhostShareAnnounced = true;
          await watchGhostShare(call);
        } else if (event.kind === "media_path" && event.payload?.path === "server") {
          // The peer fell back to the server relay; follow it at once, even
          // if TURN looks connected here, or audio would flow one way.
          if (call.connectStarted) switchToServerRelay(call, { announce: false });
          else call.peerWantsServer = true;
        } else if (event.kind === "call_note" && typeof event.payload?.text === "string") {
          call.callNote = event.payload.text.slice(0, 1200);
          notify();
        } else if (event.kind === "answer" && event.payload?.type === "answer") {
          await applyDescription(call, event.payload);
        } else if (event.kind === "offer" && event.payload?.type === "offer") {
          await applyDescription(call, event.payload);
        } else if (event.kind === "ice" && event.payload?.candidate) {
          if (call.pc.remoteDescription) await call.pc.addIceCandidate(event.payload);
          else call.pendingRemoteIce.push(event.payload);
        }
      } catch (error) {
        if (call && !call.ignoreOffer) closeCall({ notifyPeer: true, message: explainMediaError(error) });
      }
    }

    async function toggleScreenSharing() {
      if (!call || call.status === "calling") throw new Error("Wait until the call is connected before sharing your screen.");
      if (call.screenStarting) return;
      // A relayed call shares through the same relay (the SFU is the part
      // that was unreachable).
      if (call.mediaPath === "server") return toggleRelayScreenShare(call);
      if (call.ghostShareAvailable) {
        const activeCall = call;
        try { return await toggleGhostShare(activeCall); }
        catch (error) {
          if (error?.name === "NotAllowedError" || error?.name === "AbortError") return;
          const failedShare = activeCall.ghostShare;
          activeCall.ghostShare = null;
          try { await failedShare?.leave?.(); } catch {}
          throw new Error("GhostShare could not establish its required relay connection. Check the media server's TURN configuration; no direct fallback was started.");
        }
      }
      if (call.screenStream) {
        return stopDirectScreenShare(call);
      }
      const settings = api.getSettings();
      const activeCall = call;
      if (!navigator.mediaDevices?.getDisplayMedia) throw new Error("Screen capture is unavailable in this WebView. Update the Microsoft Edge WebView2 Runtime.");
      call.screenStarting = true;
      notify();
      try {
        const stream = await captureScreen(settings);
        if (!call || call !== activeCall) { stream.getTracks().forEach(track => track.stop()); return; }
        call.screenStream = stream;
        call.screenTransport = "direct";
        for (const track of stream.getTracks()) {
          track.onended = () => {
            if (call === activeCall && activeCall.screenStream === stream) {
              void stopDirectScreenShare(activeCall).catch(error => console.debug("Could not stop the ended screen share.", error));
            }
          };
          if (track.kind === "video") {
            let transceiver = findVideoTransceiver(call);
            if (!transceiver) {
              transceiver = call.pc.addTransceiver("video", { direction: "sendrecv" });
              call.videoTransceiver = transceiver;
              await tuneScreenSender(transceiver.sender, stream.__ghostShareProfiles?.[0]);
            }
            await transceiver.sender.replaceTrack(track);
          } else {
            call.pc.addTrack(track, stream);
          }
        }
        const videoTrack = stream.getVideoTracks()[0];
        if (videoTrack) startShareMonitor(call, videoTrack, stream.__ghostShareProfiles);
        notify();
      } catch (error) {
        if (error?.name === "NotAllowedError" || error?.name === "AbortError") return;
        throw new Error(explainMediaError(error));
      } finally {
        if (call === activeCall) { call.screenStarting = false; notify(); }
      }
    }

    // Screen sharing over the server media relay: low frame rate, the
    // transport encodes it (server-media-client.js). System audio is not
    // relayed.
    const RELAY_SCREEN_FPS = 5;
    async function toggleRelayScreenShare(activeCall) {
      if (activeCall.screenStream) return stopDirectScreenShare(activeCall);
      const media = activeCall.serverMedia;
      if (!media?.setScreenTrack || activeCall.status !== "connected") throw new Error("Wait until the call is connected before sharing your screen.");
      if (!navigator.mediaDevices?.getDisplayMedia) throw new Error("Screen capture is unavailable in this WebView. Update the Microsoft Edge WebView2 Runtime.");
      const settings = api.getSettings();
      const profile = shareProfiles(settings)[0];
      activeCall.screenStarting = true;
      notify();
      let stream = null;
      try {
        stream = await captureScreen(settings);
        if (call !== activeCall || activeCall.serverMedia !== media) { stream.getTracks().forEach(track => track.stop()); return; }
        for (const track of stream.getAudioTracks()) track.stop();
        const track = stream.getVideoTracks()[0];
        if (!track) throw new Error("The screen capture returned no picture.");
        media.setScreenTrack(track, { frameRate: Math.min(RELAY_SCREEN_FPS, profile.frameRate), bitrate: Math.min(profile.maxBitrate, 1_200_000) });
        activeCall.screenStream = stream;
        activeCall.screenTransport = "server";
        track.onended = () => {
          if (call === activeCall && activeCall.screenStream === stream) {
            void stopDirectScreenShare(activeCall).catch(error => console.debug("Could not stop the ended screen share.", error));
          }
        };
        notify();
      } catch (error) {
        if (activeCall.screenStream !== stream) stream?.getTracks().forEach(track => track.stop());
        if (error?.name === "NotAllowedError" || error?.name === "AbortError") return;
        throw new Error(explainMediaError(error));
      } finally {
        if (call === activeCall) { activeCall.screenStarting = false; notify(); }
      }
    }

    async function refreshScreenSharing() {
      if (!call || !call.screenStream || call.status === "calling") throw new Error("Start sharing your screen before refreshing it.");
      if (call.screenStarting) return;
      if (call.screenTransport === "server") {
        const activeCall = call;
        await stopDirectScreenShare(activeCall, { notifyPeer: false });
        if (call === activeCall) await toggleRelayScreenShare(activeCall);
        return;
      }
      if (call.ghostShare) {
        const activeCall = call;
        activeCall.screenStarting = true;
        notify();
        try {
          await activeCall.ghostShare.stop();
          if (call !== activeCall) return;
          await activeCall.ghostShare.start(activeCall.conversationId, ghostShareOptions(api.getSettings()));
        } finally {
          if (call === activeCall) { activeCall.screenStarting = false; notify(); }
        }
        return;
      }
      const activeCall = call;
      const previousStream = call.screenStream;
      const settings = api.getSettings();
      if (!navigator.mediaDevices?.getDisplayMedia) throw new Error("Screen capture is unavailable in this WebView. Update the Microsoft Edge WebView2 Runtime.");
      call.screenStarting = true;
      notify();
      try {
        const nextStream = await captureScreen(settings);
        if (!call || call !== activeCall) { nextStream.getTracks().forEach(track => track.stop()); return; }
        // Only a new capture replaces the share: a cancelled picker keeps the
        // current share and its quality monitor.
        stopShareMonitor(call);
        const nextTracks = [...nextStream.getTracks()];
        const usedTracks = new Set();
        for (const oldTrack of previousStream.getTracks()) {
          const sender = call.pc.getSenders().find(item => item.track === oldTrack);
          const replacement = nextTracks.find(track => track.kind === oldTrack.kind && !usedTracks.has(track));
          if (replacement) {
            usedTracks.add(replacement);
            if (oldTrack.kind === "video") {
              const transceiver = findVideoTransceiver(call);
              if (transceiver) await transceiver.sender.replaceTrack(replacement);
              else if (sender) await sender.replaceTrack(replacement);
              else call.pc.addTrack(replacement, nextStream);
            } else if (sender) await sender.replaceTrack(replacement);
            else call.pc.addTrack(replacement, nextStream);
          } else if (sender && oldTrack.kind === "video" && sender === findVideoTransceiver(call)?.sender) {
            await sender.replaceTrack(null);
          } else if (sender) call.pc.removeTrack(sender);
          oldTrack.onended = null;
          oldTrack.stop();
        }
        for (const track of nextTracks) {
          if (!usedTracks.has(track) && track.kind === "video") {
            const transceiver = findVideoTransceiver(call);
            if (transceiver) await transceiver.sender.replaceTrack(track);
            else call.pc.addTrack(track, nextStream);
          } else if (!usedTracks.has(track)) call.pc.addTrack(track, nextStream);
          track.onended = () => {
            if (call === activeCall && activeCall.screenStream === nextStream) {
              void stopDirectScreenShare(activeCall).catch(error => console.debug("Could not stop the ended refreshed screen share.", error));
            }
          };
        }
        call.screenStream = nextStream;
        call.screenTransport = "direct";
        const videoTrack = nextStream.getVideoTracks()[0];
        if (videoTrack) {
          await tuneScreenSender(findVideoTransceiver(call)?.sender, nextStream.__ghostShareProfiles?.[0]);
          startShareMonitor(call, videoTrack, nextStream.__ghostShareProfiles);
        }
      } catch (error) {
        if (error?.name !== "NotAllowedError" && error?.name !== "AbortError") throw new Error(explainMediaError(error));
      } finally {
        if (call === activeCall) { call.screenStarting = false; notify(); }
      }
    }

    function setCallNote(text) {
      if (!call || call.status === "idle") return;
      const value = String(text || "").slice(0, 1200);
      call.callNote = value;
      notify();
      send(call.conversationId, call.peerId, "call_note", { text: value });
    }

    async function toggleMicrophone() {
      if (!call) return;
      if (!call.localAudioTrack || call.audioSession?.status.state === "failed") {
        return replaceMicrophone(api.getSettings(), { unmute: true });
      }
      call.manualMute = !call.manualMute;
      syncOutgoingAudio(call);
      notify();
    }

    function setMicrophoneDevice(deviceId) {
      return replaceMicrophone({ ...api.getSettings(), microphoneId: deviceId || "" });
    }

    async function applyAudioSettings() {
      if (!call?.localAudioTrack) { notify(); return; }
      syncOutgoingAudio(call);
      await replaceMicrophone(api.getSettings());
    }

    function setPushToTalk(active) {
      if (!call?.localAudioTrack || !api.getSettings().pushToTalk) return;
      call.pushToTalkPressed = !!active;
      syncOutgoingAudio(call);
      notify();
    }

    function hangup() { closeCall({ notifyPeer: true }); }
    function decline() {
      // A decline tells the caller this device is online. In invisible
      // mode the caller must see what an offline callee gives: nothing,
      // until its own ring timeout.
      if (incoming && !api.isInvisible?.()) {
        try { send(incoming.conversationId, incoming.peerId, "hangup", null); } catch {}
      }
      incoming = null;
      notify();
    }

    function getState() { return snapshot(); }
    return { start, accept, decline, hangup, toggleMicrophone, setMicrophoneDevice, applyAudioSettings, toggleScreenSharing, refreshScreenSharing, setPushToTalk, setCallNote, handleSignal, getState };
  }

  window.GhostChatVoice = { create: createVoiceClient };
})();
