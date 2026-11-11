import createRNNoise from './vendor/rnnoise-sync.js';

// RNNoise consumes 480 mono samples at 48 kHz. Web Audio supplies 128-frame
// quanta. A fixed 480-sample delay bridges them without dropping samples.
class GhostChatDenoiser extends AudioWorkletProcessor {
  constructor() {
    super();
    this.module = createRNNoise();
    this.state = this.module._rnnoise_create(0);
    this.ptr = this.module._malloc(480 * 4);
    this.input = new Float32Array(480);
    this.output = new Float32Array(1920);
    this.read = 0; this.write = 480; this.count = 480; this.used = 0;
    this.closed = false; this.frames = 0;
    if (sampleRate !== 48000 || !this.state || !this.ptr) throw new Error('RNNoise requires a working 48 kHz audio context.');
    this.port.onmessage = event => {
      if (event.data === 'close') {
        this.closed = true;
        this.module._rnnoise_destroy(this.state);
        this.module._free(this.ptr);
        this.port.close();
      }
    };
    this.port.postMessage({ type: 'ready' });
  }
  process(inputs, outputs) {
    if (this.closed) return false;
    const input = inputs[0]?.[0];
    const output = outputs[0]?.[0];
    if (!output) return true;
    for (let i = 0; i < output.length; i++) {
      this.input[this.used++] = (Number.isFinite(input?.[i]) ? input[i] : 0) * 32768;
      if (this.used === 480) {
        this.module.HEAPF32.set(this.input, this.ptr / 4);
        const speech = this.module._rnnoise_process_frame(this.state, this.ptr, this.ptr);
        const filtered = this.module.HEAPF32.subarray(this.ptr / 4, this.ptr / 4 + 480);
        for (let j = 0; j < 480; j++) {
          this.output[this.write] = Math.max(-1, Math.min(1, filtered[j] / 32768));
          this.write = (this.write + 1) % this.output.length;
        }
        this.count += 480; this.used = 0;
        // Diagnostics only; never use a speech score as a hard mute gate.
        if (++this.frames % 20 === 0) this.port.postMessage({ type: 'speech', probability: speech });
      }
      output[i] = this.count ? this.output[this.read] : 0;
      if (this.count) { this.read = (this.read + 1) % this.output.length; this.count--; }
    }
    return true;
  }
}
registerProcessor('ghostchat-denoiser', GhostChatDenoiser);
