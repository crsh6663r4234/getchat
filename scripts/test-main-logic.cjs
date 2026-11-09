// Loads crates/desktop/ui/main.js in a vm with a permissive fake DOM so the
// app logic that does not depend on real layout can be exercised in Node.
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const assert = require('node:assert/strict');

const source = fs.readFileSync(path.join(__dirname, '../crates/desktop/ui/main.js'), 'utf8');

function harness({ storage = {}, credentials = {}, fetchHandler, hooks = {} } = {}) {
  const elements = new Map();
  const timers = [];
  const sockets = [];
  const fetches = [];
  const invokes = [];
  const store = new Map(Object.entries(storage));

  const STRING_PROPS = new Set(['textContent', 'value', 'title', 'innerHTML', 'className', 'placeholder', 'id', 'tagName', 'type', 'src', 'href']);
  const FALSE_PROPS = new Set(['checked', 'disabled', 'open', 'hidden', 'isConnected', 'paused', 'multiple']);
  function element(name) {
    const props = new Map();
    const children = new Map();
    const listeners = new Map();
    const classes = new Set();
    const attributes = new Map();
    const target = function fakeElement() {};
    let proxy;
    const builtins = {
      querySelector: sel => child(`${name} ${sel}`),
      querySelectorAll: () => [],
      getAttribute: key => (attributes.has(key) ? attributes.get(key) : null),
      setAttribute: (key, value) => { attributes.set(key, String(value)); },
      removeAttribute: key => { attributes.delete(key); },
      hasAttribute: key => attributes.has(key),
      closest: () => null,
      contains: () => false,
      matches: () => false,
      addEventListener: (type, fn) => { if (!listeners.has(type)) listeners.set(type, []); listeners.get(type).push(fn); },
      removeEventListener: () => {},
      append: () => {}, prepend: () => {}, replaceChildren: () => {}, remove: () => {}, insertBefore: () => {}, after: () => {}, before: () => {},
      showModal() { props.set('open', true); }, close() { props.set('open', false); }, focus: () => {}, blur: () => {}, click: () => {}, scrollTo: () => {}, scrollIntoView: () => {}, load: () => {},
      play: () => Promise.resolve(), pause: () => {},
      getBoundingClientRect: () => ({ top: 0, left: 0, right: 0, bottom: 0, width: 0, height: 0 }),
      classList: { add: (...c) => c.forEach(x => classes.add(x)), remove: (...c) => c.forEach(x => classes.delete(x)), toggle: (c, force) => { const on = force === undefined ? !classes.has(c) : !!force; if (on) classes.add(c); else classes.delete(c); return on; }, contains: c => classes.has(c) },
      dataset: {},
      style: { setProperty: () => {}, removeProperty: () => {} },
      options: [], selectedOptions: [], files: [], childNodes: [], children: [],
      __listeners: listeners,
    };
    function child(key) { if (!children.has(key)) children.set(key, element(key)); return children.get(key); }
    proxy = new Proxy(target, {
      get(_, prop) {
        if (props.has(prop)) return props.get(prop);
        if (prop === Symbol.toPrimitive) return () => '';
        if (prop === Symbol.iterator) return function* empty() {};
        if (prop === 'then') return undefined;
        if (prop === 'length') return 0;
        if (typeof prop === 'symbol') return undefined;
        if (Object.prototype.hasOwnProperty.call(builtins, prop)) return builtins[prop];
        if (STRING_PROPS.has(prop)) return '';
        if (FALSE_PROPS.has(prop)) return false;
        return child(prop);
      },
      set(_, prop, value) { props.set(prop, value); return true; },
      apply() { return element(`${name}()`); },
      construct() { return element(`new ${name}`); },
    });
    return proxy;
  }
  const $ = selector => { if (!elements.has(selector)) elements.set(selector, element(selector)); return elements.get(selector); };
  const docListeners = new Map();
  const document = {
    querySelector: $, querySelectorAll: () => [],
    createElement: tag => element(`<${tag}>`), createTextNode: text => ({ textContent: text }),
    body: element('body'), documentElement: element('html'),
    addEventListener: (type, fn) => { if (!docListeners.has(type)) docListeners.set(type, []); docListeners.get(type).push(fn); },
    hasFocus: () => false, visibilityState: 'visible', activeElement: null,
  };
  const localStorage = {
    getItem: key => (store.has(key) ? store.get(key) : null),
    setItem: (key, value) => { store.set(key, String(value)); },
    removeItem: key => { store.delete(key); },
  };
  class FakeWebSocket {
    constructor(url) { this.url = url; this.sent = []; this.listeners = {}; this.readyState = 0; sockets.push(this); }
    addEventListener(type, fn) { (this.listeners[type] ||= []).push(fn); }
    send(data) { this.sent.push(data); }
    close() { if (this.readyState === 3) return; this.readyState = 3; for (const fn of this.listeners.close || []) fn({}); }
    emit(type, event = {}) { if (type === 'open') this.readyState = 1; for (const fn of this.listeners[type] || []) fn(event); }
  }
  FakeWebSocket.OPEN = 1;
  async function invoke(command, args = {}) {
    invokes.push({ command, args });
    if (hooks[command]) { const result = await hooks[command](args); if (result !== undefined) return result; }
    if (command === 'api_base') return 'http://127.0.0.1:38241';
    if (command === 'check_for_update') return { configured: false, currentVersion: '0.0.0' };
    if (command === 'credential_get') return credentials[args.serverKey] || null;
    if (command === 'get_start_at_login') return false;
    return null;
  }
  async function fakeFetch(url, options = {}) {
    const headers = options.headers instanceof Headers ? options.headers : new Headers(options.headers || {});
    fetches.push({ url: String(url), method: options.method || 'GET', authorization: headers.get('Authorization'), body: options.body });
    const reply = fetchHandler ? await fetchHandler(String(url), options) : undefined;
    const { status = 200, body = [] } = reply || {};
    return { ok: status >= 200 && status < 300, status, json: async () => body };
  }
  const voiceOptions = {};
  const voiceStub = name => options => { voiceOptions[name] = options; return stubClient(); };
  const stubClient = () => new Proxy({ getState: () => ({ status: 'idle', participants: [] }) }, {
    get(target, prop) { return prop in target ? target[prop] : () => Promise.resolve(); },
  });
  const window = {
    __TAURI__: { core: { invoke }, event: { listen: () => Promise.resolve(() => {}) } },
    GhostChatVoice: { create: voiceStub('voice') }, GhostChatGroupVoice: { create: voiceStub('group') }, GhostChatWatchParty: { create: voiceStub('watch') },
    GhostChatRingtone: { setIncoming: () => {} },
    addEventListener: () => {}, innerWidth: 1280, innerHeight: 800,
  };
  const sandbox = {
    window, document, localStorage, WebSocket: FakeWebSocket, fetch: fakeFetch, Headers, URL, TextEncoder, crypto: globalThis.crypto,
    navigator: { clipboard: { writeText: async () => {} }, mediaDevices: { addEventListener: () => {}, enumerateDevices: async () => [] } },
    CSS: { escape: value => String(value) }, Blob, Option: function Option(text, value) { this.text = text; this.value = value; },
    setTimeout: (fn, ms = 0) => { const timer = { fn, ms, done: false }; timers.push(timer); return timer; },
    clearTimeout: timer => { if (timer) timer.done = true; },
    setInterval: () => 0, clearInterval: () => {}, requestAnimationFrame: () => 0, cancelAnimationFrame: () => {},
    queueMicrotask, console: { log() {}, warn() {}, error() {}, info() {} }, Promise, Date, Math, JSON, Number, String, Array, Object, Map, Set, Error,
  };
  sandbox.globalThis = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(source, sandbox, { filename: 'main.js' });
  // crypto.subtle.digest settles on the libuv thread pool, so waits are
  // bounded by wall time rather than a fixed number of event-loop turns.
  const flush = async (until = null, rounds = 400) => {
    if (!until) { for (let i = 0; i < rounds; i += 1) await new Promise(resolve => setImmediate(resolve)); return; }
    const deadline = Date.now() + 5000;
    while (!until()) {
      if (Date.now() > deadline) throw new Error('condition was not reached');
      await new Promise(resolve => setTimeout(resolve, 2));
    }
  };
  const runTimers = () => { for (const timer of timers.splice(0)) if (!timer.done) { timer.done = true; timer.fn(); } };
  return { $, store, sockets, fetches, invokes, timers, flush, runTimers, voiceOptions };
}

async function serverKey(base) {
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(base));
  return [...new Uint8Array(digest)].map(byte => byte.toString(16).padStart(2, '0')).join('');
}

const tests = [];
const test = (name, fn) => tests.push({ name, fn });

test('Turkish language reaches dialogs that branch on state.language', async () => {
  const app = harness({ storage: { 'ghostchat.language': 'tr', 'ghostchat.server': 'https://a.example' } });
  await app.flush(() => app.invokes.some(item => item.command === 'credential_get_pending_recovery'));
  void app.$('#revokeOtherSessions').onclick({ currentTarget: app.$('#revokeOtherSessions') });
  await app.flush(() => app.$('#appDecisionConfirm').textContent !== '');
  assert.equal(app.$('#appDecisionConfirm').textContent, 'Oturumları kapat', 'Turkish users must see the Turkish confirm label');
});

test('translate never returns Object.prototype members for user text', async () => {
  const app = harness({
    storage: { 'ghostchat.language': 'tr', 'ghostchat.server': 'https://a.example' },
    fetchHandler: url => (url.endsWith('/v1/data/export') ? { status: 400, body: { error: 'constructor' } } : undefined),
  });
  await app.flush(() => app.invokes.some(item => item.command === 'credential_get_pending_recovery'));
  await app.$('#exportAccount').onclick();
  await app.flush(() => app.$('#toast').textContent !== '');
  assert.equal(app.$('#toast').textContent, 'constructor', 'A message equal to a prototype key must render unchanged');
});

test('changing the server never sends the previous server token to the new one', async () => {
  const A = 'https://a.example';
  const B = 'https://b.example';
  const keyA = await serverKey(A);
  const keyB = await serverKey(B);
  let app;
  const hooks = {
    // Fire a reconnect timer that was already pending while the new server's
    // credential lookup is in flight.
    credential_get: async args => { if (args.serverKey === keyB) app.runTimers(); },
  };
  app = harness({
    storage: { 'ghostchat.server': A },
    credentials: { [keyA]: 'token-A' },
    hooks,
    fetchHandler: url => (url.endsWith('/v1/me') ? { body: { id: 'user-a', username: 'a', display_name: 'A' } } : { body: [] }),
  });
  await app.flush(() => app.sockets.length > 0);
  const first = app.sockets.at(-1);
  assert.ok(first && first.url.startsWith('wss://a.example'), 'boot connects to server A');
  first.emit('open');
  first.close(); // server A drops: a reconnect timer is now pending
  assert.ok(app.timers.some(timer => !timer.done), 'reconnect is scheduled');
  app.$('#serverAddressInput').value = B;
  await app.$('#changeServerButton').onclick({ currentTarget: app.$('#changeServerButton') });
  await app.flush(() => app.store.get('ghostchat.server') === B && app.invokes.some(item => item.command === 'credential_get_pending_recovery' && item.args.serverKey === keyB));
  app.runTimers();
  const bSockets = app.sockets.filter(item => item.url.startsWith('wss://b.example'));
  assert.ok(bSockets.length > 0, 'the stale reconnect timer opened a socket to server B, so the race was exercised');
  for (const socket of bSockets) {
    socket.emit('open');
    assert.ok(!socket.sent.some(data => data.includes('token-A')), 'server B must never receive server A token over WebSocket');
  }
  const leaked = app.fetches.filter(item => item.url.startsWith(B) && item.authorization === 'Bearer token-A');
  assert.deepEqual(leaked, [], 'server B must never receive server A token over HTTP');
});

async function openChatAndFailSend({ encrypted = false } = {}) {
  let offline = false;
  const conversation = { id: 'c1', kind: 'direct', peer_id: 'peer', display_name: 'Peer', username: 'peer' };
  const base = 'https://a.example';
  const hooks = encrypted ? {
    crypto_has_direct_group: () => true,
    crypto_direct_peer_fingerprint: () => 'fp-peer',
    crypto_encrypt_text: () => ({ payload: 'ciphertext' }),
  } : {};
  const app = harness({
    storage: { 'ghostchat.server': base, ...(encrypted ? { [`ghostchat.cryptoPeerTrusted.${base}.me.c1`]: 'fp-peer' } : {}) },
    credentials: { [await serverKey(base)]: 'token-A' },
    hooks,
    fetchHandler: (url, options) => {
      if (url.endsWith('/v1/me')) return { body: { id: 'me', username: 'me', display_name: 'Me' } };
      if (url.endsWith('/v1/conversations')) return { body: [conversation] };
      if (offline && options.method === 'POST' && url.includes('/messages')) throw new TypeError('Failed to fetch');
      return { body: [] };
    },
  });
  await app.flush(() => app.sockets.length > 0);
  // The incoming-call path is the simplest real entry point into openConversation().
  app.voiceOptions.voice.onIncoming({ conversationId: 'c1' });
  await app.flush(() => app.fetches.some(item => item.url.includes('/conversations/c1/messages?')));
  // loadMessages() has no observable end; let its remaining awaits settle.
  await app.flush();
  offline = true;
  app.$('#messageInput').value = 'hello there';
  const [submit] = app.$('#composer').__listeners.get('submit');
  await submit({ preventDefault() {} });
  assert.ok(app.fetches.some(item => item.method === 'POST' && item.url.endsWith('/conversations/c1/messages')), 'the send was attempted');
  app.goOnline = () => { offline = false; };
  app.submitComposer = submit;
  return app;
}

test('a failed send puts the typed text back into the composer', async () => {
  const app = await openChatAndFailSend();
  assert.equal(app.$('#messageInput').value, 'hello there', 'a failed send must not discard the typed message');
});

test('resending a restored draft unchanged reuses its client message id', async () => {
  const app = await openChatAndFailSend();
  const sentIds = () => app.fetches
    .filter(item => item.method === 'POST' && item.url.endsWith('/conversations/c1/messages'))
    .map(item => JSON.parse(item.body).client_message_id);
  const [failedId] = sentIds();
  app.goOnline();
  await app.submitComposer({ preventDefault() {} });
  assert.equal(sentIds()[1], failedId, 'the server can only deduplicate an ambiguous failure if the retry keeps its id');
  app.$('#messageInput').value = 'hello there';
  await app.submitComposer({ preventDefault() {} });
  assert.notEqual(sentIds()[2], failedId, 'a message sent after the retry succeeded is a new message');
});

test('editing a restored draft sends it with a new client message id', async () => {
  const app = await openChatAndFailSend();
  const sentIds = () => app.fetches
    .filter(item => item.method === 'POST' && item.url.endsWith('/conversations/c1/messages'))
    .map(item => JSON.parse(item.body).client_message_id);
  const [failedId] = sentIds();
  app.goOnline();
  app.$('#messageInput').value = 'hello there, again';
  await app.submitComposer({ preventDefault() {} });
  assert.notEqual(sentIds()[1], failedId, 'different text is a different message');
});

test('a failed encrypted send that is queued for retry is not restored', async () => {
  const app = await openChatAndFailSend({ encrypted: true });
  const pending = JSON.parse(app.store.get('ghostchat.cryptoMessagePending.https://a.example.me.c1') || 'null');
  assert.equal(pending?.payload, 'ciphertext', 'the ciphertext is queued for automatic retry');
  assert.equal(app.$('#messageInput').value, '', 'restoring a queued message would let it be sent twice');
});

test('the group owner removes a deleted account leaf and others tolerate it', async () => {
  const base = 'https://a.example';
  const group = { id: 'g1', kind: 'group', display_name: 'Group' };
  const processed = [];
  let removalPrepared = 0;
  const app = harness({
    storage: { 'ghostchat.server': base },
    credentials: { [await serverKey(base)]: 'token-A' },
    hooks: {
      crypto_has_direct_group: () => true,
      crypto_group_fingerprint: () => 'fp-group',
      crypto_get_event_cursor: () => 0,
      crypto_advance_event_cursor: args => args.sequence,
      crypto_process_event: args => { processed.push(args); return { kind: 'application', content: 'hi' }; },
      crypto_prepare_deleted_member_removal: args => {
        removalPrepared += 1;
        assert.deepEqual([...args.deletedMemberIds], ['carol']);
        return removalPrepared === 1
          ? { operationId: 'op-1', action: 'remove_deleted', memberIds: ['carol'], expectedMemberIds: ['bob', 'me'], commitPayload: 'commit', welcomePayload: '' }
          : false;
      },
      crypto_finalize_group_membership_update: () => 'fp-after',
    },
    fetchHandler: (url, options) => {
      if (url.endsWith('/v1/me')) return { body: { id: 'me', username: 'me', display_name: 'Me' } };
      if (url.endsWith('/v1/conversations')) return { body: [group] };
      if (url.endsWith('/conversations/g1/crypto-lock')) return { body: { locked: true, setupComplete: true, lockedBy: 'me', deletedMemberIds: ['carol'] } };
      if (url.endsWith('/conversations/g1/members')) return { body: [{ id: 'me', role: 'owner' }, { id: 'bob', role: 'member' }] };
      if (url.includes('/conversations/g1/crypto-events?after=0')) return { body: [{ sequence: 1, id: 'e1', sender_id: 'bob', recipient_id: null, payload: 'p', created_at: '' }] };
      if (options.method === 'POST' && url.includes('/crypto-membership/remove/')) return { status: 204, body: {} };
      return { body: [] };
    },
  });
  await app.flush(() => app.sockets.length > 0);
  app.voiceOptions.voice.onIncoming({ conversationId: 'g1' });
  await app.flush(() => app.fetches.some(item => item.method === 'POST' && item.url.includes('/crypto-membership/remove/')));
  assert.ok(processed.length > 0, 'the relay event was processed');
  assert.deepEqual([...processed[0].deletedMemberIds], ['carol'], 'recorded deleted accounts reach the roster check');
  const removal = app.fetches.find(item => item.method === 'POST' && item.url.includes('/crypto-membership/remove/'));
  assert.ok(removal.url.endsWith('/conversations/g1/crypto-membership/remove/carol'));
  assert.equal(JSON.parse(removal.body).operation_id, 'op-1');
  await app.flush(() => app.invokes.some(item => item.command === 'crypto_finalize_group_membership_update'));
});

test('pending encrypted rows are decoded before a relay commit is applied', async () => {
  const base = 'https://a.example';
  const group = { id: 'g1', kind: 'group', display_name: 'Group' };
  const order = [];
  const app = harness({
    storage: { 'ghostchat.server': base },
    credentials: { [await serverKey(base)]: 'token-A' },
    hooks: {
      crypto_has_direct_group: () => true,
      crypto_group_fingerprint: () => 'fp-group',
      crypto_get_event_cursor: () => 0,
      crypto_advance_event_cursor: args => args.sequence,
      crypto_get_cached_sent_text: () => null,
      crypto_decode_pending_history: args => { order.push(`history:${args.clientMessageId}`); return { kind: 'application', content: 'missed' }; },
      crypto_process_event: args => { order.push(args.eventSequence == null ? `row:${args.clientMessageId}` : `relay:${args.eventSequence}`); return { kind: 'commit' }; },
    },
    fetchHandler: url => {
      if (url.endsWith('/v1/me')) return { body: { id: 'me', username: 'me', display_name: 'Me' } };
      if (url.endsWith('/v1/conversations')) return { body: [group] };
      if (url.endsWith('/conversations/g1/crypto-lock')) return { body: { locked: true, setupComplete: true, lockedBy: 'bob', deletedMemberIds: [] } };
      if (url.endsWith('/conversations/g1/members')) return { body: [{ id: 'me', role: 'member' }, { id: 'bob', role: 'owner' }] };
      if (url.includes('/conversations/g1/crypto-events?after=0')) return { body: [{ sequence: 1, id: 'e1', sender_id: 'bob', recipient_id: 'me', payload: 'commit', created_at: '' }] };
      if (url.includes('/conversations/g1/messages?')) return { body: [{ sequence: 7, id: 'm7', conversation_id: 'g1', sender_id: 'bob', client_message_id: 'c7', content: 'e2e1:cipher', created_at: '' }] };
      return { body: [] };
    },
  });
  await app.flush(() => app.sockets.length > 0);
  app.voiceOptions.voice.onIncoming({ conversationId: 'g1' });
  await app.flush(() => order.includes('relay:1'));
  assert.ok(order.includes('history:c7'), 'the pending row was decoded');
  assert.ok(order.indexOf('history:c7') < order.indexOf('relay:1'), `the row must be decoded before the commit (order: ${order.join(', ')})`);
});

test('an unreachable update host does not block startup', async () => {
  const app = harness({
    storage: { 'ghostchat.server': 'https://a.example' },
    hooks: { check_for_update: () => { throw new Error('error sending request for url (https://github.com/...)'); } },
  });
  let reachedServer = false;
  try { await app.flush(() => app.invokes.some(item => item.command === 'credential_get_pending_recovery')); reachedServer = true; } catch {}
  assert.ok(reachedServer, 'boot must continue past the update gate when the check itself fails');
  assert.ok(app.$('#startupUpdateGate').classList.contains('hidden'), 'the startup gate is hidden');
  assert.equal(app.$('#toast').classList.contains('error'), true, 'the failed check is reported as a warning');
});

test('an available update still blocks startup and installs', async () => {
  const app = harness({
    storage: { 'ghostchat.server': 'https://a.example' },
    hooks: { check_for_update: () => ({ configured: true, available: true, version: '9.9.9' }) },
  });
  await app.flush(() => app.invokes.some(item => item.command === 'install_update'));
  // Asserting that boot does NOT continue needs a settle period, not a condition.
  await app.flush();
  assert.equal(app.invokes.find(item => item.command === 'install_update').args.expectedVersion, '9.9.9');
  assert.ok(!app.invokes.some(item => item.command === 'credential_get_pending_recovery'), 'boot stops while the mandatory update installs');
  assert.ok(!app.$('#startupUpdateGate').classList.contains('hidden'), 'the gate stays visible');
});

(async () => {
  let failed = 0;
  for (const { name, fn } of tests) {
    try { await fn(); console.log(`ok - ${name}`); }
    catch (error) { failed += 1; console.error(`not ok - ${name}\n  ${error.message}`); }
  }
  if (failed) { console.error(`${failed} of ${tests.length} main.js logic tests failed`); process.exit(1); }
  console.log(`${tests.length} main.js logic tests passed`);
})();
