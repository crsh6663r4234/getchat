# DeepFilterNet3 browser engine

Source: https://github.com/lofcz/DeepFilterNet (fork of Rikorose/DeepFilterNet).
Distribution: @lofcz/deepfilternet-web 0.1.0, npm gitHead
773cf0ca65da1f82bc63ee5d8fd15d9314190231.
Package integrity:
sha512-uKf9IsCd5HUkwLRqZTDmA8gPCsDZTAKUa+9u9GWrtJWRGxVIVj43neNcpGzPkK3DEle1L1tZ+df8GXNJJxEfGA==

df_bg.wasm is copied unchanged, including the embedded DeepFilterNet3 weights.
SHA-256: 596034b83a03b546f33daa14b791bcdb59655cb3a07053ac39ddb0ba42c52fa4
License: MIT OR Apache-2.0, both notices included. Runtime never fetches a CDN.

worklet.js is adapted from the same package:
- UTF-8 decoder compatibility for AudioWorkletGlobalScope.
- Explicit ready/error handshake; initialization failure emits silence, never raw audio.
- Fixed startup padding for 480/128 sample framing.
- Explicit model release on close and processed-frame diagnostics.
- 35 dB attenuation limit supplied by GhostChat.

This is a third-party WASM port, not an official upstream browser SDK.
It remains opt-in and experimental. RNNoise remains the default.
