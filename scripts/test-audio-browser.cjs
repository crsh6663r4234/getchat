// Real Chromium AudioWorklet + bundled WASM + production CSP, synthetic input only.
const { chromium } = require('playwright');
const http = require('node:http');
const fs = require('node:fs/promises');
const path = require('node:path');
const assert = require('node:assert/strict');
const ui = path.resolve(__dirname, '../crates/desktop/ui');
const config = require('../crates/desktop/tauri.conf.json');
(async () => {
  const server = http.createServer(async (req, res) => {
    try {
      const pathname = new URL(req.url, 'http://localhost').pathname;
      if (pathname === '/speech.wav' && process.env.GHOSTCHAT_SPEECH_FIXTURE) {
        res.setHeader('Content-Type', 'audio/wav'); return res.end(await fs.readFile(process.env.GHOSTCHAT_SPEECH_FIXTURE));
      }
      res.setHeader('Content-Security-Policy', config.app.security.csp);
      if (pathname === '/') {
        res.setHeader('Content-Type', 'text/html');
        return res.end('<html><body><script src="audio-engine.js"></script></body></html>');
      }
      const file = path.resolve(ui, '.' + pathname);
      if (!file.startsWith(ui + path.sep)) { res.writeHead(404).end(); return; }
      res.setHeader('Content-Type', 'text/javascript');
      res.end(await fs.readFile(file));
    } catch { res.writeHead(404).end(); }
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  let browser;
  try {
    browser = await chromium.launch({ channel: process.platform === 'win32' ? 'msedge' : 'chromium', headless: true, args: ['--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream', '--autoplay-policy=no-user-gesture-required'] });
    const page = await browser.newPage();
    page.on('pageerror', error => console.error('Browser error:', error.message));
    await page.goto('http://127.0.0.1:' + server.address().port);
    const result = await page.evaluate(async hasSpeech => {
      const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
      const ensure = (condition, message) => { if (!condition) throw new Error(message); };
      const settings = { noiseSuppression: true, echoCancellation: true, autoGainControl: true };
      // Test the actual capture implementation (no physical microphone).
      const captured = await GhostChatAudio.open(settings);
      ensure(captured.status.engine === 'rnnoise' && captured.status.state === 'running', 'Real worklet must start under production CSP');
      ensure(captured.stream.getAudioTracks()[0] !== captured.rawStream.getAudioTracks()[0], 'RNNoise output, not raw track, must be returned');
      ensure(!captured.stream.getAudioTracks()[0].enabled, 'New sessions start muted');
      captured.setMuted(false);
      ensure(captured.stream.getAudioTracks()[0].enabled, 'Explicit unmute enables output');
      await captured.close(); await captured.close();
      ensure(captured.rawStream.getAudioTracks()[0].readyState === 'ended' && captured.stream.getAudioTracks()[0].readyState === 'ended', 'Close releases both streams');

      // A repeatable fan-like noise fixture travels through the actual audio graph.
      const context = new AudioContext({ sampleRate: 48000 }); await context.resume();
      const buffer = context.createBuffer(1, 48000 * 2, 48000);
      let seed = 42;
      const samples = buffer.getChannelData(0);
      for (let i = 0; i < samples.length; i++) { seed = (1664525 * seed + 1013904223) >>> 0; samples[i] = ((seed / 4294967296) * 2 - 1) * 0.07; }
      const noise = context.createBufferSource(); noise.buffer = buffer; noise.loop = true;
      const destination = context.createMediaStreamDestination(); noise.connect(destination); noise.start();
      const originalCapture = navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices);
      navigator.mediaDevices.getUserMedia = async () => destination.stream.clone();
      const filtered = await GhostChatAudio.open(settings); filtered.setMuted(false);
      const before = context.createAnalyser(), after = context.createAnalyser(); before.fftSize = after.fftSize = 2048;
      context.createMediaStreamSource(filtered.rawStream).connect(before);
      context.createMediaStreamSource(filtered.stream).connect(after);
      const rms = analyser => { const data = new Float32Array(2048); analyser.getFloatTimeDomainData(data); return Math.sqrt(data.reduce((sum, x) => { ensure(Number.isFinite(x), 'Finite output'); return sum + x*x; }, 0) / data.length); };
      await wait(1600);
      let input = 0, output = 0;
      for (let i = 0; i < 10; i++) { await wait(40); input += rms(before); output += rms(after); }
      const reductionDb = 20 * Math.log10(input / Math.max(output, 1e-12));
      ensure(input > 0.1, 'Fixture must have non-silent input');
      ensure(reductionDb > 6, 'Steady noise should be attenuated by at least 6 dB, got ' + reductionDb);
      ensure(filtered.status.speechProbability !== undefined, 'Worklet processed frames');
      filtered.setMuted(true); await wait(100);
      ensure(rms(after) < 0.00001, 'Mute must silence outgoing audio');
      await filtered.close();
      const bypass = await GhostChatAudio.open({ ...settings, noiseSuppression: false }); bypass.setMuted(false);
      ensure(bypass.status.engine === 'capture', 'Explicit off mode uses native capture');
      const plain = context.createAnalyser(); plain.fftSize = 2048; context.createMediaStreamSource(bypass.stream).connect(plain); await wait(100);
      ensure(rms(plain) > 0.02, 'Turning noise suppression off must pass audio');
      await bypass.close();
      let speechRatio = null;
      if (hasSpeech) {
        noise.stop();
        const voice = context.createBufferSource();
        voice.buffer = await context.decodeAudioData(await (await fetch('/speech.wav')).arrayBuffer());
        const quiet = context.createGain(); quiet.gain.value = 0.25;
        voice.connect(quiet); quiet.connect(destination);
        const speech = await GhostChatAudio.open(settings); speech.setMuted(false);
        const inMeter = context.createAnalyser(), outMeter = context.createAnalyser();
        inMeter.fftSize = outMeter.fftSize = 2048;
        context.createMediaStreamSource(speech.rawStream).connect(inMeter);
        context.createMediaStreamSource(speech.stream).connect(outMeter);
        voice.start();
        let inputSpeech = 0, outputSpeech = 0;
        for (let i = 0; i < 80; i++) { await wait(40); inputSpeech += rms(inMeter); outputSpeech += rms(outMeter); }
        speechRatio = outputSpeech / Math.max(inputSpeech, 1e-10);
        ensure(inputSpeech > 0.01 && speechRatio > 0.1, 'Quiet speech must not be erased: ' + speechRatio);
        voice.stop(); await speech.close();
      }
      navigator.mediaDevices.getUserMedia = originalCapture;
      await context.close(); destination.stream.getTracks().forEach(track => track.stop());
      await GhostChatAudio.open({ ...settings, nvidiaBroadcast: true, microphoneId: 'not-a-broadcast-microphone' }).then(() => { throw new Error('Invalid Broadcast device accepted'); }, error => ensure(/NVIDIA/.test(error.message), 'External mode validates the selected device'));
      return { reductionDb: Math.round(reductionDb * 10) / 10, speechRatio, realWorklet: true, mute: true, bypass: true, cleanup: true };
    }, !!process.env.GHOSTCHAT_SPEECH_FIXTURE);
    assert(result.realWorklet);
    console.log('Chromium audio checks passed:', JSON.stringify(result));
  } finally { await browser?.close(); await new Promise(resolve => server.close(resolve)); }
})().catch(error => { console.error(error); process.exitCode = 1; });
