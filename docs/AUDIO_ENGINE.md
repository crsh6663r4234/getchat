# Voice engine

One pipeline serves direct calls, group calls and the microphone test:
Selected microphone -> browser AEC/optional AGC -> noise filter AudioWorklet
(RNNoise, or DeepFilterNet) -> explicit mute/PTT -> RTCRtpSender, LiveKit
user-provided LocalAudioTrack, or the server media relay's Opus encoder.

**Settings > Voice > Noise filter** chooses Standard (RNNoise, the default) or
Advanced (DeepFilterNet, experimental). Both run locally as WASM; audio stays on
the device. DeepFilterNet removed more synthetic noise in a one-machine test but
used about three times the processing time and adds about 34 MB to the app
(see [archive/DEEPFILTER_COMPARISON.md](archive/DEEPFILTER_COMPARISON.md)). If it
stutters, choose Standard; there is no automatic switch to raw capture.

The old timer-driven level gate, EQ and unconditional compressor are removed.
Noise suppression off intentionally bypasses RNNoise. Browser noise suppression
is off to avoid double denoising. AEC still needs the browser's playback reference:
RNNoise is not an echo canceller. Actual capture settings drive diagnostics;
unknown capabilities are reported as unverified.

NVIDIA mode requires the selected device to identify as NVIDIA Broadcast.
GhostChat can verify the device, not the effects inside NVIDIA's application.
Device selection never silently falls back to another physical microphone.
Failed processing initialization joins direct calls muted, with an explanation;
group microphone activation fails visibly. No raw-audio fallback is automatic.
Calls stay relay-only (see [VOICE.md](VOICE.md)).

Microphone replacement retains the working session until replacement succeeds.
Sessions created after a hangup are immediately closed. Mute intent survives a
settings/device change. Group mute/PTT never asks LiveKit to capture another mic.

## Verification

`node scripts/test-audio-routing.cjs` checks direct/group routing, mute/PTT,
replacement failure, late capture cleanup and cancellation before an offer.
`node scripts/test-media-relay.cjs` covers relay policy and SFU failure.
`node scripts/test-audio-browser.cjs` requires Playwright 1.56.1 and Edge on
Windows (Chromium on Linux); set GHOSTCHAT_TEST_ENGINE to rnnoise or deepfilter. Uses fake capture, never a physical microphone.
It starts the bundled WASM AudioWorklet under the production CSP and checks
noise attenuation, mute, intentional bypass and track cleanup.
Set GHOSTCHAT_SPEECH_FIXTURE to a local speech WAV to check that quiet speech
passes. Local testing used Windows synthesized speech at quarter amplitude;
the RMS ratio was approximately 0.94. This is not an intelligibility score or
proof for all voices. Synthetic noise attenuation is not a promise about
keyboards, nearby voices or echo in a real room.

## Physical acceptance before calling acoustics verified

Use Settings > Voice > Test microphone. Record six seconds of speech plus a
pause; compare before/after with headphones. Both inputs already have capture
AEC/AGC: comparison isolates the additional noise filter. Clips remain in memory
and are discarded on close. Repeat quiet speech, fan, keyboard and simultaneous
speech on two devices, for direct and group calls. Test speaker echo with both
people talking, changed output devices, minimized app, and reconnects.
Real microphones, loudspeakers, two-party audio and production WebView2 packaging
still require physical validation; synthetic checks do not replace it.
