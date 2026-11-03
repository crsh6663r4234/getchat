const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const source = fs.readFileSync('crates/desktop/ui/voice-client.js', 'utf8');
const body = source.slice(source.indexOf('    function createAudioProcessor('), source.indexOf('    async function tryCaptureMicrophone('));
let speech = false;
let tick;
let gate;
let analyser;
let compressor;
const node = () => ({ connections: [], connect(other) { this.connections.push(other); }, disconnect() {} });
class Context {
  state = 'running'; sampleRate = 48000; currentTime = 0;
  createMediaStreamSource() { return node(); }
  createBiquadFilter() { return { ...node(), frequency: {}, Q: {} }; }
  createDynamicsCompressor() { return compressor = { ...node(), threshold: {}, knee: {}, ratio: {}, attack: {}, release: {} }; }
  createGain() { return gate = { ...node(), gain: { value: 1, setTargetAtTime(value) { this.value = value; } } }; }
  createAnalyser() { return analyser = { ...node(), frequencyBinCount: 1024, getFloatTimeDomainData(data) { data.fill(speech ? 0.15 : 0); }, getFloatFrequencyData(data) { data.fill(speech ? -22 : -100); } }; }
  createMediaStreamDestination() { return { stream: {} }; }
  resume() { return Promise.resolve(); }
}
const activeCall = {};
const sandbox = { window: { AudioContext: Context }, call: activeCall, api: { getSettings: () => ({ voiceFocus: true, voiceFocusSensitivity: 36 }) }, performance: { now: () => 1000 }, setInterval(fn) { tick = fn; return 1; }, notify() {}, console };
vm.createContext(sandbox);
vm.runInContext(body + '\n globalThis.makeProcessor = createAudioProcessor;', sandbox);
sandbox.makeProcessor({ getAudioTracks: () => [{ getSettings: () => ({}) }] }, activeCall);
assert(compressor.connections.includes(analyser), 'Detection must observe audio before attenuation');
assert(!gate.connections.includes(analyser), 'A closed gate must not silence its own detector');
assert.equal(gate.gain.value, 0.08, 'Quiet passages retain a fail-safe audio floor');
speech = true; tick();
assert.equal(gate.gain.value, 1, 'New speech reopens a previously closed gate');
console.log('Voice gate regression checks passed: pre-gate detection, quiet floor, speech recovery.');
