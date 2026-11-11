// Loads crates/desktop/ui/main.js in a vm with a permissive fake DOM so the
// app logic that does not depend on real layout can be exercised in Node.
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const assert = require('node:assert/strict');

const source = fs.readFileSync(path.join(__dirname, '../crates/desktop/ui/main.js'), 'utf8');

function harness({ storage = {}, credentials = {}, fetchHandler, hooks = {}, selectAll = null, dateNow = null } = {}) {
  const elements = new Map();
  const timers = [];
  const sockets = [];
  const fetches = [];
  const invokes = [];
  const created = [];
  const observerDisconnects = { count: 0 };
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
      // Rendered message rows are not tracked, so a lookup for one finds none.
      querySelector: sel => (String(sel).startsWith('[data-message-id') ? null : child(`${name} ${sel}`)),
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
    querySelector: $, querySelectorAll: selector => (selectAll ? selectAll(String(selector)) || [] : []),
    createElement: tag => { const node = element(`<${tag}>`); created.push(node); return node; }, createTextNode: text => ({ textContent: text }),
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
    const { status = 200, body = [], html = false } = reply || {};
    return { ok: status >= 200 && status < 300, status, json: async () => { if (html) throw new SyntaxError('Unexpected token <'); return body; } };
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
    IntersectionObserver: class { observe() {} unobserve() {} disconnect() { observerDisconnects.count += 1; } },
  };
  const sandbox = {
    window, document, localStorage, WebSocket: FakeWebSocket, fetch: fakeFetch, Headers, URL, TextEncoder, crypto: globalThis.crypto,
    navigator: { clipboard: { writeText: async () => {} }, mediaDevices: { addEventListener: () => {}, enumerateDevices: async () => [] } },
    CSS: { escape: value => String(value) }, Blob, Option: function Option(text, value) { this.text = text; this.value = value; },
    setTimeout: (fn, ms = 0) => { const timer = { fn, ms, done: false }; timers.push(timer); return timer; },
    clearTimeout: timer => { if (timer) timer.done = true; },
    setInterval: () => 0, clearInterval: () => {}, requestAnimationFrame: () => 0, cancelAnimationFrame: () => {},
    queueMicrotask, console: { log() {}, warn() {}, error() {}, info() {} }, Promise, Date: dateNow ? class SkewedDate extends Date { static now() { return dateNow(); } } : Date, Math, JSON, Number, String, Array, Object, Map, Set, Error,
  };
  sandbox.IntersectionObserver = window.IntersectionObserver;
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
  return { $, store, sockets, fetches, invokes, created, observerDisconnects, timers, flush, runTimers, voiceOptions };
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

test('a display name equal to a UI string is shown untranslated', async () => {
  const base = 'https://a.example';
  const app = harness({
    storage: { 'ghostchat.language': 'tr', 'ghostchat.server': base },
    credentials: { [await serverKey(base)]: 'token-A' },
    fetchHandler: url => {
      if (url.endsWith('/v1/me')) return { body: { id: 'me', username: 'me', display_name: 'Me' } };
      if (url.endsWith('/v1/conversations')) return { body: [{ id: 'c1', kind: 'direct', peer_id: 'p', display_name: 'Owner', username: 'owner', last_message: 'Admin' }] };
      return { body: [] };
    },
  });
  await app.flush(() => app.created.some(node => node.textContent === 'Owner' || node.textContent === 'Kurucu'));
  const texts = app.created.map(node => node.textContent);
  assert.ok(texts.includes('Owner'), 'the peer named "Owner" keeps that name');
  assert.ok(!texts.includes('Kurucu'), 'a user name is never run through the dictionary');
  assert.ok(texts.includes('Admin') && !texts.includes('Yönetici'), 'message text is never translated either');
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

test('changing the server drops the previous server\'s presence, reply target and attachment views', async () => {
  const A = 'https://a.example';
  const B = 'https://b.example';
  const conversation = { id: 'c1', kind: 'direct', peer_id: 'peer', display_name: 'Peer', username: 'peer' };
  const history = [
    { sequence: 1, id: 'm1', conversation_id: 'c1', sender_id: 'peer', sender_name: 'Peer', client_message_id: 'x1', content: 'earlier', created_at: '2026-01-01T00:00:00Z' },
    { sequence: 2, id: 'm2', conversation_id: 'c1', sender_id: 'peer', sender_name: 'Peer', client_message_id: 'x2', content: 'file:0b3c9a52-6f1e-4c55-9d43-3f6d2f6c1a10', created_at: '2026-01-01T00:00:00Z' },
  ];
  const app = harness({
    storage: { 'ghostchat.server': A },
    credentials: { [await serverKey(A)]: 'token-A' },
    fetchHandler: url => {
      if (url.startsWith(B)) return url.endsWith('/v1/me/presence') ? { status: 500, body: {} } : { status: 401, body: {} };
      if (url.endsWith('/v1/me')) return { body: { id: 'me', username: 'me', display_name: 'Me' } };
      if (url.endsWith('/v1/me/presence')) return { body: [{ user_id: 'me', mode: 'invisible' }] };
      if (url.endsWith('/v1/conversations')) return { body: [conversation] };
      if (url.includes('/conversations/c1/messages?')) return { body: history };
      return { body: [] };
    },
  });
  await app.flush(() => app.sockets.length > 0);
  const socket = app.sockets.at(-1);
  socket.emit('open');
  socket.emit('message', { data: JSON.stringify({ type: 'ready' }) });
  await app.flush(() => app.voiceOptions.voice.isInvisible());
  app.voiceOptions.voice.onIncoming({ conversationId: 'c1' });
  await app.flush(() => app.created.some(node => node.className === 'message-inline-action' && node.textContent === 'Reply'));
  await app.flush();
  app.created.find(node => node.className === 'message-inline-action' && node.textContent === 'Reply').onclick();
  assert.ok(!app.$('#replyComposerPreview').classList.contains('hidden'), 'a reply target is set on server A');
  const disconnectsBefore = app.observerDisconnects.count;
  app.$('#serverAddressInput').value = B;
  await app.$('#changeServerButton').onclick({ currentTarget: app.$('#changeServerButton') });
  await app.flush();
  assert.equal(app.voiceOptions.voice.isInvisible(), false, 'server A\'s saved presence does not apply on server B');
  assert.ok(app.$('#replyComposerPreview').classList.contains('hidden'), 'the reply target from server A is cleared');
  assert.ok(app.observerDisconnects.count > disconnectsBefore, 'attachment views (and their object URLs) from server A are released');
});

async function openChatAndFailSend({ encrypted = false, history = [], beforeSend = null, extraHooks = {} } = {}) {
  let offline = false;
  const conversation = { id: 'c1', kind: 'direct', peer_id: 'peer', display_name: 'Peer', username: 'peer' };
  const base = 'https://a.example';
  const hooks = encrypted ? {
    crypto_has_direct_group: () => true,
    crypto_direct_peer_fingerprint: () => 'fp-peer',
    crypto_encrypt_text: () => ({ payload: 'ciphertext' }),
    ...extraHooks,
  } : {};
  const app = harness({
    storage: { 'ghostchat.server': base, ...(encrypted ? { [`ghostchat.cryptoPeerTrusted.${base}.me.c1`]: 'fp-peer' } : {}) },
    credentials: { [await serverKey(base)]: 'token-A' },
    hooks,
    fetchHandler: (url, options) => {
      if (url.endsWith('/v1/me')) return { body: { id: 'me', username: 'me', display_name: 'Me' } };
      if (url.endsWith('/v1/conversations')) return { body: [conversation] };
      if (offline && options.method === 'POST' && url.includes('/messages')) throw new TypeError('Failed to fetch');
      if (options.method === 'POST' && url.endsWith('/conversations/c1/messages')) {
        const sent = JSON.parse(options.body);
        return { body: { sequence: 99, id: `s-${sent.client_message_id}`, conversation_id: 'c1', sender_id: 'me', sender_name: 'Me', client_message_id: sent.client_message_id, content: sent.content, reply_to_id: sent.reply_to_id, created_at: '2026-01-01T00:00:00Z' } };
      }
      if (url.includes('/conversations/c1/messages?')) return { body: history };
      return { body: [] };
    },
  });
  await app.flush(() => app.sockets.length > 0);
  // The incoming-call path is the simplest real entry point into openConversation().
  app.voiceOptions.voice.onIncoming({ conversationId: 'c1' });
  await app.flush(() => app.fetches.some(item => item.url.includes('/conversations/c1/messages?')));
  // loadMessages() has no observable end; let its remaining awaits settle.
  await app.flush();
  if (beforeSend) beforeSend(app);
  offline = true;
  app.$('#messageInput').value = 'hello there';
  const [submit] = app.$('#composer').__listeners.get('submit');
  await submit({ preventDefault() {} });
  assert.ok(app.fetches.some(item => item.method === 'POST' && item.url.endsWith('/conversations/c1/messages')), 'the send was attempted');
  app.goOnline = () => { offline = false; };
  app.submitComposer = submit;
  return app;
}

test('a signed-in account whose presence could not be loaded stays silent like an invisible one', async () => {
  const A = 'https://a.example';
  let presenceFails = true;
  const app = harness({
    storage: { 'ghostchat.server': A },
    credentials: { [await serverKey(A)]: 'token-A' },
    fetchHandler: url => {
      if (url.endsWith('/v1/me')) return { body: { id: 'me', username: 'me', display_name: 'Me' } };
      if (url.endsWith('/v1/me/presence')) return presenceFails ? { status: 500, body: {} } : { body: [{ user_id: 'me', mode: 'online' }] };
      return { body: [] };
    },
  });
  await app.flush(() => app.sockets.length > 0);
  const socket = app.sockets.at(-1);
  socket.emit('open');
  socket.emit('message', { data: JSON.stringify({ type: 'ready' }) });
  await app.flush(() => app.fetches.some(item => item.url.endsWith('/v1/me/presence')));
  await app.flush();
  // The saved mode may be invisible; until the server says otherwise the
  // automatic busy/decline replies must not reveal this device.
  assert.equal(app.voiceOptions.voice.isInvisible(), true, 'an unknown saved mode is treated as invisible');
  socket.emit('message', { data: JSON.stringify({ type: 'presence_updated', user_id: 'me', mode: 'online' }) });
  assert.equal(app.voiceOptions.voice.isInvisible(), false, 'the server\'s own answer settles it');
});

test('a failed send puts the typed text back into the composer', async () => {
  const app = await openChatAndFailSend();
  assert.equal(app.$('#messageInput').value, 'hello there', 'a failed send must not discard the typed message');
});

test('a restored draft is fitted to the composer height', async () => {
  const app = await openChatAndFailSend({ beforeSend: app => { app.$('#messageInput').scrollHeight = 64; } });
  assert.equal(app.$('#messageInput').value, 'hello there');
  assert.equal(app.$('#messageInput').style.height, '64px', 'the restored text is not squeezed into one line');
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

test('resending a restored draft with a different reply target uses a new client message id', async () => {
  const earlier = { sequence: 1, id: 'm1', conversation_id: 'c1', sender_id: 'peer', sender_name: 'Peer', client_message_id: 'x1', content: 'earlier', created_at: '2026-01-01T00:00:00Z' };
  const app = await openChatAndFailSend({
    history: [earlier],
    beforeSend: app => {
      const reply = app.created.find(node => node.className === 'message-inline-action' && node.textContent === 'Reply');
      assert.ok(reply, 'the history message has a Reply action');
      reply.onclick();
    },
  });
  const sent = () => app.fetches
    .filter(item => item.method === 'POST' && item.url.endsWith('/conversations/c1/messages'))
    .map(item => JSON.parse(item.body));
  const [failed] = sent();
  assert.equal(failed.reply_to_id, 'm1', 'the failed send was a reply');
  app.$('#cancelReply').onclick();
  app.goOnline();
  await app.submitComposer({ preventDefault() {} });
  const retry = sent()[1];
  assert.equal(retry.reply_to_id, null);
  assert.notEqual(retry.client_message_id, failed.client_message_id, 'a different reply target is a different message; reusing the id would return the old reply');
});

test('a failed encrypted send that is queued for retry is not restored', async () => {
  const app = await openChatAndFailSend({ encrypted: true });
  const pending = JSON.parse(app.store.get('ghostchat.cryptoMessagePending.https://a.example.me.c1') || 'null');
  assert.equal(pending?.payload, 'ciphertext', 'the ciphertext is queued for automatic retry');
  assert.equal(app.$('#messageInput').value, '', 'restoring a queued message would let it be sent twice');
});

test('a safety copy that could not be written pauses encrypted chats with its own translated message', async () => {
  // The exact text of crypto::LOCAL_BACKUP_FAILED; Tauri rejects with it as a string.
  const failure = 'Encrypted chats are paused because GhostChat could not write a safety copy of its local data before upgrading it. Free some disk space, then restart GhostChat.';
  const turkish = 'Yükseltmeden önce yerel verilerin güvenlik kopyası yazılamadığı için şifreli sohbetler duraklatıldı. Diskte yer aç, ardından GhostChat’i yeniden başlat.';
  for (const [language, expected] of [['en', failure], ['tr', turkish]]) {
    const base = 'https://a.example';
    const conversation = { id: 'c1', kind: 'direct', peer_id: 'peer', display_name: 'Peer', username: 'peer' };
    const app = harness({
      storage: { 'ghostchat.server': base, 'ghostchat.language': language },
      credentials: { [await serverKey(base)]: 'token-A' },
      hooks: { crypto_has_direct_group: () => { throw failure; } },
      fetchHandler: url => {
        if (url.endsWith('/v1/me')) return { body: { id: 'me', username: 'me', display_name: 'Me' } };
        if (url.endsWith('/v1/conversations')) return { body: [conversation] };
        // A locked conversation would otherwise show the generic setup hint.
        if (url.endsWith('/conversations/c1/crypto-lock')) return { body: { locked: true, setupComplete: true } };
        return { body: [] };
      },
    });
    await app.flush(() => app.sockets.length > 0);
    app.voiceOptions.voice.onIncoming({ conversationId: 'c1' });
    await app.flush(() => app.invokes.some(item => item.command === 'crypto_has_direct_group'));
    await app.flush();
    assert.equal(app.$('#cryptoConversationStatus').textContent, expected, `${language}: the status names the failed safety copy`);
  }
});

const SAFETY_COPY_FAILURE = 'Encrypted chats are paused because GhostChat could not write a safety copy of its local data before upgrading it. Free some disk space, then restart GhostChat.';
const SAFETY_COPY_FAILURE_TR = 'Yükseltmeden önce yerel verilerin güvenlik kopyası yazılamadığı için şifreli sohbetler duraklatıldı. Diskte yer aç, ardından GhostChat’i yeniden başlat.';

async function openTrustedEncryptedChat(language, hooks) {
  const base = 'https://a.example';
  const conversation = { id: 'c1', kind: 'direct', peer_id: 'peer', display_name: 'Peer', username: 'peer' };
  const app = harness({
    storage: { 'ghostchat.server': base, 'ghostchat.language': language, [`ghostchat.cryptoPeerTrusted.${base}.me.c1`]: 'fp-peer' },
    credentials: { [await serverKey(base)]: 'token-A' },
    hooks: { crypto_has_direct_group: () => true, crypto_direct_peer_fingerprint: () => 'fp-peer', ...hooks },
    fetchHandler: url => {
      if (url.endsWith('/v1/me')) return { body: { id: 'me', username: 'me', display_name: 'Me' } };
      if (url.endsWith('/v1/conversations')) return { body: [conversation] };
      if (url.endsWith('/conversations/c1/crypto-lock')) return { body: { locked: true, setupComplete: true } };
      return { body: [] };
    },
  });
  await app.flush(() => app.sockets.length > 0);
  app.voiceOptions.voice.onIncoming({ conversationId: 'c1' });
  await app.flush(() => app.invokes.some(item => item.command === 'crypto_has_direct_group'));
  await app.flush();
  return app;
}

test('a failed safety copy is named when an encrypted send fails', async () => {
  for (const [language, expected] of [['en', SAFETY_COPY_FAILURE], ['tr', SAFETY_COPY_FAILURE_TR]]) {
    const app = await openTrustedEncryptedChat(language, { crypto_encrypt_text: () => { throw SAFETY_COPY_FAILURE; } });
    app.$('#messageInput').value = 'hello there';
    const [submit] = app.$('#composer').__listeners.get('submit');
    await submit({ preventDefault() {} });
    assert.ok(app.invokes.some(item => item.command === 'crypto_encrypt_text'), 'the encrypted send was attempted');
    assert.equal(app.$('#toast').textContent, expected, `${language}: the send failure names the safety copy`);
  }
});

test('a failed safety copy is named when encrypted events cannot be synced', async () => {
  for (const [language, expected] of [['en', SAFETY_COPY_FAILURE], ['tr', SAFETY_COPY_FAILURE_TR]]) {
    const app = await openTrustedEncryptedChat(language, { crypto_get_event_cursor: () => { throw SAFETY_COPY_FAILURE; } });
    assert.ok(app.invokes.some(item => item.command === 'crypto_get_event_cursor'), 'the sync was attempted');
    assert.equal(app.$('#toast').textContent, expected, `${language}: the sync failure names the safety copy`);
  }
});

test('an encrypted file message names its encrypted attachment for the server', async () => {
  const attachmentId = '0b3c9a52-6f1e-4c55-9d43-3f6d2f6c1a10';
  const app = await openChatAndFailSend({
    encrypted: true,
    extraHooks: { crypto_upload_encrypted_attachment: () => ({ attachmentId, sizeBytes: 0, chunkSize: 1048576, chunkCount: 1 }) },
  });
  app.goOnline();
  localStorageClear(app);
  app.$('#attachFileButton').onclick();
  const sends = () => app.fetches.filter(item => item.method === 'POST' && item.url.endsWith('/conversations/c1/messages')).map(item => JSON.parse(item.body));
  await app.flush(() => sends().some(body => (body.attachment_ids || []).length));
  const sent = sends().find(body => (body.attachment_ids || []).length);
  assert.deepEqual(sent.attachment_ids, [attachmentId]);
});

function localStorageClear(app) {
  // Drop the earlier failed send's queued ciphertext so the next send is new.
  for (const key of [...app.store.keys()]) if (key.startsWith('ghostchat.cryptoMessagePending.')) app.store.delete(key);
}

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
  assert.deepEqual(JSON.parse(removal.body).member_ids, ['carol'], 'the removal declares its target');
  await app.flush(() => app.invokes.some(item => item.command === 'crypto_finalize_group_membership_update'));
});

async function deletedLeafHarness({ role = 'owner', deletedAt = '2000-01-01T00:00:00.000Z', removeStatus = 204, removeError = 'refused', removeHtml = false, removeReply = null, skipRefused = false, deleted = ['carol'], serverNow = undefined, dateNow = null } = {}) {
  const base = 'https://a.example';
  const group = { id: 'g1', kind: 'group', display_name: 'Group' };
  let prepared = 0;
  const gone = new Set();
  let staged = null;
  const app = harness({
    storage: { 'ghostchat.server': base },
    credentials: { [await serverKey(base)]: 'token-A' },
    dateNow,
    hooks: {
      crypto_has_direct_group: () => true,
      crypto_group_fingerprint: () => 'fp-group',
      crypto_get_event_cursor: () => 0,
      crypto_advance_event_cursor: args => args.sequence,
      crypto_prepare_deleted_member_removal: args => {
        // Like the Rust side, leaves the caller excludes are never staged.
        const target = deleted.find(id => !gone.has(id) && !(args.excludeMemberIds || []).includes(id));
        if (staged) return staged;
        if (!target) return false;
        prepared += 1;
        return staged = { operationId: `op-${prepared}`, action: 'remove_deleted', memberIds: [target], expectedMemberIds: ['bob', 'me', 'olga'], commitPayload: 'commit', welcomePayload: '' };
      },
      crypto_get_pending_group_membership_update: () => staged,
      crypto_discard_group_membership_update: () => { staged = null; return null; },
      crypto_finalize_group_membership_update: () => { staged = null; return 'fp-after'; },
    },
    fetchHandler: (url, options) => {
      if (url.endsWith('/v1/me')) return { body: { id: 'me', username: 'me', display_name: 'Me' } };
      if (url.endsWith('/v1/conversations')) return { body: [group] };
      if (url.endsWith('/conversations/g1/crypto-lock')) return { body: { locked: true, setupComplete: true, lockedBy: 'olga', deletedMemberIds: deleted, deletedMembers: deleted.map(userId => ({ userId, deletedAt, removalCommittedAt: null })), serverNow } };
      if (url.endsWith('/conversations/g1/members')) {
        return { body: role === 'owner'
          ? [{ id: 'me', role: 'owner', joined_seq: 1 }, { id: 'bob', role: 'member', joined_seq: 2 }]
          : [{ id: 'olga', role: 'owner', joined_seq: 1 }, { id: 'bob', role: 'admin', joined_seq: 2 }, { id: 'me', role: 'admin', joined_seq: 3 }] };
      }
      if (options.method === 'POST' && url.includes('/crypto-membership/remove/')) {
        const target = decodeURIComponent(url.split('/crypto-membership/remove/')[1]);
        const reply = removeReply ? removeReply(target) : { status: removeStatus, error: removeError, html: removeHtml };
        if (reply.status === 204 || reply.status === 410 || (skipRefused && reply.status === 403)) gone.add(target);
        return { status: reply.status, html: reply.html, body: reply.status === 204 ? {} : { error: reply.error } };
      }
      return { body: [] };
    },
  });
  await app.flush(() => app.sockets.length > 0);
  app.voiceOptions.voice.onIncoming({ conversationId: 'g1' });
  await app.flush(() => app.fetches.some(item => item.url.includes('/conversations/g1/crypto-events')));
  await app.flush();
  app.removals = () => app.fetches.filter(item => item.method === 'POST' && item.url.includes('/crypto-membership/remove/'));
  app.discards = () => app.invokes.filter(item => item.command === 'crypto_discard_group_membership_update');
  app.prepared = () => prepared;
  app.again = async () => { app.voiceOptions.voice.onIncoming({ conversationId: 'g1' }); await app.flush(); };
  return app;
}

test('a definitively refused staged removal is dropped once and not retried', async () => {
  const app = await deletedLeafHarness({ removeStatus: 409, removeError: 'The encrypted group membership changed. Refresh the group and try again.' });
  await app.flush(() => app.discards().length > 0);
  assert.equal(app.removals().length, 1, 'the refused removal is not retried in a loop');
  assert.equal(app.discards()[0].args.operationId, 'op-1');
  assert.ok(!app.invokes.some(item => item.command === 'crypto_finalize_group_membership_update'));
  assert.equal(app.$('#toast').classList.contains('error'), true, 'the refusal is reported once');
});

test('a removal another owner/admin already committed counts as done', async () => {
  const app = await deletedLeafHarness({ removeStatus: 410, removeError: 'This deleted account was already removed from the encrypted group.' });
  await app.flush(() => app.discards().length > 0);
  assert.equal(app.removals().length, 1);
  assert.equal(app.prepared(), 1, 'no further removal is staged; the accepted commit arrives on the relay');
  assert.ok(!app.$('#toast').textContent.includes('refused'), 'no error is shown for an already removed leaf');
});

for (const status of [408, 425]) {
  test(`a ${status} on a staged removal is transient: the update is kept and retried`, async () => {
    const app = await deletedLeafHarness({ removeStatus: status, removeError: 'try later' });
    await app.flush(() => app.removals().length > 0);
    await app.flush();
    assert.equal(app.discards().length, 0, 'the staged update is not discarded');
    assert.ok(!app.$('#toast').textContent.includes('discarded'), 'no refusal is reported');
    await app.again();
    assert.ok(app.removals().length >= 2, 'the removal is tried again');
  });
}

test('a 4xx without the app\'s JSON error shape (a proxy page) keeps the staged removal', async () => {
  const app = await deletedLeafHarness({ removeStatus: 403, removeHtml: true });
  await app.flush(() => app.removals().length > 0);
  await app.flush();
  assert.equal(app.discards().length, 0);
  assert.ok(!app.$('#toast').textContent.includes('discarded'));
  await app.again();
  assert.ok(app.removals().length >= 2, 'it is retried');
});

test('a 403 with the app\'s JSON error discards the staged removal and reports it once', async () => {
  const app = await deletedLeafHarness({ removeStatus: 403, removeError: 'Only an owner or admin may remove members.' });
  await app.flush(() => app.discards().length > 0);
  assert.equal(app.removals().length, 1);
  assert.equal(app.$('#toast').classList.contains('error'), true);
  await app.again();
  assert.equal(app.removals().length, 1, 'the same refused target is not retried in a loop');
});

test('a 410 for one deleted member does not block the removal of the next one', async () => {
  const app = await deletedLeafHarness({ deleted: ['c1', 'c2'], removeReply: target => target === 'c1' ? { status: 410, error: 'already removed' } : { status: 204 } });
  await app.flush(() => app.removals().length > 0);
  await app.flush();
  await app.again();
  await app.flush(() => app.removals().length > 1);
  assert.deepEqual(app.removals().map(item => item.url.split('/remove/')[1]), ['c1', 'c2'], 'C2 is staged without a restart');
});

test('a definitive refusal for one deleted member does not block the next one', async () => {
  // The staging side moves on to C2 once C1 is not offered again (here the fake drops it).
  const app = await deletedLeafHarness({ deleted: ['c1', 'c2'], skipRefused: true, removeReply: target => target === 'c1' ? { status: 403, error: 'refused' } : { status: 204 } });
  await app.flush(() => app.discards().length > 0);
  await app.again();
  await app.flush(() => app.removals().length > 1);
  assert.deepEqual(app.removals().map(item => item.url.split('/remove/')[1]), ['c1', 'c2']);
});

test('the refused deleted members are excluded when the next removal is prepared', async () => {
  // The fake keeps offering C1 unless the caller excludes it.
  const app = await deletedLeafHarness({ deleted: ['c1', 'c2'], removeReply: target => target === 'c1' ? { status: 403, error: 'refused' } : { status: 204 } });
  await app.flush(() => app.discards().length > 0);
  await app.again();
  await app.flush(() => app.removals().length > 1);
  assert.deepEqual(app.removals().map(item => item.url.split('/remove/')[1]), ['c1', 'c2'], 'C2 is staged and sent after C1 was refused');
  const prepares = app.invokes.filter(item => item.command === 'crypto_prepare_deleted_member_removal');
  assert.deepEqual([...prepares[0].args.excludeMemberIds], [], 'nothing is excluded before a refusal');
  assert.deepEqual([...prepares.at(-1).args.excludeMemberIds], ['c1'], 'the refused target is passed to the prepare call');
});

test('an admin removes a deleted leaf after the owner\'s grace period', async () => {
  const late = await deletedLeafHarness({ role: 'admin', deletedAt: '2000-01-01T00:00:00.000Z' });
  await late.flush(() => late.removals().length > 0);
  assert.ok(late.removals()[0].url.endsWith('/crypto-membership/remove/carol'));
  const early = await deletedLeafHarness({ role: 'admin', deletedAt: new Date().toISOString() });
  assert.equal(early.removals().length, 0, 'an admin waits while the owner may still remove the leaf');
  assert.ok(early.timers.some(timer => !timer.done && timer.ms > 0), 'and checks again when its turn comes');
});

// An encrypted group whose server counts accepted commits (commitSeq). The
// lock, the relay pages and the answers to membership requests are scripted.
async function commitFenceHarness({ lock, pages = [], replies = [], members = null, friends = [], selectAll = null, pending = null, health = null, language = 'en' } = {}) {
  const base = 'https://a.example';
  const group = { id: 'g1', kind: 'group', display_name: 'Group' };
  const prepared = [];
  let staged = pending;
  let cursor = 0;
  const app = harness({
    storage: { 'ghostchat.server': base, 'ghostchat.language': language },
    credentials: { [await serverKey(base)]: 'token-A' },
    selectAll,
    hooks: {
      crypto_has_direct_group: () => true,
      crypto_group_fingerprint: () => 'fp-group',
      crypto_get_event_cursor: () => cursor,
      crypto_advance_event_cursor: args => { cursor = Math.max(cursor, args.sequence); return cursor; },
      crypto_inspect_event: () => ({ kind: 'application' }),
      crypto_process_event: args => { cursor = Math.max(cursor, args.eventSequence); return { kind: 'application', content: null }; },
      crypto_inspect_key_package: () => ({ identityFingerprint: 'fp-dave' }),
      crypto_get_pending_group_membership_update: () => staged,
      crypto_prepare_deleted_member_removal: args => {
        if (staged) return staged;
        prepared.push({ command: 'remove_deleted', expectedCommitSeq: args.expectedCommitSeq });
        if (!replies.some(reply => reply.repeat) && prepared.filter(item => item.command === 'remove_deleted').length > replies.filter(reply => reply.for === 'remove').length) return false;
        staged = { operationId: `op-${prepared.length}`, action: 'remove_deleted', memberIds: ['carol'], expectedMemberIds: ['bob', 'me'], commitPayload: 'commit', welcomePayload: '' };
        if (args.expectedCommitSeq != null) staged.expectedCommitSeq = args.expectedCommitSeq;
        return staged;
      },
      crypto_prepare_group_membership_update: args => {
        prepared.push({ command: 'add', expectedCommitSeq: args.expectedCommitSeq });
        staged = { operationId: `op-${prepared.length}`, action: 'add', memberIds: ['dave'], expectedMemberIds: ['bob', 'me'], commitPayload: 'commit', welcomePayload: 'welcome' };
        if (args.expectedCommitSeq != null) staged.expectedCommitSeq = args.expectedCommitSeq;
        return staged;
      },
      crypto_finalize_group_membership_update: () => { staged = null; return 'fp-after'; },
      crypto_discard_group_membership_update: () => { staged = null; return null; },
    },
    fetchHandler: (url, options) => {
      if (health && url.endsWith('/health')) return { body: health };
      if (url.endsWith('/v1/me')) return { body: { id: 'me', username: 'me', display_name: 'Me' } };
      if (url.endsWith('/v1/conversations')) return { body: [group] };
      if (url.endsWith('/v1/friends')) return { body: friends };
      if (url.endsWith('/conversations/g1/crypto-lock')) return { body: typeof lock === 'function' ? lock() : lock };
      if (url.endsWith('/conversations/g1/members') && options.method !== 'POST') return { body: members || [{ id: 'me', role: 'owner', joined_seq: 1 }, { id: 'bob', role: 'member', joined_seq: 2 }] };
      if (url.includes('/conversations/g1/crypto-events?after=')) {
        const after = Number(new URL(url).searchParams.get('after'));
        const page = pages.find(candidate => candidate.length && candidate[0].sequence > after);
        return { body: page || [] };
      }
      if (url.includes('/crypto/key-package')) return { body: { device_id: 'device-dave', payload: 'package' } };
      const membership = options.method === 'POST' && (url.includes('/crypto-membership/remove/') ? 'remove' : url.endsWith('/conversations/g1/members') ? 'add' : null);
      if (membership) {
        const sent = app.fetches.filter(item => item.method === 'POST' && (item.url.includes('/crypto-membership/remove/') || item.url.endsWith('/conversations/g1/members'))).length;
        const reply = replies[Math.min(sent, replies.length) - 1] || { status: 204 };
        reply.then?.();
        return { status: reply.status, body: reply.status < 300 ? (membership === 'add' ? [] : {}) : { error: reply.error || 'refused', code: reply.code } };
      }
      return { body: [] };
    },
  });
  app.prepared = prepared;
  app.membershipPosts = () => app.fetches.filter(item => item.method === 'POST' && (item.url.includes('/crypto-membership/remove/') || item.url.endsWith('/conversations/g1/members')));
  app.discards = () => app.invokes.filter(item => item.command === 'crypto_discard_group_membership_update');
  await app.flush(() => app.sockets.length > 0);
  app.voiceOptions.voice.onIncoming({ conversationId: 'g1' });
  await app.flush(() => app.fetches.some(item => item.url.includes('/conversations/g1/crypto-events')));
  await app.flush();
  return app;
}
const relayEvent = (sequence, commitSeq) => ({ sequence, id: `e${sequence}`, sender_id: 'bob', recipient_id: 'me', payload: 'p', created_at: '', setup_generation: 0, commit_seq: commitSeq });
const deletedCarol = commitSeq => ({ locked: true, setupComplete: true, lockedBy: 'me', deletedMemberIds: ['carol'], deletedMembers: [{ userId: 'carol', deletedAt: '2000-01-01T00:00:00.000Z', removalCommittedAt: null }], setupGeneration: 0, commitSeq });

test('a staged removal names the commit sequence this device has synced to', async () => {
  // The lock was read at 5; the relay page listed afterwards already says 6,
  // and this device applied everything on that page.
  const app = await commitFenceHarness({ lock: deletedCarol(5), pages: [[relayEvent(1, 6)]], replies: [{ for: 'remove', status: 204 }] });
  await app.flush(() => app.invokes.some(item => item.command === 'crypto_finalize_group_membership_update'));
  await app.flush();
  assert.equal(app.prepared[0].expectedCommitSeq, 6, 'the removal is built on the last state this device applied');
  assert.equal(JSON.parse(app.membershipPosts()[0].body).expected_commit_seq, 6, 'and says so to the server');
  assert.equal(app.prepared[1].expectedCommitSeq, 7, 'the accepted commit moves this device one state on');
});

test('a server without a commit sequence still receives removals', async () => {
  const lock = deletedCarol(0); delete lock.commitSeq;
  const event = relayEvent(1, 0); delete event.commit_seq;
  const app = await commitFenceHarness({ lock, pages: [[event]], replies: [{ for: 'remove', status: 204 }] });
  await app.flush(() => app.membershipPosts().length > 0);
  assert.equal(app.prepared[0].expectedCommitSeq ?? null, null);
  assert.ok(!('expected_commit_seq' in JSON.parse(app.membershipPosts()[0].body)));
});

// A 4.0.50 server has no commit sequence, so it cannot keep two members'
// changes built on one group state apart; 4.0.51 has it but no version in
// /health; newer servers name their version there.
test('adding a member on a server without a commit sequence asks to update the server, naming its version', async () => {
  const friends = [{ id: 'dave', username: 'dave', display_name: 'Dave' }];
  const lock = { locked: true, setupComplete: true, lockedBy: 'me' };
  const cases = [
    ['en', { status: 'ok', service: 'ghostchat' }, 'This server runs GhostChat 4.0.50 or older. Update the server to add or remove members of encrypted groups.'],
    ['tr', { status: 'ok', service: 'ghostchat' }, 'Bu sunucuda GhostChat 4.0.50 veya daha eski bir sürüm çalışıyor. Şifreli gruplara üye eklemek veya çıkarmak için sunucuyu güncelle.'],
    ['en', { status: 'ok', service: 'ghostchat', version: '4.0.50' }, 'This server runs GhostChat 4.0.50. Update the server to add or remove members of encrypted groups.'],
  ];
  for (const [language, health, expected] of cases) {
    const app = await commitFenceHarness({
      lock, friends, health, language,
      selectAll: selector => (selector === '#groupInviteList input:checked' ? [{ value: 'dave' }] : []),
    });
    await app.flush(() => app.fetches.some(item => item.url.endsWith('/v1/friends')));
    await app.flush();
    assert.ok(app.fetches.some(item => item.url.endsWith('/health')), 'the server version is read from /health');
    await app.$('#inviteGroupFriends').onclick({ currentTarget: app.$('#inviteGroupFriends') });
    assert.equal(app.$('#groupManageStatus').textContent, expected, `${language}: the reason names the server version`);
    assert.ok(!app.fetches.some(item => item.url.includes('/crypto/key-package')), 'no key package is claimed for a change that cannot be made');
    assert.equal(app.membershipPosts().length, 0);
  }
});

test('a server that reports its version and a commit sequence adds members as before', async () => {
  const friends = [{ id: 'dave', username: 'dave', display_name: 'Dave' }];
  const lock = { locked: true, setupComplete: true, lockedBy: 'me', deletedMemberIds: [], deletedMembers: [], setupGeneration: 0, commitSeq: 3 };
  const app = await commitFenceHarness({
    lock, friends, pages: [[relayEvent(1, 3)]], health: { status: 'ok', service: 'ghostchat', version: '4.0.52' },
    selectAll: selector => (selector === '#groupInviteList input:checked' ? [{ value: 'dave' }] : []),
  });
  await app.flush(() => app.fetches.some(item => item.url.endsWith('/v1/friends')));
  await app.flush();
  const adding = app.$('#inviteGroupFriends').onclick({ currentTarget: app.$('#inviteGroupFriends') });
  await app.flush(() => app.$('#appDecisionTitle').textContent === 'Member security codes');
  app.$('#appDecisionConfirm').onclick();
  await adding;
  assert.equal(JSON.parse(app.membershipPosts()[0].body).crypto_update.expected_commit_seq, 3);
});

test('a commit refused as stale is dropped and staged again after the next sync', async () => {
  // Another manager's commit is accepted first: the server is at 6 when this
  // device's removal, built on 5, arrives.
  let commitSeq = 5;
  const stale = { status: 409, error: 'The encrypted group changed while this update was being prepared. Sync the group and try again.', code: 'stale_commit_seq' };
  const app = await commitFenceHarness({
    lock: () => deletedCarol(commitSeq),
    replies: [{ for: 'remove', ...stale, then: () => { commitSeq = 6; } }, { for: 'remove', status: 204 }],
  });
  await app.flush(() => app.discards().length > 0);
  assert.equal(app.prepared[0].expectedCommitSeq, 5);
  assert.equal(app.discards()[0].args.operationId, 'op-1', 'the commit built on the older state is dropped');
  await app.flush();
  assert.ok(app.timers.some(timer => timer.ms === 1000), 'the removal is looked at again without waiting for another event');
  app.runTimers();
  await app.flush(() => app.membershipPosts().length > 1);
  assert.equal(app.prepared[1].expectedCommitSeq, 6, 'the removal is staged again on the synced state');
  assert.equal(JSON.parse(app.membershipPosts()[1].body).expected_commit_seq, 6);
  await app.flush(() => app.invokes.some(item => item.command === 'crypto_finalize_group_membership_update'));
  assert.equal(app.$('#toast').classList.contains('error'), false, 'a concurrent change by another manager is not an error to show');
});

test('a removal that keeps being refused as stale stops after a few attempts', async () => {
  const stale = { for: 'remove', repeat: true, status: 409, error: 'stale', code: 'stale_commit_seq' };
  const app = await commitFenceHarness({ lock: deletedCarol(5), replies: [stale] });
  for (let round = 0; round < 8; round += 1) {
    await app.flush();
    app.runTimers();
  }
  await app.flush();
  const attempts = app.membershipPosts().length;
  assert.equal(attempts, 4, 'the removal is retried three times');
  app.voiceOptions.voice.onIncoming({ conversationId: 'g1' });
  await app.flush();
  app.runTimers();
  await app.flush();
  assert.equal(app.membershipPosts().length, attempts, 'and then left until the roster or the deleted list changes');
});

test('a device that has not caught up with the relay does not stage a commit', async () => {
  // More relay pages than one sync reads: the device is still behind.
  const pages = Array.from({ length: 21 }, (_, page) => Array.from({ length: 100 }, (_, index) => relayEvent(page * 100 + index + 1, 30)));
  const app = await commitFenceHarness({ lock: deletedCarol(30), pages, replies: [{ for: 'remove', status: 204 }] });
  await app.flush(() => app.invokes.filter(item => item.command === 'crypto_process_event').length >= 2000);
  await app.flush();
  assert.equal(app.prepared.length, 0, 'no commit is built on a state the device knows is old');
  assert.equal(app.membershipPosts().length, 0);
});

test('adding a member syncs first and names the synced commit sequence', async () => {
  const friends = [{ id: 'dave', username: 'dave', display_name: 'Dave' }];
  const lock = { locked: true, setupComplete: true, lockedBy: 'me', deletedMemberIds: [], deletedMembers: [], setupGeneration: 0, commitSeq: 3 };
  const app = await commitFenceHarness({
    lock, friends, pages: [[relayEvent(1, 3)]],
    selectAll: selector => (selector === '#groupInviteList input:checked' ? [{ value: 'dave' }] : []),
    replies: [{ for: 'add', status: 409, error: 'The encrypted group changed while this update was being prepared. Sync the group and try again.', code: 'stale_commit_seq' }],
  });
  await app.flush(() => app.fetches.some(item => item.url.endsWith('/v1/friends')));
  await app.flush();
  const syncsBefore = app.fetches.filter(item => item.url.includes('/conversations/g1/crypto-events')).length;
  const adding = app.$('#inviteGroupFriends').onclick({ currentTarget: app.$('#inviteGroupFriends') });
  await app.flush(() => app.$('#appDecisionTitle').textContent === 'Member security codes');
  assert.ok(app.fetches.filter(item => item.url.includes('/conversations/g1/crypto-events')).length > syncsBefore, 'the relay is synced before the commit is built');
  app.$('#appDecisionConfirm').onclick();
  await adding;
  assert.equal(app.prepared.find(item => item.command === 'add').expectedCommitSeq, 3);
  const post = app.membershipPosts()[0];
  assert.equal(JSON.parse(post.body).crypto_update.expected_commit_seq, 3);
  // Another manager's commit won: the staged add is dropped and the user is
  // told to try again, not that the friend was added.
  assert.equal(app.discards().length, 1);
  assert.ok(app.$('#groupManageStatus').textContent.includes('Try again'), app.$('#groupManageStatus').textContent);
  assert.notEqual(app.$('#toast').textContent, 'Friends added to the group.');
});

test('a refused encrypted add is not reported as done', async () => {
  const friends = [{ id: 'dave', username: 'dave', display_name: 'Dave' }];
  const lock = { locked: true, setupComplete: true, lockedBy: 'me', deletedMemberIds: [], deletedMembers: [], setupGeneration: 0, commitSeq: 3 };
  const app = await commitFenceHarness({
    lock, friends,
    selectAll: selector => (selector === '#groupInviteList input:checked' ? [{ value: 'dave' }] : []),
    replies: [{ for: 'add', status: 409, error: 'Resolve the pending encrypted leave request before changing group members.' }],
  });
  await app.flush(() => app.fetches.some(item => item.url.endsWith('/v1/friends')));
  await app.flush();
  const adding = app.$('#inviteGroupFriends').onclick({ currentTarget: app.$('#inviteGroupFriends') });
  await app.flush(() => app.$('#appDecisionTitle').textContent === 'Member security codes');
  app.$('#appDecisionConfirm').onclick();
  await adding;
  assert.equal(app.discards().length, 1, 'the refused commit is dropped');
  assert.notEqual(app.$('#toast').textContent, 'Friends added to the group.', 'nobody was added');
  assert.ok(app.$('#groupManageStatus').textContent.includes('refused'), app.$('#groupManageStatus').textContent);
});

test('an admin measures the owner\'s grace period on the server\'s clock, not its own', async () => {
  const minute = 60 * 1000;
  const serverNow = '2026-03-01T12:00:00.000Z';
  const server = Date.parse(serverNow);
  // This device's clock is a day ahead. The account was deleted ten seconds
  // ago on the server's clock: the owner's turn is not over.
  const ahead = await deletedLeafHarness({ role: 'admin', serverNow, deletedAt: new Date(server - 10 * 1000).toISOString(), dateNow: () => server + 24 * 60 * minute });
  assert.equal(ahead.removals().length, 0, 'a clock that runs ahead does not bring the fallback forward');
  // Second admin in join order: two grace periods, less the ten seconds.
  assert.ok(ahead.timers.some(timer => !timer.done && timer.ms === 4 * minute - 10 * 1000), 'it checks again when the server-side wait is over');
  // The clock is a day behind and the leaf has waited an hour: remove now.
  const behind = await deletedLeafHarness({ role: 'admin', serverNow, deletedAt: new Date(server - 60 * minute).toISOString(), dateNow: () => server - 24 * 60 * minute });
  await behind.flush(() => behind.removals().length > 0);
  assert.ok(behind.removals()[0].url.endsWith('/crypto-membership/remove/carol'), 'a clock that runs behind does not delay it');
});

async function deletedLockerHarness(role) {
  const base = 'https://a.example';
  const group = { id: 'g1', kind: 'group', display_name: 'Group' };
  // This device joined the deleted starter's group from its Welcome.
  let holdsGroup = true;
  let localGeneration = 0;
  let takenOver = false;
  const app = harness({
    storage: { 'ghostchat.server': base },
    credentials: { [await serverKey(base)]: 'token-A' },
    hooks: {
      crypto_has_direct_group: () => holdsGroup,
      crypto_apply_setup_generation: args => { if (args.generation <= localGeneration) return false; localGeneration = args.generation; holdsGroup = false; return true; },
      crypto_group_fingerprint: () => 'fp-group',
      crypto_get_event_cursor: () => 0,
      crypto_advance_event_cursor: args => args.sequence,
    },
    fetchHandler: (url, options) => {
      if (url.endsWith('/v1/me')) return { body: { id: 'me', username: 'me', display_name: 'Me' } };
      if (url.endsWith('/v1/conversations')) return { body: [group] };
      if (url.endsWith('/conversations/g1/crypto-lock') && options.method === 'POST') { takenOver = true; return { body: { locked: true, setupComplete: false, lockedBy: 'me', deletedMemberIds: [], deletedMembers: [], lockedByDeletedAccount: false, setupGeneration: 1 } }; }
      if (url.endsWith('/conversations/g1/crypto-lock/complete')) return { body: { locked: true, setupComplete: true, lockedBy: 'me', deletedMemberIds: [], deletedMembers: [], lockedByDeletedAccount: false, setupGeneration: 1 } };
      if (url.endsWith('/conversations/g1/crypto-lock') && takenOver) return { body: { locked: true, setupComplete: false, lockedBy: 'me', deletedMemberIds: [], deletedMembers: [], lockedByDeletedAccount: false, setupGeneration: 1 } };
      if (url.endsWith('/conversations/g1/crypto-lock')) return { body: { locked: true, setupComplete: false, lockedBy: 'carol', deletedMemberIds: [], deletedMembers: [], lockedByDeletedAccount: true, setupGeneration: 0 } };
      if (url.endsWith('/conversations/g1/members')) {
        return { body: role === 'owner'
          ? [{ id: 'me', role: 'owner', joined_seq: 1 }, { id: 'bob', role: 'admin', joined_seq: 2 }]
          : [{ id: 'olga', role: 'owner', joined_seq: 1 }, { id: 'me', role: 'admin', joined_seq: 2 }] };
      }
      return { body: [] };
    },
  });
  await app.flush(() => app.sockets.length > 0);
  app.voiceOptions.voice.onIncoming({ conversationId: 'g1' });
  await app.flush(() => app.fetches.some(item => item.url.endsWith('/conversations/g1/members')));
  await app.flush();
  return app;
}

test('the owner takes over a setup left unfinished by a deleted account as a reset, never completing the old one', async () => {
  const app = await deletedLockerHarness('owner');
  await app.flush(() => app.fetches.some(item => item.method === 'POST' && item.url.endsWith('/conversations/g1/crypto-lock')));
  await app.flush();
  const takeover = app.fetches.find(item => item.method === 'POST' && item.url.endsWith('/conversations/g1/crypto-lock'));
  assert.deepEqual(JSON.parse(takeover.body).member_ids, ['bob', 'me'], 'the owner takes the lock over for the current roster');
  // Nobody can know who received the deleted starter's Welcome, so the old
  // group is never declared complete: it is dropped and set up again.
  assert.ok(!app.fetches.some(item => item.url.endsWith('/conversations/g1/crypto-lock/complete')), 'the old setup is not completed');
  const resets = app.invokes.filter(item => item.command === 'crypto_apply_setup_generation' && item.args.generation === 1);
  assert.equal(resets.length, 1, 'the old group is dropped for the new setup generation');
  assert.equal(resets[0].args.conversationId, 'g1');
  assert.equal(app.$('#enableEncryptionButton').textContent, 'Continue encryption setup', 'the owner sets the group up again');
  assert.equal(app.$('#enableEncryptionButton').disabled, false);
  const admin = await deletedLockerHarness('admin');
  assert.ok(!admin.fetches.some(item => item.method === 'POST' && item.url.includes('/crypto-lock')), 'an admin does not while there is an owner');
});

test('a relay event of a newer setup generation drops the old group before it is processed', async () => {
  const base = 'https://a.example';
  const group = { id: 'g1', kind: 'group', display_name: 'Group' };
  const order = [];
  let holdsOldGroup = true;
  let localGeneration = 0;
  const prefix = `ghostchat.cryptoPeerTrusted.${base}.me.g1`;
  const app = harness({
    storage: { 'ghostchat.server': base, [prefix]: 'fp-old', [`ghostchat.cryptoPeerObserved.${base}.me.g1`]: 'fp-old' },
    credentials: { [await serverKey(base)]: 'token-A' },
    hooks: {
      crypto_has_direct_group: () => true,
      crypto_group_fingerprint: () => (holdsOldGroup ? 'fp-old' : 'fp-new'),
      crypto_get_event_cursor: () => 4,
      crypto_advance_event_cursor: args => args.sequence,
      crypto_apply_setup_generation: args => {
        order.push(`generation:${args.generation}`);
        if (args.generation <= localGeneration) return false;
        localGeneration = args.generation; holdsOldGroup = false; return true;
      },
      crypto_inspect_event: () => ({ kind: 'welcome' }),
      crypto_process_event: args => {
        order.push(`event:${args.eventSequence}`);
        return holdsOldGroup ? { kind: 'welcome_duplicate' } : { kind: 'welcome', peerIdentityFingerprint: 'fp-new' };
      },
    },
    fetchHandler: url => {
      if (url.endsWith('/v1/me')) return { body: { id: 'me', username: 'me', display_name: 'Me' } };
      if (url.endsWith('/v1/conversations')) return { body: [group] };
      // The lock was read just before the takeover; the events after it.
      if (url.endsWith('/conversations/g1/crypto-lock')) return { body: { locked: true, setupComplete: false, lockedBy: 'carol', deletedMemberIds: ['carol'], deletedMembers: [], lockedByDeletedAccount: true, setupGeneration: 0 } };
      if (url.endsWith('/conversations/g1/members')) return { body: [{ id: 'olga', role: 'owner', joined_seq: 1 }, { id: 'me', role: 'member', joined_seq: 2 }] };
      if (url.includes('/conversations/g1/crypto-events?after=4')) return { body: [{ sequence: 9, id: 'e9', sender_id: 'olga', recipient_id: 'me', payload: 'welcome', created_at: '', setup_generation: 1 }] };
      return { body: [] };
    },
  });
  await app.flush(() => app.sockets.length > 0);
  app.voiceOptions.voice.onIncoming({ conversationId: 'g1' });
  await app.flush(() => order.includes('event:9'));
  assert.deepEqual(order.filter(step => step !== 'generation:0'), ['generation:1', 'event:9'], 'the old group is dropped before the new Welcome is processed');
  assert.equal(app.store.get(prefix) ?? null, null, 'the old group\'s verified security code does not vouch for the new group');
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
      crypto_inspect_event: () => ({ kind: 'commit' }),
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

function pendingCommitHarness({ relayEvents, rowsAfter, inspect }) {
  return (async () => {
    const base = 'https://a.example';
    const group = { id: 'g1', kind: 'group', display_name: 'Group' };
    const order = [];
    const cached = new Set();
    let applied = 0;
    const app = harness({
      storage: { 'ghostchat.server': base, [`ghostchat.cryptoPeerTrusted.${base}.me.g1`]: 'fp-group' },
      credentials: { [await serverKey(base)]: 'token-A' },
      hooks: {
        crypto_has_direct_group: () => true,
        crypto_group_fingerprint: () => 'fp-group',
        crypto_get_event_cursor: () => 0,
        crypto_advance_event_cursor: args => args.sequence,
        crypto_get_cached_text_state: args => {
          // The cache is looked up for this account, conversation and sender.
          if (args.userId !== 'me' || args.conversationId !== 'g1' || args.senderId !== 'bob') order.push(`bad-cache-key:${JSON.stringify(args)}`);
          return { content: cached.has(args.clientMessageId) ? 'text' : null, undecryptable: false };
        },
        crypto_inspect_event: args => { order.push(`inspect:${args.eventSequence}`); return inspect(args); },
        crypto_decode_pending_history: args => { order.push(`history:${args.clientMessageId}`); cached.add(args.clientMessageId); return { kind: 'application', content: 'text' }; },
        crypto_process_event: args => {
          if (args.eventSequence == null) { order.push(`row:${args.clientMessageId}`); return { kind: 'application', content: 'text' }; }
          order.push(`relay:${args.eventSequence}`); applied = args.eventSequence;
          return { kind: relayEvents.find(event => event.sequence === args.eventSequence).kind };
        },
      },
      fetchHandler: url => {
        if (url.endsWith('/v1/me')) return { body: { id: 'me', username: 'me', display_name: 'Me' } };
        if (url.endsWith('/v1/conversations')) return { body: [group] };
        if (url.endsWith('/conversations/g1/crypto-lock')) return { body: { locked: true, setupComplete: true, lockedBy: 'bob', deletedMemberIds: [] } };
        if (url.endsWith('/conversations/g1/members')) return { body: [{ id: 'me', role: 'member' }, { id: 'bob', role: 'owner' }] };
        if (url.includes('/conversations/g1/crypto-events?after=0')) return { body: relayEvents.map(event => ({ sequence: event.sequence, id: `e${event.sequence}`, sender_id: 'bob', recipient_id: 'me', payload: event.kind, created_at: '' })) };
        if (url.includes('/conversations/g1/messages?')) return { body: rowsAfter(applied).map(id => ({ sequence: Number(id.slice(1)), id: `m-${id}`, conversation_id: 'g1', sender_id: 'bob', client_message_id: id, content: 'e2e1:cipher', created_at: '' })) };
        return { body: [] };
      },
    });
    await app.flush(() => app.sockets.length > 0);
    app.voiceOptions.voice.onIncoming({ conversationId: 'g1' });
    const last = relayEvents.at(-1).sequence;
    await app.flush(() => order.includes(`relay:${last}`));
    await app.flush();
    return order;
  })();
}

test('pending rows are decoded again before each later commit', async () => {
  // c7 arrives in epoch 0, commit 1, c8 arrives in epoch 1, commit 2.
  const order = await pendingCommitHarness({
    relayEvents: [{ sequence: 1, kind: 'commit' }, { sequence: 2, kind: 'commit' }],
    rowsAfter: applied => (applied >= 1 ? ['c7', 'c8'] : ['c7']),
    inspect: () => ({ kind: 'commit' }),
  });
  const at = item => order.indexOf(item);
  assert.ok(at('history:c7') >= 0 && at('history:c7') < at('relay:1'), `c7 before commit 1 (order: ${order.join(', ')})`);
  assert.ok(at('history:c8') > at('relay:1') && at('history:c8') < at('relay:2'), `c8 before commit 2 (order: ${order.join(', ')})`);
  assert.ok(!order.some(item => item.startsWith('bad-cache-key')), `cache key (order: ${order.join(', ')})`);
});

test('pending rows are decoded only before an authenticated commit', async () => {
  const order = await pendingCommitHarness({
    relayEvents: [{ sequence: 1, kind: 'application' }],
    rowsAfter: () => ['c7'],
    inspect: () => { throw new Error('GhostChat could not authenticate or decrypt this MLS event.'); },
  });
  assert.ok(order.includes('inspect:1'), `the relay event was inspected first (order: ${order.join(', ')})`);
  assert.ok(!order.some(item => item.startsWith('history:')), `a relay event that is not a valid commit must not trigger the local-epoch decode (order: ${order.join(', ')})`);
});

test('a row marked undecryptable is not decoded again', async () => {
  const base = 'https://a.example';
  const group = { id: 'g1', kind: 'group', display_name: 'Group' };
  const order = [];
  const app = harness({
    storage: { 'ghostchat.server': base, [`ghostchat.cryptoPeerTrusted.${base}.me.g1`]: 'fp-group' },
    credentials: { [await serverKey(base)]: 'token-A' },
    hooks: {
      crypto_has_direct_group: () => true,
      crypto_group_fingerprint: () => 'fp-group',
      crypto_get_event_cursor: () => 0,
      crypto_advance_event_cursor: args => args.sequence,
      crypto_get_cached_sent_text: () => null,
      crypto_get_cached_text_state: args => ({ content: null, undecryptable: args.clientMessageId === 'c6' }),
      crypto_inspect_event: () => ({ kind: 'commit' }),
      crypto_decode_pending_history: args => { order.push(`history:${args.clientMessageId}`); return { kind: 'application', content: 'text' }; },
      crypto_process_event: args => { order.push(args.eventSequence == null ? `row:${args.clientMessageId}` : `relay:${args.eventSequence}`); return { kind: 'commit' }; },
    },
    fetchHandler: url => {
      if (url.endsWith('/v1/me')) return { body: { id: 'me', username: 'me', display_name: 'Me' } };
      if (url.endsWith('/v1/conversations')) return { body: [group] };
      if (url.endsWith('/conversations/g1/crypto-lock')) return { body: { locked: true, setupComplete: true, lockedBy: 'bob', deletedMemberIds: [] } };
      if (url.endsWith('/conversations/g1/members')) return { body: [{ id: 'me', role: 'member' }, { id: 'bob', role: 'owner' }] };
      if (url.includes('/conversations/g1/crypto-events?after=0')) return { body: [{ sequence: 1, id: 'e1', sender_id: 'bob', recipient_id: 'me', payload: 'commit', created_at: '' }] };
      if (url.includes('/conversations/g1/messages?')) return { body: ['c6', 'c7'].map(id => ({ sequence: Number(id.slice(1)), id: `m-${id}`, conversation_id: 'g1', sender_id: 'bob', client_message_id: id, content: 'e2e1:cipher', created_at: '' })) };
      return { body: [] };
    },
  });
  await app.flush(() => app.sockets.length > 0);
  app.voiceOptions.voice.onIncoming({ conversationId: 'g1' });
  await app.flush(() => order.includes('relay:1'));
  await app.flush();
  assert.ok(order.includes('history:c7'), `the unmarked row is decoded (order: ${order.join(', ')})`);
  assert.ok(!order.includes('history:c6') && !order.includes('row:c6'), `a permanently undecryptable row is not retried (order: ${order.join(', ')})`);
});

test('the undecryptable marker is checked against the row payload', async () => {
  const base = 'https://a.example';
  const group = { id: 'g1', kind: 'group', display_name: 'Group' };
  const order = [];
  const app = harness({
    storage: { 'ghostchat.server': base, [`ghostchat.cryptoPeerTrusted.${base}.me.g1`]: 'fp-group' },
    credentials: { [await serverKey(base)]: 'token-A' },
    hooks: {
      crypto_has_direct_group: () => true,
      crypto_group_fingerprint: () => 'fp-group',
      crypto_get_event_cursor: () => 0,
      crypto_advance_event_cursor: args => args.sequence,
      crypto_get_cached_sent_text: () => null,
      // c6 was marked for a payload the server no longer delivers.
      crypto_get_cached_text_state: args => ({ content: null, undecryptable: args.clientMessageId === 'c6' && args.payload !== 'honest-c6' }),
      crypto_inspect_event: () => ({ kind: 'commit' }),
      crypto_decode_pending_history: args => { order.push(`history:${args.clientMessageId}`); return { kind: 'application', content: 'text' }; },
      crypto_process_event: args => { order.push(args.eventSequence == null ? `row:${args.clientMessageId}` : `relay:${args.eventSequence}`); return { kind: 'commit' }; },
    },
    fetchHandler: url => {
      if (url.endsWith('/v1/me')) return { body: { id: 'me', username: 'me', display_name: 'Me' } };
      if (url.endsWith('/v1/conversations')) return { body: [group] };
      if (url.endsWith('/conversations/g1/crypto-lock')) return { body: { locked: true, setupComplete: true, lockedBy: 'bob', deletedMemberIds: [] } };
      if (url.endsWith('/conversations/g1/members')) return { body: [{ id: 'me', role: 'member' }, { id: 'bob', role: 'owner' }] };
      if (url.includes('/conversations/g1/crypto-events?after=0')) return { body: [{ sequence: 1, id: 'e1', sender_id: 'bob', recipient_id: 'me', payload: 'commit', created_at: '' }] };
      if (url.includes('/conversations/g1/messages?')) return { body: ['c6', 'c7'].map(id => ({ sequence: Number(id.slice(1)), id: `m-${id}`, conversation_id: 'g1', sender_id: 'bob', client_message_id: id, content: `e2e1:honest-${id}`, created_at: '' })) };
      return { body: [] };
    },
  });
  await app.flush(() => app.sockets.length > 0);
  app.voiceOptions.voice.onIncoming({ conversationId: 'g1' });
  await app.flush(() => order.includes('relay:1'));
  await app.flush();
  assert.ok(order.includes('history:c6'), `a row whose marker was written for another payload is decoded (order: ${order.join(', ')})`);
  assert.ok(order.includes('history:c7'), `the unmarked row is decoded (order: ${order.join(', ')})`);
});

test('a pending-commit decode retries a row whose ordinary decode failed', async () => {
  const base = 'https://a.example';
  const group = { id: 'g1', kind: 'group', display_name: 'Group' };
  const order = [];
  const cached = new Set();
  let relayEvents = [];
  let rows = [];
  let releaseOrdinary;
  const ordinaryGate = new Promise(resolve => { releaseOrdinary = resolve; });
  const row = { sequence: 7, id: 'm7', conversation_id: 'g1', sender_id: 'bob', sender_name: 'Bob', client_message_id: 'c7', content: 'e2e1:cipher', created_at: '' };
  const app = harness({
    storage: { 'ghostchat.server': base, [`ghostchat.cryptoPeerTrusted.${base}.me.g1`]: 'fp-group' },
    credentials: { [await serverKey(base)]: 'token-A' },
    hooks: {
      crypto_has_direct_group: () => true,
      crypto_group_fingerprint: () => 'fp-group',
      crypto_get_event_cursor: () => 0,
      crypto_advance_event_cursor: args => args.sequence,
      crypto_get_cached_text_state: args => ({ content: cached.has(args.clientMessageId) ? 'text' : null, undecryptable: false }),
      crypto_inspect_event: () => ({ kind: 'commit' }),
      crypto_decode_pending_history: args => { order.push(`history:${args.clientMessageId}`); cached.add(args.clientMessageId); return { kind: 'application', content: 'text' }; },
      crypto_process_event: async args => {
        if (args.eventSequence == null) {
          order.push(`row:${args.clientMessageId}`);
          await ordinaryGate;
          throw new Error('The encrypted group does not match the current server roster.');
        }
        order.push(`relay:${args.eventSequence}`);
        return { kind: 'commit' };
      },
    },
    fetchHandler: url => {
      if (url.endsWith('/v1/me')) return { body: { id: 'me', username: 'me', display_name: 'Me' } };
      if (url.endsWith('/v1/conversations')) return { body: [group] };
      if (url.endsWith('/conversations/g1/crypto-lock')) return { body: { locked: true, setupComplete: true, lockedBy: 'bob', deletedMemberIds: [] } };
      if (url.endsWith('/conversations/g1/members')) return { body: [{ id: 'me', role: 'member' }, { id: 'bob', role: 'owner' }] };
      if (url.includes('/conversations/g1/crypto-events?after=0')) return { body: relayEvents };
      if (url.includes('/conversations/g1/messages?')) return { body: rows };
      return { body: [] };
    },
  });
  await app.flush(() => app.sockets.length > 0);
  app.voiceOptions.voice.onIncoming({ conversationId: 'g1' });
  await app.flush(() => app.fetches.some(item => item.url.includes('/conversations/g1/messages?')));
  await app.flush();
  const socket = app.sockets.at(-1);
  rows = [row];
  relayEvents = [{ sequence: 1, id: 'e1', sender_id: 'bob', recipient_id: 'me', payload: 'commit', created_at: '' }];
  // A live row starts an ordinary decode, which the changed roster will fail.
  socket.emit('message', { data: JSON.stringify({ type: 'message', message: row }) });
  await app.flush(() => order.includes('row:c7'));
  socket.emit('message', { data: JSON.stringify({ type: 'crypto_event', event: { conversation_id: 'g1' } }) });
  await app.flush(() => app.invokes.some(item => item.command === 'crypto_inspect_event'));
  await app.flush();
  releaseOrdinary();
  await app.flush(() => order.includes('relay:1'));
  assert.ok(order.includes('history:c7') && order.indexOf('history:c7') < order.indexOf('relay:1'), `the row is decoded in pending-commit mode before the commit (order: ${order.join(', ')})`);
});

test('an interrupted default-server credential migration is retried on the next launch', async () => {
  const legacy = 'https://185.158.151.179';
  const current = 'https://ghost-cassper.duckdns.org';
  const legacyKey = await serverKey(legacy);
  const failing = harness({
    storage: { 'ghostchat.server': legacy },
    credentials: { [legacyKey]: 'token-old' },
    hooks: { credential_set: () => { throw new Error('Credential Manager is busy'); } },
  });
  await failing.flush(() => failing.invokes.some(item => item.command === 'credential_get_pending_recovery' && item.args.serverKey !== legacyKey));
  assert.equal(failing.store.get('ghostchat.server'), legacy, 'the legacy address is kept until its credentials have moved');
  const setKeys = [];
  const retried = harness({
    storage: { 'ghostchat.server': failing.store.get('ghostchat.server') },
    credentials: { [legacyKey]: 'token-old' },
    hooks: { credential_set: args => { setKeys.push(args.serverKey); return true; } },
  });
  await retried.flush(() => retried.store.get('ghostchat.server') === current);
  assert.deepEqual(setKeys, [await serverKey(current)], 'the next launch moves the token to the new server key');
});

test('a Credential Manager read error keeps the user on a retryable screen', async () => {
  let fail = true;
  const app = harness({
    storage: { 'ghostchat.server': 'https://a.example' },
    hooks: { credential_get: () => { if (fail) throw new Error('Element not found. (os error 1168)'); return null; } },
  });
  // index.html starts with every view hidden.
  for (const view of ['#authView', '#recoveryView', '#appView']) app.$(view).classList.add('hidden');
  await app.flush(() => app.invokes.some(item => item.command === 'credential_get'));
  await app.flush();
  assert.ok(app.$('#authView').classList.contains('hidden'), 'a read error is not a logout');
  assert.ok(!app.$('#startupUpdateGate').classList.contains('hidden'), 'the startup gate explains the error');
  assert.ok(!app.$('#retryStartupUpdate').classList.contains('hidden'), 'and offers a retry');
  fail = false;
  app.$('#retryStartupUpdate').onclick();
  await app.flush(() => !app.$('#authView').classList.contains('hidden'));
  assert.ok(app.$('#startupUpdateGate').classList.contains('hidden'), 'a retry that finds no saved session shows the login');
});

test('no saved session still shows the login screen', async () => {
  const app = harness({ storage: { 'ghostchat.server': 'https://a.example' } });
  for (const view of ['#authView', '#recoveryView', '#appView']) app.$(view).classList.add('hidden');
  await app.flush(() => !app.$('#authView').classList.contains('hidden'));
  assert.ok(app.$('#startupUpdateGate').classList.contains('hidden'));
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
