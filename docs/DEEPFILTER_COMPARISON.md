# DeepFilterNet candidate comparison — 2026-11-11

DeepFilterNet is an opt-in experimental alternative, not a stack of filters.
Browser AEC and optional AGC precede either RNNoise or DeepFilterNet. Neither
denoiser alone provides acoustic echo cancellation or speaker identity isolation.
No RTX GPU, cloud credentials or per-call inference service is required.

## Local evidence

Edge, actual shipped WASM AudioWorklets; Windows synthesized speech at 0.25 gain.
Six seconds mixed with seeded white noise (0.015 amplitude) and a 120 Hz hum
(0.005 amplitude). Offline rendering, with alignment before SI-SDR measurement:

| Signal | SI-SDR | Measured delay | Offline processing / audio duration |
| --- | ---: | ---: | ---: |
| Noisy input | 7.68 dB | 0 ms | — |
| RNNoise | 8.89 dB | 30 ms | 0.087 |
| DeepFilterNet, 35 dB limit | 17.44 dB | 40 ms | 0.265 |

These are one-machine synthetic results, not perceptual scores, CPU percentages,
end-to-end call latency or guarantees for real rooms. DeepFilterNet preserved
this reference waveform better but used roughly three times the processing time.
Other voices, transient noise and low-power machines require separate testing.
The default therefore stays RNNoise. The bundled model adds about 34 MB before
installer compression. Audio stays on the device.

## Reproduce

Install Playwright 1.56.1 in an isolated tooling folder, set NODE_PATH to its
node_modules and GHOSTCHAT_SPEECH_FIXTURE to a local speech WAV.
Run node scripts/benchmark-denoisers.cjs. No physical microphone is opened.

Run scripts/test-audio-browser.cjs with GHOSTCHAT_TEST_ENGINE set to rnnoise,
then deepfilter. This checks the production CSP, real worklet startup, synthetic
noise suppression, optional quiet speech, mute, bypass and cleanup. DeepFilterNet
also tests a broken model: initialization must fail and release capture rather
than leaking unfiltered audio. Both engines run in the release workflow.

## User validation

Settings > Voice > Noise filter: compare Standard and Advanced using the existing
six-second before/after recorder. Validate direct and group calls, quiet speech,
keyboard sounds and simultaneous speakers. If the advanced option stutters,
choose Standard; no automatic switch to raw capture is made.
