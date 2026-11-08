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
    fetches.push({ url: String(url), method: options.method || 'GET', authorization: headers.get('Authorization') });
    const reply = fetchHandler ? await fetchHandler(String(url), options) : undefined;
    const { status = 200, body = [] } = reply || {};
    return { ok: status >= 200 && status < 300, status, json: async () => body };
  }
  const voiceStub = () => new Proxy({ getState: () => ({ status: 'idle', participants: [] }) }, {
    get(target, prop) { return prop in target ? target[prop] : () => Promise.resolve(); },
  });
  const window = {
    __TAURI__: { core: { invoke }, event: { listen: () => Promise.resolve(() => {}) } },
    GhostChatVoice: { create: voiceStub }, GhostChatGroupVoice: { create: voiceStub }, GhostChatWatchParty: { create: voiceStub },
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
  const flush = async (until = null, rounds = 400) => {
    for (let i = 0; i < rounds; i += 1) {
      if (until && until()) return;
      await new Promise(resolve => setImmediate(resolve));
    }
    if (until) throw new Error('condition was not reached');
  };
  const runTimers = () => { for (const timer of timers.splice(0)) if (!timer.done) { timer.done = true; timer.fn(); } };
  return { $, store, sockets, fetches, invokes, timers, flush, runTimers };
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
  await app.flush();
  void app.$('#revokeOtherSessions').onclick({ currentTarget: app.$('#revokeOtherSessions') });
  await app.flush();
  assert.equal(app.$('#appDecisionConfirm').textContent, 'Oturumları kapat', 'Turkish users must see the Turkish confirm label');
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
  await app.flush();
  app.runTimers();
  await app.flush();
  for (const socket of app.sockets.filter(item => item.url.startsWith('wss://b.example'))) {
    socket.emit('open');
    assert.ok(!socket.sent.some(data => data.includes('token-A')), 'server B must never receive server A token over WebSocket');
  }
  const leaked = app.fetches.filter(item => item.url.startsWith(B) && item.authorization === 'Bearer token-A');
  assert.deepEqual(leaked, [], 'server B must never receive server A token over HTTP');
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
