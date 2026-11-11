# Bundled RNNoise

Source: https://github.com/jitsi/rnnoise-wasm
Version: 0.2.1, commit cb529a59a8478fe604e57986fc96afdaecfa6fb7.
Artifact: dist/rnnoise-sync.js, copied unchanged (WASM embedded).
SHA-256: 05a553f523d59502d133a6d05dbf1878137c9e7bcff06edf5561f7001b62f95f.
RNNoise submodule: https://github.com/xiph/rnnoise/tree/372f7b4b76cde4ca1ec4605353dd17898a99de38
Licenses: LICENSE-RNNoise-Jitsi.txt (Apache 2.0), LICENSE-RNNoise.txt (BSD).

Runs locally in audio-worklet.js with 480-sample mono frames at 48 kHz.
No runtime downloads or audio uploads. Normalized samples are converted to
RNNoise's signed-16-bit amplitude scale and back. A 480-sample FIFO bridges
the Web Audio render quantum, in addition to the algorithm's own delay.
Speech probability is diagnostic only; it never mutes the speaker.
