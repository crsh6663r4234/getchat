// Visual check for the UI in both themes. Not part of the test suite: it needs
// Playwright and a browser (see the environment notes), so it is run by hand:
//   OUT=/some/dir/prefix node scripts/visual-check.cjs
// Without OUT the screenshots go to the system temp folder, never the repository.
// It loads crates/desktop/ui/index.html with a stubbed backend (mock API and a
// test double of the group call client) and writes one screenshot per theme.
const { chromium } = require('playwright');
const API = 'http://127.0.0.1:38241';
const me = { id: 'me', username: 'sen', display_name: 'Sen', avatar_data: null, presence: 'online' };
const convo = { id: 'c1', kind: 'group', display_name: 'Ev grubu', name: 'Ev grubu', members: [], member_count: 3, unread_count: 0, last_message: null, created_at: '2026-11-20T10:00:00Z' };
(async () => {
  const browser = await chromium.launch({ args: ['--no-sandbox'] });
  for (const theme of ['signage', 'plain']) {
    const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
    page.on("pageerror", e => console.log("PAGEERROR", e.message.slice(0,200)));
    page.on("console", m => { if (m.type()==="error") console.log("CONSOLE", m.text().slice(0,200)); });
    await page.route("**/group-voice-client.js", r => r.fulfill({ contentType: "application/javascript", body: "window.GhostChatGroupVoice = { create(api) { window.__gv = api; return new Proxy({}, { get: (t, k) => (k === \"then\" ? undefined : () => undefined) }); } };" }));
    await page.route(`${API}/**`, async route => {
      const url = route.request().url();
      const body = url.endsWith('/v1/me') ? me : url.includes('/v1/conversations/c1/messages') ? [] : url.endsWith('/v1/conversations') ? [convo] : [];
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(body) });
    });
    await page.addInitScript(() => {
      localStorage.setItem("ghostchat.server", "__ghostchat_local__");
      window.__TAURI__ = { core: { invoke: async (cmd) => {
        if (cmd === 'api_base') return 'http://127.0.0.1:38241';
        if (cmd === 'credential_get') return 'test-token';
        if (cmd === 'get_start_at_login') return false;
        if (cmd === 'check_for_update') return null;
        return null;
      } }, event: { listen: async () => () => {} } };
      window.GhostChatGroupVoice = { create(api) { window.__gv = api; return new Proxy({}, { get: (t, k) => (k === 'then' ? undefined : () => undefined) }); } };
    });
    await page.goto('file://' + require('path').resolve(__dirname, '../crates/desktop/ui/index.html'));
    await page.waitForTimeout(800);
    await page.evaluate((t) => { document.body.dataset.theme = t; document.querySelector('#startupUpdateGate')?.classList.add('hidden'); }, theme);
    const row = page.locator('#sidebarContent .list-row').first();
    if (await row.count()) { await row.click().catch(() => {}); }
    await page.waitForTimeout(500);
    await page.evaluate(() => {
      const p = [
        { id: 'a', identity: 'a', name: 'Ayşe Kaya', avatar: null, local: false, speaking: true, micMuted: false, micManuallyMuted: false, pushToTalkPressed: false, screenSharing: false, mutedForYou: false },
        { id: 'm', identity: 'm', name: 'Mert Demir', avatar: null, local: false, speaking: false, micMuted: false, micManuallyMuted: false, pushToTalkPressed: false, screenSharing: false, mutedForYou: false },
        { id: 'e', identity: 'e', name: 'Ece', avatar: null, local: false, speaking: false, micMuted: true, micManuallyMuted: true, pushToTalkPressed: false, screenSharing: false, mutedForYou: false },
      ];
      window.__gv?.onChange({ status: 'connected', conversationId: 'c1', participants: p, micMuted: false, micManuallyMuted: false, pushToTalkPressed: false, micAvailable: true, screenSharing: false, screenStarting: false, error: '', mediaPath: 'server', audioProcessing: null, localAudioStream: null });
    });
    await page.waitForTimeout(700);
    await page.screenshot({ path: `${process.env.OUT || require('os').tmpdir() + '/ghostchat-visual'}-${theme}.png` });
    const info = await page.evaluate(() => ({ rows: document.querySelectorAll('#sidebarContent .list-row').length, title: document.querySelector('#chatTitle')?.textContent, participants: document.querySelectorAll('#groupVoiceParticipants .voice-participant-card').length, gvHidden: document.querySelector('#groupVoiceParticipants')?.className }));
    console.log(theme, JSON.stringify(info));
    await page.close();
  }
  await browser.close();
})().catch(e => { console.error(e); process.exit(1); });
