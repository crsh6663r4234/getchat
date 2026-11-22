// AudioWorklet processors for calls relayed through the GhostChat server
// (server-media-client.js). Loaded with audioWorklet.addModule from 'self'
// because the desktop CSP does not allow blob: scripts.
//
// ghostchat-relay-capture: collects microphone samples into 20 ms frames
// (960 samples at 48 kHz) and posts each frame to the main thread.
// ghostchat-relay-playback: plays decoded frames of one peer through a small
// jitter buffer, so network bursts do not cause clicks or growing delay.
(() => {
  const FRAME = 960;
  // Adaptive jitter buffer, counted in 20 ms frames. Playback starts once the
  // target is buffered (60 ms at first). Each underrun raises the target by
  // 40 ms (up to 600 ms); every 250 calm frames (5 s) lower it by 20 ms. The
  // queue may hold the target plus 200 ms of slack; older audio is dropped,
  // which keeps a relayed call close to real time.
  const MIN_TARGET_FRAMES = 3;
  const MAX_TARGET_FRAMES = 30;
  const TARGET_STEP_FRAMES = 2;
  const CALM_FRAMES = 250;
  const SLACK_FRAMES = 10;

  class RelayCapture extends AudioWorkletProcessor {
    constructor() {
      super();
      this.frame = new Float32Array(FRAME);
      this.filled = 0;
    }

    process(inputs) {
      const channel = inputs[0]?.[0];
      if (!channel) return true;
      let offset = 0;
      while (offset < channel.length) {
        const take = Math.min(channel.length - offset, FRAME - this.filled);
        this.frame.set(channel.subarray(offset, offset + take), this.filled);
        this.filled += take;
        offset += take;
        if (this.filled === FRAME) {
          const ready = this.frame;
          this.frame = new Float32Array(FRAME);
          this.filled = 0;
          this.port.postMessage(ready, [ready.buffer]);
        }
      }
      return true;
    }
  }

  class RelayPlayback extends AudioWorkletProcessor {
    constructor() {
      super();
      this.queue = [];
      this.offset = 0;
      this.buffered = 0;
      this.playing = false;
      this.targetFrames = MIN_TARGET_FRAMES;
      this.calmFrames = 0;
      this.underruns = 0;
      this.port.onmessage = event => {
        if (event.data?.type === "stats") {
          this.port.postMessage({
            type: "stats",
            underruns: this.underruns,
            targetMs: this.targetFrames * 20,
            queueMs: (this.buffered - this.offset) / 48,
          });
          return;
        }
        const samples = event.data;
        if (!(samples instanceof Float32Array) || !samples.length) return;
        this.queue.push(samples);
        this.buffered += samples.length;
        const maxSamples = (this.targetFrames + SLACK_FRAMES) * FRAME;
        while (this.buffered - this.offset > maxSamples && this.queue.length > 1) {
          this.buffered -= this.queue.shift().length;
          this.offset = 0;
        }
      };
    }

    process(_inputs, outputs) {
      const output = outputs[0]?.[0];
      if (!output) return true;
      output.fill(0);
      const available = this.buffered - this.offset;
      if (!this.playing && available < this.targetFrames * FRAME) return true;
      this.playing = true;
      let written = 0;
      while (written < output.length && this.queue.length) {
        const head = this.queue[0];
        const take = Math.min(output.length - written, head.length - this.offset);
        output.set(head.subarray(this.offset, this.offset + take), written);
        written += take;
        this.offset += take;
        if (this.offset === head.length) {
          this.queue.shift();
          this.buffered -= head.length;
          this.offset = 0;
        }
      }
      // An underrun grows the cushion and waits for it instead of stuttering.
      if (written < output.length) {
        this.underruns++;
        this.targetFrames = Math.min(MAX_TARGET_FRAMES, this.targetFrames + TARGET_STEP_FRAMES);
        this.calmFrames = 0;
        this.playing = false;
      } else {
        this.calmFrames += output.length / FRAME;
        if (this.calmFrames >= CALM_FRAMES) {
          this.targetFrames = Math.max(MIN_TARGET_FRAMES, this.targetFrames - 1);
          this.calmFrames = 0;
        }
      }
      for (let index = 1; index < outputs[0].length; index++) outputs[0][index].set(output);
      return true;
    }
  }

  registerProcessor("ghostchat-relay-capture", RelayCapture);
  registerProcessor("ghostchat-relay-playback", RelayPlayback);
})();
