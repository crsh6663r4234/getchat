(() => {
  'use strict';
  const workletURL = new URL('audio-worklet.js', document.currentScript.src).href;
  const deepFilterURL = new URL('vendor/deepfilter/', document.currentScript.src).href;
  let deepFilterModule;
  function loadDeepFilter() {
    if (!deepFilterModule) deepFilterModule = (async () => {
      const response = await fetch(new URL('df_bg.wasm', deepFilterURL), { signal: AbortSignal.timeout(15000) });
      if (!response.ok) throw new Error('DeepFilterNet could not load. Choose the standard filter.');
      return WebAssembly.compile(await response.arrayBuffer());
    })().catch(error => { deepFilterModule = null; throw error; });
    return deepFilterModule;
  }
  async function open(settings, { onStatus = () => {} } = {}) {
    let rawStream, context, source, processor, muteGain, destination, closed = false;
    let muted = true;
    const status = { engine: 'starting', echoCancellation: null, noiseSuppression: false, autoGainControl: null, warning: '', state: 'starting' };
    const emit = () => { if (!closed) onStatus({ ...status }); };
    const session = {
      stream: null, rawStream: null, status,
      setMuted(value) {
        muted = !!value;
        const enabled = !muted && status.state === 'running';
        // Silence the graph as well as the outgoing track. This also protects
        // local meters/recorders on browsers with delayed track mute propagation.
        if (muteGain) muteGain.gain.value = enabled ? 1 : 0;
        session.stream?.getAudioTracks().forEach(track => { track.enabled = enabled; });
      },
      async close() {
        if (closed) return;
        closed = true;
        context?.removeEventListener('statechange', stateChanged);
        for (const stream of new Set([session.stream, rawStream])) stream?.getTracks().forEach(track => track.stop());
        try { processor?.port.postMessage('close'); source?.disconnect(); processor?.disconnect(); muteGain?.disconnect(); } catch {}
        if (context && context.state !== 'closed') await context.close().catch(() => {});
      },
    };
    function stateChanged() {
      if (closed || status.state === 'failed') return;
      status.state = context.state === 'running' ? 'running' : 'suspended';
      status.warning = status.state === 'running' ? status.captureWarning : 'Audio processing paused. Reconnect your microphone.';
      session.setMuted(muted); emit();
    }
    try {
      if (!navigator.mediaDevices?.getUserMedia) throw new Error('Microphone capture is unavailable in this app.');
      if (settings.nvidiaBroadcast) {
        const devices = await navigator.mediaDevices.enumerateDevices();
        if (!devices.some(device => device.kind === 'audioinput' && device.deviceId === settings.microphoneId && /nvidia.*broadcast|broadcast.*nvidia/i.test(device.label))) {
          throw new Error('Select the NVIDIA Broadcast microphone or turn off NVIDIA Broadcast mode.');
        }
      }
      const external = !!settings.nvidiaBroadcast;
      rawStream = await navigator.mediaDevices.getUserMedia({ video: false, audio: {
        ...(settings.microphoneId ? { deviceId: { exact: settings.microphoneId } } : {}),
        channelCount: { ideal: 1 }, sampleRate: { ideal: 48000 },
        echoCancellation: !external && !!settings.echoCancellation,
        noiseSuppression: false,
        autoGainControl: !external && !!settings.autoGainControl,
      } });
      session.rawStream = rawStream;
      const raw = rawStream.getAudioTracks()[0];
      if (!raw) throw new Error('The microphone did not provide an audio track.');
      const actual = raw.getSettings();
      status.echoCancellation = typeof actual.echoCancellation === 'boolean' ? actual.echoCancellation : null;
      status.autoGainControl = typeof actual.autoGainControl === 'boolean' ? actual.autoGainControl : null;
      const warnings = [];
      if (!external && settings.echoCancellation && actual.echoCancellation !== true) warnings.push('Echo cancellation could not be verified on this device.');
      if (!external && settings.autoGainControl && actual.autoGainControl !== true) warnings.push('Automatic level could not be verified on this device.');
      if (!external && settings.noiseSuppression) {
        const AudioContextType = window.AudioContext || window.webkitAudioContext;
        if (!AudioContextType) throw new Error('Noise suppression is unavailable. Update WebView2 or turn noise suppression off.');
        context = new AudioContextType({ sampleRate: 48000, latencyHint: 'interactive' });
        await context.resume();
        if (context.state !== 'running') throw new Error('Audio processing could not start. Try the microphone again.');
        const advanced = settings.noiseEngine === 'deepfilter';
        const wasmModule = advanced ? await loadDeepFilter() : null;
        await context.audioWorklet.addModule(advanced ? new URL('worklet.js', deepFilterURL).href : workletURL);
        processor = new AudioWorkletNode(context, advanced ? 'deepfilternet-processor' : 'ghostchat-denoiser', {
          numberOfInputs: 1, numberOfOutputs: 1, outputChannelCount: [1], channelCount: 1, channelCountMode: 'explicit',
          ...(advanced ? { processorOptions: { wasmModule, attenuationLimit: 35 } } : {}),
        });
        await new Promise((resolve, reject) => {
          const timer = setTimeout(() => reject(new Error('Noise suppression did not start.')), 5000);
          processor.port.onmessage = event => {
            if (event.data?.type === 'ready') { clearTimeout(timer); resolve(); }
            if (event.data?.type === 'failed') { clearTimeout(timer); reject(new Error('DeepFilterNet could not start. Choose the standard filter.')); }
          };
          processor.onprocessorerror = () => { clearTimeout(timer); reject(new Error('Noise suppression could not load.')); };
        });
        destination = context.createMediaStreamDestination();
        source = context.createMediaStreamSource(rawStream);
        muteGain = context.createGain(); muteGain.gain.value = 0;
        source.connect(processor); processor.connect(muteGain); muteGain.connect(destination);
        session.stream = destination.stream;
        status.engine = advanced ? 'deepfilter' : 'rnnoise'; status.noiseSuppression = true;
        processor.onprocessorerror = () => {
          status.state = 'failed'; status.warning = 'Noise suppression stopped. Reconnect your microphone.';
          session.setMuted(true); emit();
        };
        processor.port.onmessage = event => {
          if (event.data?.type === 'speech') status.speechProbability = event.data.probability;
          if (event.data?.type === 'frames') status.processedFrames = event.data.count;
        };
        context.addEventListener('statechange', stateChanged);
      } else {
        session.stream = rawStream;
        status.engine = external ? 'nvidia' : 'capture';
      }
      status.captureWarning = warnings.join(' ');
      status.warning = status.captureWarning;
      status.state = 'running';
      session.setMuted(true); // Callers must explicitly authorize transmission.
      raw.addEventListener('ended', () => {
        if (closed) return;
        status.state = 'failed'; status.warning = 'The microphone disconnected. Reconnect it to speak.';
        session.setMuted(true); emit();
      });
      emit();
      return session;
    } catch (error) {
      await session.close();
      throw error;
    }
  }
  window.GhostChatAudio = { open };
})();
