const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');

const window = { navigator: { hardwareConcurrency: 8 } };
vm.runInNewContext(fs.readFileSync('crates/desktop/ui/audio-engine.js', 'utf8'), {
  window,
  navigator: window.navigator,
  document: { currentScript: { src: 'https://ghostchat.invalid/audio-engine.js' } },
  URL,
  AbortSignal,
  fetch: async () => { throw new Error('not used by processing-plan tests'); },
});

const choose = window.GhostChatAudio.selectProcessingPlan;
const standard = choose({ noiseEngine: 'rnnoise', noiseSuppression: true, echoCancellation: true, autoGainControl: true }, 2);
assert.equal(standard.advanced, false, 'standard filtering stays standard');
assert.equal(standard.browserEchoCancellation, true, 'echo cancellation stays enabled for relayed calls');
assert.equal(standard.browserAutoGain, true, 'automatic level stays enabled when requested');
assert.equal(choose({ noiseEngine: 'deepfilter', noiseSuppression: true }, 8).advanced, true, 'experimental filtering remains available on capable devices');
assert.equal(choose({ noiseEngine: 'deepfilter', noiseSuppression: true }, 2).useStandardForContinuity, true, 'low-power clients use the stable filter instead of risking speech dropouts');
console.log('Audio processing plan checks passed: stable default and low-power experimental fallback.');
