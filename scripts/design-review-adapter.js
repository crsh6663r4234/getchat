// Injected only by design-review-server.mjs. Never included in the Tauri bundle.
// This exercises real UI rendering/handlers, not real credentials, transport or permissions.
(() => {
  const params = new URLSearchParams(location.search);
  const view = params.get('view') || 'dm';
  const tr = params.get('lang') === 'tr';
  const empty = view === 'empty';
  const me = { id: 'demo-casper', display_name: 'Casper', username: 'casper_4821', avatar_data: null };
  const mira = { id: 'demo-mira', display_name: 'Mira', username: 'mira_7192', avatar_data: null };
  const jules = { id: 'demo-jules', display_name: 'Jules', username: 'jules_2608', avatar_data: null };
  const conversations = [
    { id: 'demo-direct', kind: 'direct', display_name: 'Mira', username: mira.username, peer_id: mira.id, member_count: 2, unread_count: 1, peer_read_sequence: 2, last_message: tr ? 'Akşam görüşürüz ✨' : 'See you tonight ✨' },
    { id: 'demo-group', kind: 'group', display_name: tr ? 'Hafta sonu' : 'Weekend crew', member_count: 3, member_names: 'Casper, Mira, Jules', last_message: tr ? 'Birlikte planlayalım' : 'Let’s make a plan' }
  ];
  const messages = [
    { id: 'demo-message-1', sender_id: mira.id, sender_name: 'Mira', content: tr ? 'Yeni tasarımı birlikte inceleyelim mi? Ekranımı paylaşabilirim.' : 'Shall we look at the new design together? I can share my screen.' },
    { id: 'demo-message-2', sender_id: me.id, sender_name: 'Casper', content: tr ? 'Olur, aramaya geçelim. 🎧' : 'Sure, let’s jump into a call. 🎧' },
    { id: 'demo-message-3', sender_id: mira.id, sender_name: 'Mira', content: tr ? 'Tamam! Mikrofonun yoksa sessiz katılabilirsin.' : 'Sounds good! You can join muted if you don’t have a microphone.' }
  ].map((message, index) => ({ ...message, conversation_id: 'demo-direct', sequence: index + 1, created_at: `2026-10-31T18:0${index}:00Z` }));
  localStorage.setItem('ghostchat.server', '__ghostchat_local__');
  localStorage.setItem('ghostchat.language', tr ? 'tr' : 'en');
  localStorage.setItem('ghostchat.theme', params.get('theme') === 'light' ? 'light' : 'dark');
  localStorage.setItem('ghostchat.desktopNotifications', 'false');
  document.title = 'GhostChat — synthetic design review';
  let token = ['auth', 'recovery', 'update-error'].includes(view) ? '' : 'demo-session-only';
  window.__TAURI__ = {
    core: { invoke: async (command, args = {}) => {
      if (command === 'api_base') return location.origin;
      if (command === 'check_for_update') {
        if (view === 'update-error') throw new Error(tr ? 'Güncelleme sunucusuna ulaşılamadı. Tekrar dene.' : 'The update server could not be reached. Try again.');
        return { configured: true, available: false, currentVersion: '4.0.0' };
      }
      if (command === 'credential_get') return token;
      if (command === 'credential_set') { token = args.token; return; }
      if (command === 'credential_delete') { token = ''; return; }
      if (command === 'credential_get_pending_recovery') return view === 'recovery' ? 'DEMO-RECOVERY-KEY-NOT-A-REAL-ACCOUNT' : '';
      if (command === 'credential_get_recovery') return 'DEMO-RECOVERY-KEY-NOT-A-REAL-ACCOUNT';
      if (command.startsWith('credential_') || command === 'show_desktop_notification') return;
      if (command === 'get_start_at_login' || command === 'crypto_has_direct_group') return false;
      if (command === 'take_pending_group_invites') return [];
      throw new Error('This native action is unavailable in the synthetic design review.');
    } },
    event: { listen: async () => () => {} }
  };
  window.fetch = async (input, options = {}) => {
    const url = new URL(typeof input === 'string' ? input : input.url, location.origin);
    if (url.origin !== location.origin) throw new Error('External requests are disabled in the design review.');
    const pathname = url.pathname;
    let body;
    if (pathname === '/health') body = { status: 'ok', service: 'ghostchat' };
    else if (pathname === '/v1/me') body = me;
    else if (pathname === '/v1/me/blocks') body = [];
    else if (pathname === '/v1/friends') body = empty ? [] : [mira, jules];
    else if (pathname === '/v1/friends/requests') body = empty ? [] : [{ id: 'demo-request', display_name: 'Deniz', username: 'deniz_3408' }];
    else if (pathname === '/v1/conversations') body = empty ? [] : conversations;
    else if (pathname === '/v1/users/search') body = url.searchParams.get('q')?.includes('missing') ? [] : [jules];
    else if (pathname.endsWith('/messages')) {
      if (view === 'history-error') return new Response(JSON.stringify({ error: tr ? 'Mesajlar yüklenemedi.' : 'Messages could not be loaded.' }), { status: 503 });
      body = options.method === 'POST' ? { ...messages[1], id: crypto.randomUUID(), content: JSON.parse(options.body).content } : messages;
    }
    else if (pathname.endsWith('/reactions') || pathname.endsWith('/invites') || pathname.includes('/crypto/devices')) body = [];
    else if (pathname.endsWith('/members')) body = [{ ...me, role: 'owner' }, { ...mira, role: 'member' }, { ...jules, role: 'member' }];
    else if (pathname.endsWith('/crypto-lock')) body = { locked: false, setupComplete: false };
    else if (pathname.endsWith('/read')) body = {};
    else if (pathname === '/v1/voice/ice') body = { iceServers: [], relayAvailable: false };
    else if (pathname === '/v1/voice/config') body = { configured: false, available: false };
    else return new Response(JSON.stringify({ error: 'This action is not part of the visual fixture.' }), { status: 404 });
    return new Response(JSON.stringify(body), { headers: { 'Content-Type': 'application/json' } });
  };
  class PreviewSocket extends EventTarget {
    static OPEN = 1;
    readyState = 1;
    constructor() { super(); queueMicrotask(() => this.dispatchEvent(new Event('open'))); }
    send(raw) { if (JSON.parse(raw).token) queueMicrotask(() => this.dispatchEvent(new MessageEvent('message', { data: '{"type":"ready"}' }))); }
    close() { this.readyState = 3; }
  }
  window.WebSocket = PreviewSocket;
  // Fake call states cannot request microphone/screen permissions or establish connections.
  function fakeVoice(options) {
    let state = { status: 'idle', micAvailable: false, micMuted: true, speakerMuted: false, remoteAudioStream: null, remoteVideoStream: null, localScreenStream: null, participants: [] };
    const change = update => { state = { ...state, ...update }; options.onChange(state); };
    return new Proxy({
      getState: () => state,
      start: async conversation => change({ status: view === 'incoming' ? 'incoming' : 'connected', conversationId: conversation.id, peerId: conversation.peer_id }),
      accept: async () => change({ status: 'connected' }),
      decline: () => change({ status: 'idle' }),
      hangup: () => change({ status: 'idle' }),
      leave: async () => change({ status: 'idle' }),
      toggleMicrophone: async () => change({ micAvailable: true, micMuted: !state.micMuted }),
      toggleScreenSharing: async () => change({ screenSharing: !state.screenSharing })
    }, { get: (target, key) => target[key] || (async () => {}) });
  }
  window.GhostChatVoice = { create: fakeVoice };
  window.GhostChatGroupVoice = { create: fakeVoice };
  if (navigator.mediaDevices) {
    navigator.mediaDevices.enumerateDevices = async () => [];
    navigator.mediaDevices.getUserMedia = async () => { throw new DOMException('Design review does not capture audio.', 'NotFoundError'); };
    navigator.mediaDevices.getDisplayMedia = async () => { throw new DOMException('Design review does not capture screens.', 'NotAllowedError'); };
  }
  function waitFor(selector) {
    return new Promise(resolve => {
      const found = document.querySelector(selector);
      if (found) { resolve(found); return; }
      const observer = new MutationObserver(() => {
        const node = document.querySelector(selector);
        if (node) { observer.disconnect(); resolve(node); }
      });
      observer.observe(document.body, { childList: true, subtree: true, attributes: true });
    });
  }
  window.addEventListener('load', async () => {
    if (['auth', 'recovery', 'update-error', 'empty'].includes(view)) return;
    const conversation = await waitFor('#sidebarContent .list-row');
    if (['friends', 'people', 'message-search', 'welcome'].includes(view)) {
      if (view !== 'welcome') document.querySelector(view === 'message-search' ? '#searchInChat' : `[data-page="${view === 'people' ? 'search' : 'friends'}"]`).click();
      return;
    }
    conversation.click();
    await waitFor('#messages .message');
    if (['call', 'incoming', 'call-dock'].includes(view)) {
      document.querySelector('#voiceCallButton').click();
      if (view === 'call-dock') document.querySelector('[data-page="friends"]').click();
    }
    const actions = { settings: '#settingsButton', 'new-conversation': '#sidebarAction', safety: '#userSafetyButton' };
    if (actions[view]) document.querySelector(actions[view]).click();
    if (['create-group', 'join-group'].includes(view)) {
      document.querySelector('#sidebarAction').click();
      document.querySelector(view === 'create-group' ? '#createGroupAction' : '#joinGroupAction').click();
    }
  });
})();
