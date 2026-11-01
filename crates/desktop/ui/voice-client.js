(() => {
  function createVoiceClient(api) {
    let call = null;
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
        // The speech gate may briefly close an otherwise live microphone.
        // Do not present that as a manual mute in the call UI.
        micMuted: call ? (!call.localAudioTrack || !!call.manualMute || (!!api.getSettings().pushToTalk && !call.pushToTalkPressed)) : false,
        micAvailable: !!call?.localAudioTrack,
        micManuallyMuted: !!call?.manualMute,
        pushToTalkPressed: !!call?.pushToTalkPressed,
        micUnavailableReason: call?.micUnavailableReason || "",
        pushToTalk: !!api.getSettings().pushToTalk,
        speakerMuted: !!api.getSettings().speakerMuted,
        screenSharing: !!call?.screenStream,
        screenStarting: !!call?.screenStarting,
        screenQuality: call?.screenQuality || null,
        localAudioStream: call?.localStream || null,
        localScreenStream: call?.screenStream || null,
        remoteAudioStream: call?.remoteAudioStream || null,
        remoteVideoStream: call?.remoteVideoStream || null,
        audioProcessing: call?.audioProcessing || null,
        voiceGateOpen: call?.audioProcessor?.gateOpen !== false,
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

    function captureConstraints(settings, { fallback = false, includeDevice = true } = {}) {
      const supported = navigator.mediaDevices?.getSupportedConstraints?.() || {};
      const audio = {};
      const set = (key, value) => { if (fallback || supported[key] !== false) audio[key] = value; };
      // Ask for one mono, low-latency speech channel. The fallback keeps this
      // usable on older WebView2 runtimes which do not understand every hint.
      set("echoCancellation", fallback ? !!settings.echoCancellation : (settings.echoCancellation ? { ideal: "all" } : false));
      set("noiseSuppression", !!settings.noiseSuppression);
      set("autoGainControl", !!settings.autoGainControl);
      if (!fallback) {
        set("channelCount", { ideal: 1 });
        set("sampleRate", { ideal: 48_000 });
        set("latency", { ideal: 0.01 });
        if (settings.voiceIsolation && supported.voiceIsolation) set("voiceIsolation", true);
      }
      if (includeDevice && settings.microphoneId) audio.deviceId = { exact: settings.microphoneId };
      return audio;
    }

    async function captureMicrophone(settings) {
      if (!navigator.mediaDevices?.getUserMedia) throw new Error("Microphone capture is unavailable in this WebView. Update the Microsoft Edge WebView2 Runtime.");
      const attempts = [
        captureConstraints(settings),
        captureConstraints(settings, { fallback: true }),
        ...(settings.microphoneId ? [captureConstraints(settings, { fallback: true, includeDevice: false })] : []),
      ];
      let lastError;
      for (const audio of attempts) {
        try { return await navigator.mediaDevices.getUserMedia({ audio, video: false }); }
        catch (error) {
          lastError = error;
          if (error?.name === "NotAllowedError" || error?.name === "PermissionDeniedError") throw error;
        }
      }
      throw lastError;
    }

    function syncOutgoingAudio(activeCall) {
      const track = activeCall?.localAudioTrack;
      if (!track) return;
      const settings = api.getSettings();
      const gateOpen = activeCall.audioProcessor?.gateOpen !== false;
      track.enabled = !activeCall.manualMute && (!settings.pushToTalk || !!activeCall.pushToTalkPressed) && gateOpen;
    }

    function stopAudioProcessor(processor) {
      if (!processor) return;
      if (processor.frame) cancelAnimationFrame(processor.frame);
      try { processor.source?.disconnect(); processor.highPass?.disconnect(); processor.lowPass?.disconnect(); processor.compressor?.disconnect(); processor.analyser?.disconnect(); } catch {}
      try { processor.context?.close(); } catch {}
    }

    function createAudioProcessor(rawStream, activeCall) {
      const rawTrack = rawStream?.getAudioTracks?.()[0];
      if (!rawTrack) return { stream: null, processor: null, effective: null };
      const effective = rawTrack.getSettings?.() || {};
      const AudioContextType = window.AudioContext || window.webkitAudioContext;
      if (!AudioContextType) return { stream: rawStream, processor: null, effective };
      try {
        const context = new AudioContextType({ latencyHint: "interactive", sampleRate: 48_000 });
        const source = context.createMediaStreamSource(rawStream);
        const highPass = context.createBiquadFilter(); highPass.type = "highpass"; highPass.frequency.value = 75; highPass.Q.value = 0.7;
        const lowPass = context.createBiquadFilter(); lowPass.type = "lowpass"; lowPass.frequency.value = 9_000; lowPass.Q.value = 0.7;
        const compressor = context.createDynamicsCompressor();
        compressor.threshold.value = -30; compressor.knee.value = 20; compressor.ratio.value = 4; compressor.attack.value = 0.003; compressor.release.value = 0.18;
        const analyser = context.createAnalyser(); analyser.fftSize = 1024; analyser.smoothingTimeConstant = 0.75;
        const destination = context.createMediaStreamDestination();
        source.connect(highPass); highPass.connect(lowPass); lowPass.connect(compressor); compressor.connect(destination); compressor.connect(analyser);
        const processor = { context, source, highPass, lowPass, compressor, analyser, destination, gateOpen: true, frame: 0 };
        const samples = new Float32Array(analyser.fftSize);
        const updateGate = () => {
          if (call !== activeCall) return;
          analyser.getFloatTimeDomainData(samples);
          let energy = 0;
          for (const sample of samples) energy += sample * sample;
          const db = 20 * Math.log10(Math.max(Math.sqrt(energy / samples.length), 0.00001));
          const sensitivity = Math.max(1, Math.min(100, Number(api.getSettings().voiceFocusSensitivity) || 36));
          const threshold = -68 + sensitivity * 0.34;
          const now = performance.now();
          if (db >= threshold) processor.holdUntil = now + 420;
          const shouldOpen = !api.getSettings().voiceFocus || now < (processor.holdUntil || 0);
          if (processor.gateOpen !== shouldOpen) { processor.gateOpen = shouldOpen; syncOutgoingAudio(activeCall); notify(); }
          processor.frame = requestAnimationFrame(updateGate);
        };
        void context.resume().catch(() => {});
        updateGate();
        return { stream: destination.stream, processor, effective };
      } catch (error) {
        console.debug("Speech processing graph was unavailable; using the WebRTC capture track.", error);
        return { stream: rawStream, processor: null, effective };
      }
    }

    async function tryCaptureMicrophone(settings) {
      try {
        return { stream: await captureMicrophone(settings), error: "" };
      } catch (error) {
        if (error?.name === "NotAllowedError" || error?.name === "PermissionDeniedError") {
          return { stream: null, error: "Microphone permission was denied. You joined muted and can still listen; allow access to speak." };
        }
        if (error?.name === "NotFoundError" || error?.name === "OverconstrainedError") {
          return { stream: null, error: "No microphone was found. You joined muted and can still listen." };
        }
        if (error?.name === "NotReadableError") {
          return { stream: null, error: "The microphone is busy in another app. You joined muted and can still listen." };
        }
        return { stream: null, error: `${explainMediaError(error)} You joined muted and can still listen.` };
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
      const stream = activeCall?.remoteVideoStream;
      if (!stream) return;
      for (const track of stream.getVideoTracks()) {
        track.onended = null;
        track.onmute = null;
        try { stream.removeTrack(track); } catch {}
      }
      activeCall.remoteVideoStream = null;
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

    async function attachMicrophone(activeCall, rawStream, { keepMuted = false } = {}) {
      const previousStream = activeCall.localStream;
      const previousRawStream = activeCall.rawStream;
      const previousTrack = activeCall.localAudioTrack;
      stopAudioProcessor(activeCall.audioProcessor);
      const prepared = createAudioProcessor(rawStream, activeCall);
      const stream = prepared.stream;
      const track = stream?.getAudioTracks()[0] || null;
      const transceiver = findAudioTransceiver(activeCall) || activeCall.pc.addTransceiver("audio", { direction: track ? "sendrecv" : "recvonly" });
      activeCall.audioTransceiver = transceiver;
      if (transceiver.sender.track !== track) await transceiver.sender.replaceTrack(track);
      activeCall.localStream = stream || null;
      activeCall.rawStream = rawStream || null;
      activeCall.localAudioTrack = track;
      activeCall.audioProcessor = prepared.processor;
      activeCall.audioProcessing = rawStream ? {
        echoCancellation: prepared.effective?.echoCancellation === true,
        noiseSuppression: prepared.effective?.noiseSuppression === true,
        autoGainControl: prepared.effective?.autoGainControl === true,
        voiceIsolation: prepared.effective?.voiceIsolation === true,
        voiceFocus: !!api.getSettings().voiceFocus,
      } : null;
      if (track) {
        const settings = api.getSettings();
        activeCall.manualMute = !!keepMuted;
        syncOutgoingAudio(activeCall);
        transceiver.direction = "sendrecv";
        track.onended = () => {
          if (call !== activeCall || activeCall.localAudioTrack !== track) return;
          activeCall.localAudioTrack = null;
          activeCall.localStream = null;
          activeCall.manualMute = true;
          activeCall.micUnavailableReason = "The microphone disconnected. You are still in the call, muted.";
          void transceiver.sender.replaceTrack(null).then(() => { transceiver.direction = "recvonly"; notify(); }).catch(notify);
        };
        activeCall.micUnavailableReason = "";
      } else {
        activeCall.manualMute = true;
        activeCall.micUnavailableReason ||= "No microphone is available. You joined muted and can still listen.";
        transceiver.direction = "recvonly";
      }
      if (previousTrack && previousTrack !== track) previousTrack.onended = null;
      if (previousStream && previousStream !== stream) previousStream.getTracks().forEach(item => { item.onended = null; item.stop(); });
      if (previousRawStream && previousRawStream !== rawStream) previousRawStream.getTracks().forEach(item => { item.onended = null; item.stop(); });
      notify();
    }

    function closeCall({ notifyPeer = false, message = "" } = {}) {
      clearTimeout(ringTimer);
      ringTimer = null;
      if (call && notifyPeer) {
        try { send(call.conversationId, call.peerId, "hangup", null); } catch {}
      }
      if (call) {
        stopAudioProcessor(call.audioProcessor);
        stopShareMonitor(call);
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

    async function loadIceConfiguration(settings) {
      const config = await api.getIceConfiguration();
      const iceServers = Array.isArray(config?.ice_servers) ? config.ice_servers : [];
      if (!iceServers.length) throw new Error("The server did not provide an ICE server. Update the GhostChat server and try again.");
      if (settings.relayOnly && !config.turn_available) {
        throw new Error("Relay-only privacy needs a TURN server. Ask the server owner to configure coturn first.");
      }
      return {
        iceServers,
        iceTransportPolicy: settings.relayOnly ? "relay" : "all",
        bundlePolicy: "max-bundle",
        rtcpMuxPolicy: "require",
      };
    }

    function makePeer(activeCall, configuration) {
      const pc = new RTCPeerConnection(configuration);
      activeCall.pc = pc;
      pc.onicecandidate = event => {
        if (call !== activeCall || !event.candidate) return;
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
          activeCall.remoteAudioStream ||= new MediaStream();
          if (!activeCall.remoteAudioStream.getTracks().some(item => item.id === track.id)) activeCall.remoteAudioStream.addTrack(track);
        } else if (track.kind === "video") {
          activeCall.remoteVideoStream ||= new MediaStream();
          if (!activeCall.remoteVideoStream.getTracks().some(item => item.id === track.id)) activeCall.remoteVideoStream.addTrack(track);
          track.onended = () => clearRemoteScreen(activeCall);
          track.onmute = () => { if (track.muted) clearRemoteScreen(activeCall); else notify(); };
          track.onunmute = notify;
        }
        track.onended ||= notify;
        notify();
      };
      pc.onconnectionstatechange = () => {
        if (!call || call !== activeCall) return;
        if (pc.connectionState === "connected") { activeCall.status = "connected"; clearTimeout(ringTimer); ringTimer = null; }
        else if (pc.connectionState === "disconnected") activeCall.status = "reconnecting";
        else if (pc.connectionState === "failed") {
          closeCall({ message: "The call could not cross this network. A TURN server may be needed." });
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
      if (call || incoming) throw new Error("Finish the current call before starting another one.");
      if (!conversation?.id || !conversation?.peer_id) throw new Error("Open a private conversation before starting a call.");
      const settings = api.getSettings();
      const configuration = await loadIceConfiguration(settings);
      const microphone = await tryCaptureMicrophone(settings);
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
      };
      call = activeCall;
      try {
        const pc = makePeer(activeCall, configuration);
        activeCall.audioTransceiver = pc.addTransceiver("audio", { direction: microphone.stream ? "sendrecv" : "recvonly" });
        activeCall.videoTransceiver = pc.addTransceiver("video", { direction: "sendrecv" });
        await tuneScreenSender(activeCall.videoTransceiver.sender);
        await attachMicrophone(activeCall, microphone.stream, { keepMuted: !microphone.stream });
        notify();
        const offer = await pc.createOffer();
        await pc.setLocalDescription(offer);
        send(activeCall.conversationId, activeCall.peerId, "offer", {
          type: pc.localDescription.type,
          sdp: pc.localDescription.sdp,
        });
        flushLocalIce(activeCall);
        ringTimer = setTimeout(() => {
          if (call === activeCall && activeCall.status === "calling") closeCall({ notifyPeer: true, message: "No answer yet. Try calling again later." });
        }, 45_000);
      } catch (error) {
        closeCall();
        throw new Error(explainMediaError(error));
      }
    }

    async function accept() {
      if (!incoming || call) return;
      const request = incoming;
      const settings = api.getSettings();
      const configuration = await loadIceConfiguration(settings);
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
        activeCall.micUnavailableReason = microphone.error;
        activeCall.audioTransceiver = findAudioTransceiver(activeCall) || pc.addTransceiver("audio", { direction: "recvonly" });
        await attachMicrophone(activeCall, microphone.stream, { keepMuted: !microphone.stream });
        const answer = await pc.createAnswer();
        await pc.setLocalDescription(answer);
        send(activeCall.conversationId, activeCall.peerId, "answer", {
          type: pc.localDescription.type,
          sdp: pc.localDescription.sdp,
        });
        activeCall.initialNegotiation = false;
        flushLocalIce(activeCall);
      } catch (error) {
        closeCall({ notifyPeer: true });
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
      activeCall.settingRemoteAnswer = description.type === "answer";
      await pc.setRemoteDescription(description);
      activeCall.settingRemoteAnswer = false;
      for (const candidate of activeCall.pendingRemoteIce.splice(0)) await pc.addIceCandidate(candidate);
      if (description.type === "answer") {
        activeCall.initialNegotiation = false;
        activeCall.status = "connecting";
        clearTimeout(ringTimer);
        ringTimer = null;
        notify();
      } else if (description.type === "offer") {
        const answer = await pc.createAnswer();
        activeCall.signalReady = false;
        await pc.setLocalDescription(answer);
        send(activeCall.conversationId, activeCall.peerId, "answer", {
          type: pc.localDescription.type,
          sdp: pc.localDescription.sdp,
        });
        activeCall.initialNegotiation = false;
        flushLocalIce(activeCall);
      }
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
      if (event.kind === "offer" && !call) {
        if (incoming && (incoming.conversationId !== conversationId || incoming.peerId !== peerId)) {
          try { send(conversationId, peerId, "hangup", null); } catch {}
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
        try { send(conversationId, peerId, "hangup", null); } catch {}
        return;
      }
      if (!call || call.conversationId !== conversationId || call.peerId !== peerId) return;
      try {
        if (event.kind === "screen_stopped") {
          clearRemoteScreen(call);
        } else if (event.kind === "answer" && event.payload?.type === "answer") {
          await applyDescription(call, event.payload);
        } else if (event.kind === "offer" && event.payload?.type === "offer") {
          await applyDescription(call, event.payload);
        } else if (event.kind === "ice" && event.payload?.candidate) {
          if (call.pc.remoteDescription) await call.pc.addIceCandidate(event.payload);
          else call.pendingRemoteIce.push(event.payload);
        }
      } catch (error) {
        if (!call.ignoreOffer) closeCall({ notifyPeer: true, message: explainMediaError(error) });
      }
    }

    async function toggleScreenSharing() {
      if (!call || call.status === "calling") throw new Error("Wait until the call is connected before sharing your screen.");
      if (call.screenStarting) return;
      if (call.screenStream) {
        const stream = call.screenStream;
        call.screenStream = null;
        stopShareMonitor(call);
        call.screenQuality = null;
        for (const track of stream.getTracks()) {
          const sender = call.pc.getSenders().find(item => item.track === track);
          if (sender && track.kind === "video" && sender === findVideoTransceiver(call)?.sender) {
            await sender.replaceTrack(null);
          } else if (sender) {
            call.pc.removeTrack(sender);
          }
          track.onended = null;
          track.stop();
        }
        send(call.conversationId, call.peerId, "screen_stopped", null);
        notify();
        return;
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
        for (const track of stream.getTracks()) {
          track.onended = () => { if (call?.screenStream === stream) void toggleScreenSharing(); };
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

    async function refreshScreenSharing() {
      if (!call || !call.screenStream || call.status === "calling") throw new Error("Start sharing your screen before refreshing it.");
      if (call.screenStarting) return;
      const activeCall = call;
      const previousStream = call.screenStream;
      stopShareMonitor(call);
      const settings = api.getSettings();
      if (!navigator.mediaDevices?.getDisplayMedia) throw new Error("Screen capture is unavailable in this WebView. Update the Microsoft Edge WebView2 Runtime.");
      call.screenStarting = true;
      notify();
      try {
        const nextStream = await captureScreen(settings);
        if (!call || call !== activeCall) { nextStream.getTracks().forEach(track => track.stop()); return; }
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
          track.onended = () => { if (call?.screenStream === nextStream) void toggleScreenSharing(); };
        }
        call.screenStream = nextStream;
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

    async function toggleMicrophone() {
      if (!call) return;
      const settings = api.getSettings();
      if (!call.localAudioTrack) {
        const microphone = await tryCaptureMicrophone(settings);
        if (!microphone.stream) {
          call.micUnavailableReason = microphone.error || "No microphone is available. You can still listen to the call.";
          notify();
          throw new Error(call.micUnavailableReason);
        }
        await attachMicrophone(call, microphone.stream, { keepMuted: false });
        return;
      }
      if (settings.pushToTalk) {
        call.manualMute = !call.manualMute;
      } else {
        call.manualMute = !call.manualMute;
      }
      syncOutgoingAudio(call);
      notify();
    }

    async function setMicrophoneDevice(deviceId) {
      if (!call) return;
      const currentTrack = call.localAudioTrack;
      const keepMuted = currentTrack ? !!call.manualMute : true;
      const microphone = await tryCaptureMicrophone({ ...api.getSettings(), microphoneId: deviceId || "" });
      if (!microphone.stream) {
        call.micUnavailableReason = microphone.error || "The selected microphone is unavailable. You are still in the call, muted.";
        notify();
        throw new Error(call.micUnavailableReason);
      }
      await attachMicrophone(call, microphone.stream, { keepMuted });
    }

    // Media-processing constraints are selected when a capture track is made.
    // Replacing the active track makes saved settings take effect mid-call.
    async function applyAudioSettings() {
      if (!call?.localAudioTrack) { notify(); return; }
      const keepMuted = !!call.manualMute;
      const microphone = await tryCaptureMicrophone(api.getSettings());
      if (!microphone.stream) {
        call.micUnavailableReason = microphone.error || "The microphone could not apply the new audio settings.";
        notify();
        throw new Error(call.micUnavailableReason);
      }
      await attachMicrophone(call, microphone.stream, { keepMuted });
    }

    function setPushToTalk(active) {
      if (!call?.localAudioTrack || !api.getSettings().pushToTalk) return;
      call.pushToTalkPressed = !!active;
      syncOutgoingAudio(call);
      notify();
    }

    function hangup() { closeCall({ notifyPeer: true }); }
    function decline() {
      if (incoming) {
        try { send(incoming.conversationId, incoming.peerId, "hangup", null); } catch {}
      }
      incoming = null;
      notify();
    }

    function getState() { return snapshot(); }
    return { start, accept, decline, hangup, toggleMicrophone, setMicrophoneDevice, applyAudioSettings, toggleScreenSharing, refreshScreenSharing, setPushToTalk, handleSignal, getState };
  }

  window.GhostChatVoice = { create: createVoiceClient };
})();
