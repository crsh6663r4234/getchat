const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const rooms = [];
class Room {
  events = new Map();
  localParticipant = { isScreenShareEnabled: false };
  constructor(options) { this.options = options; rooms.push(this); }
  on(event, handler) { this.events.set(event, handler); return this; }
  async connect(url, token, options) { this.connectOptions = options; }
  async disconnect() { this.events.get('Disconnected')?.(); }
  emit(event, ...args) { this.events.get(event)?.(...args); }
}
const events = Object.fromEntries(['TrackSubscribed', 'TrackUnsubscribed', 'LocalTrackPublished', 'LocalTrackUnpublished', 'Disconnected'].map(key => [key, key]));
let current = null;
let timer;
const window = { LivekitClient: { Room, RoomEvent: events, Track: { Kind: { Video: 'video' }, Source: { ScreenShare: 'screen' } } } };
vm.runInNewContext(fs.readFileSync('crates/desktop/ui/ghost-share-client.js', 'utf8'), {
  window, MediaStream: class { constructor(tracks) { this.tracks = tracks; } },
  setTimeout(fn) { timer = fn; return 1; }, clearTimeout() {},
});
const track = () => ({ kind: 'video', source: 'screen', mediaStreamTrack: { muted: false } });
(async () => {
  const share = window.GhostChatShare.create({ getToken: async () => ({ url: 'wss://fixture', token: 'fixture' }), onRemoteStream: stream => { current = stream; } });
  await share.watch('conversation');
  assert.equal(rooms[0].connectOptions.rtcConfig.iceTransportPolicy, 'relay', 'SFU viewers must use relay-only RTC configuration');
  const firstRoom = rooms[0]; const first = track();
  firstRoom.emit('TrackSubscribed', first);
  const firstStream = current;
  first.mediaStreamTrack.muted = true; first.mediaStreamTrack.onmute(); timer();
  assert.equal(current, firstStream, 'A transport mute must retain a recoverable share');
  first.mediaStreamTrack.muted = false; first.mediaStreamTrack.onunmute();
  assert.equal(current, firstStream, 'Unmute restores the existing stream');
  const replacement = track(); firstRoom.emit('TrackSubscribed', replacement);
  const replacementStream = current;
  firstRoom.emit('TrackUnsubscribed', first);
  assert.equal(current, replacementStream, 'Late old unsubscribe cannot remove the new share');
  await share.leave(); await share.watch('next-conversation');
  assert.equal(rooms[1].connectOptions.rtcConfig.iceTransportPolicy, 'relay', 'A replacement room must retain relay policy');
  rooms[1].emit('TrackSubscribed', track()); const nextStream = current;
  firstRoom.emit('TrackSubscribed', track());
  assert.equal(current, nextStream, 'Events from a disconnected room must be ignored');
  await share.leave(); assert.equal(current, null);
  console.log('GhostShare recovery, replacement, stale-room and leave checks passed.');
})().catch(error => { console.error(error); process.exitCode = 1; });
