// Offline, deterministic A/B benchmark of the actual shipped AudioWorklet code.
// Requires Playwright and GHOSTCHAT_SPEECH_FIXTURE (a local, non-private WAV).
const { chromium } = require('playwright');
const fs = require('node:fs/promises');
const http = require('node:http');
const path = require('node:path');
const ui = path.resolve(__dirname, '../crates/desktop/ui');
(async () => {
  if (!process.env.GHOSTCHAT_SPEECH_FIXTURE) throw new Error('Set GHOSTCHAT_SPEECH_FIXTURE to a speech WAV.');
  const server = http.createServer(async (req, res) => {
    try {
      const pathname = new URL(req.url, 'http://localhost').pathname;
      if (pathname === '/') return res.end('<html><body>Audio benchmark</body></html>');
      const file = pathname === '/speech.wav' ? process.env.GHOSTCHAT_SPEECH_FIXTURE : path.resolve(ui, '.' + pathname);
      if (pathname !== '/speech.wav' && !file.startsWith(ui + path.sep)) return res.writeHead(404).end();
      res.setHeader('Content-Type', file.endsWith('.wasm') ? 'application/wasm' : file.endsWith('.wav') ? 'audio/wav' : 'text/javascript');
      res.end(await fs.readFile(file));
    } catch { res.writeHead(404).end(); }
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  let browser;
  try {
    browser = await chromium.launch({ channel: process.platform === 'win32' ? 'msedge' : 'chromium', headless: true });
    const page = await browser.newPage();
    await page.goto('http://127.0.0.1:' + server.address().port);
    const report = await page.evaluate(async () => {
      const sr = 48000;
      const decoder = new OfflineAudioContext(1, sr, sr);
      const decoded = await decoder.decodeAudioData(await (await fetch('/speech.wav')).arrayBuffer());
      const length = Math.min(decoded.length, sr * 6);
      const clean = Float32Array.from(decoded.getChannelData(0).subarray(0, length), x => x * 0.25);
      const noisy = new Float32Array(length);
      let seed = 42;
      for (let i = 0; i < length; i++) {
        seed = (1664525 * seed + 1013904223) >>> 0;
        noisy[i] = clean[i] + (seed / 4294967296 * 2 - 1) * 0.015 + Math.sin(i * 2 * Math.PI * 120 / sr) * 0.005;
      }
      const wasmModule = await WebAssembly.compile(await (await fetch('/vendor/deepfilter/df_bg.wasm')).arrayBuffer());
      function score(output) {
        let best = { correlation: -Infinity, lag: 0 };
        function at(lag, stride) {
          let xx = 0, yy = 0, xy = 0;
          for (let i = 24000; i < length - 4800; i += stride) {
            const x = clean[i], y = output[i + lag] || 0;
            xx += x*x; yy += y*y; xy += x*y;
          }
          return { correlation: xy / Math.sqrt(Math.max(xx * yy, 1e-30)), xx, yy, xy, lag };
        }
        for (let lag = 0; lag <= 4800; lag += 16) { const s = at(lag, 16); if (s.correlation > best.correlation) best = s; }
        const coarse = best.lag;
        for (let lag = Math.max(0, coarse - 16); lag <= Math.min(4800, coarse + 16); lag++) { const s = at(lag, 4); if (s.correlation > best.correlation) best = s; }
        const s = at(best.lag, 1);
        const projected = s.xy*s.xy / Math.max(s.xx, 1e-30);
        return { delayMs: best.lag / 48, siSdrDb: 10*Math.log10(projected / Math.max(s.yy - projected, 1e-30)), correlation: s.correlation };
      }
      const results = { fixtureSeconds: length / sr, noisy: score(noisy), engines: {} };
      for (const engine of ['rnnoise', 'deepfilter']) {
        const ctx = new OfflineAudioContext(1, length, sr);
        await ctx.audioWorklet.addModule(engine === 'rnnoise' ? '/audio-worklet.js' : '/vendor/deepfilter/worklet.js');
        const node = new AudioWorkletNode(ctx, engine === 'rnnoise' ? 'ghostchat-denoiser' : 'deepfilternet-processor', {
          numberOfInputs: 1, numberOfOutputs: 1, outputChannelCount: [1],
          processorOptions: { wasmModule, attenuationLimit: 35 },
        });
        let failure; node.port.onmessage = e => { if (e.data?.type === 'failed') failure = e.data.message; };
        node.onprocessorerror = () => { failure = 'processorerror'; };
        const buffer = ctx.createBuffer(1, length, sr); buffer.copyToChannel(noisy, 0);
        const source = ctx.createBufferSource(); source.buffer = buffer; source.connect(node); node.connect(ctx.destination); source.start();
        const started = performance.now();
        const output = (await ctx.startRendering()).getChannelData(0);
        const renderMs = performance.now() - started;
        if (failure) throw new Error(engine + ': ' + failure);
        if (!output.every(Number.isFinite)) throw new Error(engine + ': nonfinite output');
        results.engines[engine] = { ...score(output), renderMs, realtimeFactor: renderMs / (length / 48) };
        node.port.postMessage('close'); node.disconnect(); node.port.close();
      }
      return results;
    });
    console.log(JSON.stringify(report, null, 2));
  } finally { await browser?.close(); await new Promise(resolve => server.close(resolve)); }
})().catch(error => { console.error(error); process.exitCode = 1; });
