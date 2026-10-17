(() => {
  const $ = (selector, root = document) => root.querySelector(selector);
  const $$ = (selector, root = document) => [...root.querySelectorAll(selector)];
  const invoke = window.__TAURI__?.core?.invoke;
  const state = { localBase: "", base: "", token: "", user: null, pendingRecovery: "", page: "messages", conversations: [], friends: [], active: null, older: true, reconnect: 1200, socket: null, searchTab: "users", updateVersion: null, updateNotifiedVersion: null };

  function toast(message, error = false) {
    const node = $("#toast"); node.textContent = message; node.classList.toggle("error", error); node.classList.add("show");
    clearTimeout(toast.timer); toast.timer = setTimeout(() => node.classList.remove("show"), 3000);
  }
  async function checkForUpdates(quiet = false) {
    const statusNode = $("#updateStatus");
    const checkButton = $("#checkUpdates");
    if (statusNode) statusNode.textContent = "Checking for updates…";
    if (checkButton) checkButton.disabled = true;
    try {
      const result = await invoke("check_for_update");
      if (!result.configured) {
        if (statusNode) statusNode.textContent = `GitHub Releases are not connected yet. Current version: ${result.currentVersion}.`;
        $("#installUpdate")?.classList.add("hidden");
        return;
      }
      if (result.available) {
        const newlyAvailable = state.updateNotifiedVersion !== result.version;
        state.updateVersion = result.version;
        state.updateNotifiedVersion = result.version;
        if (statusNode) statusNode.textContent = `GhostChat ${result.version} is ready to install.`;
        const notes = $("#updateNotes");
        if (notes) {
          notes.textContent = result.notes || "This update includes the latest GhostChat improvements.";
          notes.classList.remove("hidden");
        }
        $("#installUpdate")?.classList.remove("hidden");
        if (newlyAvailable) toast(`GhostChat ${result.version} is ready to install.`);
      } else {
        state.updateVersion = null;
        state.updateNotifiedVersion = null;
        if (statusNode) statusNode.textContent = `You’re up to date (${result.currentVersion}).`;
        $("#updateNotes")?.classList.add("hidden");
        $("#installUpdate")?.classList.add("hidden");
        if (!quiet) toast("GhostChat is up to date.");
      }
    } catch (error) {
      if (statusNode) statusNode.textContent = "Could not check for updates. Try again when you’re online.";
      if (!quiet) toast(error.message || "Could not check for updates.", true);
    } finally {
      if (checkButton) checkButton.disabled = false;
    }
  }
  async function installUpdate() {
    if (!state.updateVersion) return;
    const button = $("#installUpdate");
    button.disabled = true;
    $("#updateStatus").textContent = "Preparing the update…";
    try { await invoke("install_update", { expectedVersion: state.updateVersion }); }
    catch (error) {
      $("#updateStatus").textContent = error.message || "The update could not be installed.";
      button.disabled = false;
      toast(error.message || "The update could not be installed.", true);
    }
  }
  function normalizedBase(value) { return value.replace(/\/+$/, ""); }
  function activeBase() { return state.base || state.localBase; }
  async function serverKey(base) {
    const data = new TextEncoder().encode(normalizedBase(base));
    const digest = await crypto.subtle.digest("SHA-256", data);
    return [...new Uint8Array(digest)].map(byte => byte.toString(16).padStart(2, "0")).join("");
  }
  async function request(path, options = {}) {
    const headers = new Headers(options.headers || {});
    headers.set("Content-Type", "application/json");
    if (state.token) headers.set("Authorization", `Bearer ${state.token}`);
    let response;
    try { response = await fetch(`${activeBase()}${path}`, { ...options, headers }); }
    catch { throw new Error("Could not reach the selected server. Check its address and connection."); }
    if (response.status === 204) return null;
    const body = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(body.error || `Request failed (${response.status}).`);
    return body;
  }
  async function storeToken(token) {
    state.token = token;
    try { await invoke("credential_set", { serverKey: await serverKey(activeBase()), token }); }
    catch (error) { console.warn("Credential Manager unavailable", error); toast("Session active, but Windows could not save it. Keep your recovery key safe.", true); }
  }
  async function restoreToken() {
    try { state.token = await invoke("credential_get", { serverKey: await serverKey(activeBase()) }) || ""; }
    catch { state.token = ""; }
  }
  async function clearToken() {
    const previous = state.token; state.token = "";
    try { if (previous) await invoke("credential_delete", { serverKey: await serverKey(activeBase()) }); } catch {}
  }
  async function savePendingRecovery(secret) {
    state.pendingRecovery = secret;
    try { await invoke("credential_set_pending_recovery", { serverKey: await serverKey(activeBase()), token: secret }); }
    catch (error) { console.warn("Could not safely retain pending recovery key", error); toast("Windows could not keep a temporary recovery backup. Save the key before closing this screen.", true); }
  }
  async function clearPendingRecovery() {
    try { await invoke("credential_delete_pending_recovery", { serverKey: await serverKey(activeBase()) }); } catch {}
    state.pendingRecovery = "";
  }
  function show(view) {
    for (const id of ["authView", "recoveryView", "appView"]) $(`#${id}`).classList.toggle("hidden", id !== view);
  }
  function updateServerLabels() {
    const remote = !!state.base;
    const label = remote ? new URL(state.base).host : "on this device";
    $("#serverLabel").textContent = label; $("#serverAddressInput").value = state.base;
  }
  function avatarText(value = "G") { return (value.trim().replace(/^@/, "")[0] || "G").toUpperCase(); }
  function create(tag, className, text) { const node = document.createElement(tag); if (className) node.className = className; if (text !== undefined) node.textContent = text; return node; }
  function setTheme() {
    const theme = localStorage.getItem("ghostchat.theme") || "dark";
    document.body.classList.toggle("light", theme === "light"); $("#themeSelect").value = theme;
    const size = localStorage.getItem("ghostchat.textSize") || "15";
    document.documentElement.style.fontSize = `${size}px`; $("#textSize").value = size;
  }
  function updateIdentity() {
    $("#accountName").textContent = state.user.display_name;
    $("#accountTag").textContent = `@${state.user.username}`;
    $("#accountAvatar").textContent = avatarText(state.user.display_name);
    $("#profileButton").textContent = avatarText(state.user.display_name);
  }
  async function enterApp() {
    try { state.user = await request("/v1/me"); }
    catch { await clearToken(); state.user = null; show("authView"); return; }
    show("appView"); updateIdentity(); setTheme(); setPage("messages");
    await refreshFriends(); await refreshConversations(); connectSocket();
    renderSidebar(); showWelcome();
  }
  function showWelcome() {
    if (state.page !== "messages" || state.active) return;
    $("#chatTitle").textContent = "Your quiet corner"; $("#chatSubtitle").textContent = "Choose a friend or find someone new.";
    $("#chatAvatar").textContent = "G"; $("#messagePanel").classList.add("hidden"); $("#pageContent").classList.remove("hidden");
    $("#pageContent").innerHTML = "";
    const card = create("div", "welcome-card");
    card.append(create("div", "welcome-orb", "◌"), create("h2", "", "A little more room to talk."), create("p", "", "Find someone by username, send a friend request, then start a private conversation. Your account does not need an email or phone number."));
    const actions = create("div", "welcome-actions");
    const find = create("button", "button primary", "Find people"); find.onclick = () => setPage("search");
    const friends = create("button", "button secondary", "View friends"); friends.onclick = () => setPage("friends");
    actions.append(find, friends); card.append(actions); $("#pageContent").append(card);
  }
  function setPage(page) {
    state.page = page; state.active = page === "messages" ? state.active : null;
    $$(".rail-button[data-page]").forEach(button => button.classList.toggle("active", button.dataset.page === page));
    const titles = { messages: "Messages", friends: "Friends", search: "Find people", searchMessages: "Search messages" };
    $("#sidebarTitle").textContent = titles[page]; $("#sidebarAction").title = page === "messages" ? "Start a conversation" : "Find people";
    if (page !== "messages") { $("#messagePanel").classList.add("hidden"); $("#pageContent").classList.remove("hidden"); renderPage(); }
    else if (state.active) openConversation(state.active.id, false);
    else showWelcome();
    renderSidebar();
  }
  function renderSidebar() {
    const root = $("#sidebarContent"); root.replaceChildren();
    if (state.page === "messages") {
      if (!state.conversations.length) { root.append(create("div", "empty-list", "No conversations yet. Find a friend to start a chat.")); return; }
      const label = create("div", "section-label", "DIRECT MESSAGES"); root.append(label);
      for (const convo of state.conversations) {
        const row = create("button", `list-row${state.active?.id === convo.id ? " active" : ""}`); row.onclick = () => openConversation(convo.id);
        row.append(create("div", "mini-avatar", avatarText(convo.display_name)));
        const copy = create("div", "list-copy"); copy.append(create("strong", "", convo.display_name), create("span", "", convo.last_message || `@${convo.username}`)); row.append(copy); root.append(row);
      }
      return;
    }
    if (state.page === "friends") {
      const requestsLabel = create("div", "section-label", "REQUESTS"); root.append(requestsLabel);
      for (const request of state.pending || []) { const row = create("button", "list-row"); row.append(create("div", "mini-avatar", avatarText(request.display_name))); const copy = create("div", "list-copy"); copy.append(create("strong", "", request.display_name), create("span", "", `@${request.username}`)); row.append(copy); root.append(row); }
      if (!(state.pending || []).length) root.append(create("div", "empty-list", "No pending requests."));
      root.append(create("div", "section-label", "YOUR FRIENDS"));
      for (const friend of state.friends) { const row = create("button", "list-row"); row.onclick = () => startConversation(friend.id); row.append(create("div", "mini-avatar", avatarText(friend.display_name))); const copy = create("div", "list-copy"); copy.append(create("strong", "", friend.display_name), create("span", "", `@${friend.username}`)); row.append(copy); root.append(row); }
      if (!state.friends.length) root.append(create("div", "empty-list", "Friends you add will show up here."));
      return;
    }
    root.append(create("div", "empty-list", "Search by username or user ID. GhostChat does not publish a public directory."));
  }
  function renderPage() {
    const root = $("#pageContent"); root.replaceChildren(); root.classList.remove("hidden");
    const page = create("div", "content-page");
    if (state.page === "search") {
      page.append(create("h2", "", "Find people"), create("p", "muted", "Search for a username or user ID. Users are not listed publicly."));
      const form = create("form", "search-form"); const input = create("input"); input.placeholder = "Search @username or user ID"; input.setAttribute("aria-label", "Search people"); input.minLength = 2;
      const submit = create("button", "button primary", "Search"); submit.type = "submit"; form.append(input, submit); page.append(form);
      const results = create("div", "search-results"); page.append(results);
      form.onsubmit = async event => { event.preventDefault(); if (input.value.trim().length < 2) return; results.replaceChildren(create("div", "empty-list", "Searching…")); try { const users = await request(`/v1/users/search?q=${encodeURIComponent(input.value.trim())}`); results.replaceChildren(); if (!users.length) results.append(create("div", "empty-list", "No users found. Check the spelling and try again.")); for (const user of users) { const card = create("div", "result-card"); card.append(create("div", "mini-avatar", avatarText(user.display_name))); const copy = create("div", "list-copy"); copy.append(create("strong", "", user.display_name), create("span", "", `@${user.username} · ${user.id.slice(0, 8)}`)); card.append(copy); const add = create("button", "button secondary", "Add friend"); add.onclick = async () => { try { await request("/v1/friends", { method: "POST", body: JSON.stringify({ user_id: user.id }) }); add.textContent = "Request sent"; add.disabled = true; toast("Friend request sent."); } catch (error) { toast(error.message, true); } }; card.append(add); results.append(card); } } catch (error) { results.replaceChildren(create("div", "empty-list", error.message)); } };
      setTimeout(() => input.focus(), 20);
    } else if (state.page === "friends") {
      page.append(create("h2", "", "Friends"), create("p", "muted", "Manage requests and start a private conversation."));
      const tabs = create("div", "tab-row");
      for (const [key, label] of [["all", "All friends"], ["pending", `Requests${(state.pending || []).length ? ` · ${(state.pending || []).length}` : ""}`]]) { const button = create("button", state.searchTab === key ? "active" : "", label); button.onclick = () => { state.searchTab = key; renderPage(); }; tabs.append(button); }
      page.append(tabs);
      const list = create("div", "friend-results"); page.append(list);
      if (state.searchTab === "pending") {
        if (!(state.pending || []).length) list.append(create("div", "empty-list", "No friend requests right now."));
        for (const item of state.pending || []) { const row = create("div", "request-card"); row.append(create("div", "mini-avatar", avatarText(item.display_name))); const copy = create("div", "list-copy"); copy.append(create("strong", "", item.display_name), create("span", "", `@${item.username}`)); row.append(copy); const actions = create("div", "request-actions"); const accept = create("button", "button primary", "Accept"); accept.onclick = () => respondFriend(item.id, "accept"); const reject = create("button", "button secondary", "Ignore"); reject.onclick = () => respondFriend(item.id, "reject"); actions.append(accept, reject); row.append(actions); list.append(row); }
      } else {
        if (!state.friends.length) list.append(create("div", "empty-list", "No friends yet. Use Find people to add someone."));
        for (const friend of state.friends) { const row = create("div", "result-card"); row.append(create("div", "mini-avatar", avatarText(friend.display_name))); const copy = create("div", "list-copy"); copy.append(create("strong", "", friend.display_name), create("span", "", `@${friend.username}`)); row.append(copy); const message = create("button", "button primary", "Message"); message.onclick = () => startConversation(friend.id); row.append(message); list.append(row); }
      }
    } else if (state.page === "searchMessages") {
      page.append(create("h2", "", "Search messages"), create("p", "muted", "Search messages from conversations you belong to."));
      const form = create("form", "search-form"); const input = create("input"); input.placeholder = "Search in your messages"; input.minLength = 2; const go = create("button", "button primary", "Search"); go.type = "submit"; form.append(input, go); page.append(form); const results = create("div", "search-results"); page.append(results);
      form.onsubmit = async event => { event.preventDefault(); try { const rows = await request(`/v1/messages/search?q=${encodeURIComponent(input.value.trim())}`); results.replaceChildren(); if (!rows.length) results.append(create("div", "empty-list", "No matching messages.")); for (const row of rows) { const card = create("button", "result-card"); card.style.textAlign = "left"; const copy = create("div", "list-copy"); copy.append(create("strong", "", row.sender_name), create("span", "", row.content)); card.append(copy); card.onclick = () => { setPage("messages"); openConversation(row.conversation_id); }; results.append(card); } } catch (error) { toast(error.message, true); } };
      setTimeout(() => input.focus(), 20);
    }
    root.append(page); $("#messagePanel").classList.add("hidden");
  }
  async function refreshFriends() {
    try { [state.friends, state.pending] = await Promise.all([request("/v1/friends"), request("/v1/friends/requests")]); } catch (error) { console.warn(error); }
    renderSidebar(); if (state.page === "friends") renderPage();
  }
  async function refreshConversations() {
    try { state.conversations = await request("/v1/conversations"); } catch (error) { console.warn(error); }
    renderSidebar();
  }
  async function respondFriend(id, action) {
    try { await request(`/v1/friends/requests/${encodeURIComponent(id)}/${action}`, { method: "POST", body: "{}" }); await refreshFriends(); toast(action === "accept" ? "Friend request accepted." : "Request removed."); }
    catch (error) { toast(error.message, true); }
  }
  async function startConversation(userId) {
    try { const convo = await request("/v1/conversations/direct", { method: "POST", body: JSON.stringify({ user_id: userId }) }); await refreshConversations(); setPage("messages"); await openConversation(convo.id); }
    catch (error) { toast(error.message, true); }
  }
  async function openConversation(id, refresh = true) {
    state.page = "messages"; $("#messagePanel").classList.remove("hidden"); $("#pageContent").classList.add("hidden");
    if (refresh) { $$(".rail-button[data-page]").forEach(button => button.classList.toggle("active", button.dataset.page === "messages")); }
    const convo = state.conversations.find(item => item.id === id);
    if (!convo) { await refreshConversations(); }
    state.active = state.conversations.find(item => item.id === id) || { id, display_name: "Conversation", username: "" };
    $("#chatTitle").textContent = state.active.display_name; $("#chatSubtitle").textContent = `@${state.active.username}`; $("#chatAvatar").textContent = avatarText(state.active.display_name);
    await loadMessages(false);
  }
  function timeLabel(value) {
    const date = new Date(value); if (Number.isNaN(date.getTime())) return "";
    return new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" }).format(date);
  }
  function appendMessage(message, prepend = false) {
    const container = $("#messages"); if ($(`[data-message-id="${CSS.escape(message.id)}"]`, container)) return;
    const row = create("article", "message"); row.dataset.messageId = message.id; row.dataset.sequence = String(message.sequence);
    row.append(create("div", "mini-avatar", avatarText(message.sender_name)));
    const main = create("div", "message-main"); const meta = create("div", "message-meta"); meta.append(create("strong", "", message.sender_name)); const time = create("time", "", timeLabel(message.created_at)); time.dateTime = message.created_at; meta.append(time); if (message.edited_at) meta.append(create("span", "optional", "edited"));
    main.append(meta, create("div", "message-body", message.content)); row.append(main);
    if (message.sender_id === state.user.id) {
      const actions = create("div", "message-actions"); const edit = create("button", "", "Edit"); edit.onclick = () => editMessage(message); const remove = create("button", "", "Delete"); remove.onclick = () => deleteMessage(message.id); actions.append(edit, remove); row.append(actions);
    }
    if (prepend) container.prepend(row); else container.append(row);
  }
  async function loadMessages(older = false) {
    if (!state.active) return;
    const container = $("#messages"); const oldHeight = container.scrollHeight; const first = container.firstElementChild;
    let path = `/v1/conversations/${encodeURIComponent(state.active.id)}/messages?limit=50`;
    if (older && first) path += `&before=${encodeURIComponent(first.dataset.sequence || "")}`;
    try {
      const rows = await request(path); if (!older) container.replaceChildren();
      for (const row of rows) appendMessage(row, older);
      state.older = rows.length === 50; $("#loadOlderRow").classList.toggle("hidden", !state.older || !rows.length);
      if (older) container.scrollTop += container.scrollHeight - oldHeight; else container.scrollTop = container.scrollHeight;
      await refreshConversations();
    } catch (error) { toast(error.message, true); }
  }
  async function sendMessage(content, clientId = crypto.randomUUID()) {
    if (!state.active || !content.trim()) return;
    try { const message = await request(`/v1/conversations/${encodeURIComponent(state.active.id)}/messages`, { method: "POST", body: JSON.stringify({ client_message_id: clientId, content }) }); appendMessage(message); $("#messages").scrollTop = $("#messages").scrollHeight; await refreshConversations(); }
    catch (error) { toast(error.message, true); }
  }
  async function editMessage(message) {
    const content = prompt("Edit message", message.content); if (content === null || content.trim() === message.content) return;
    try { await request(`/v1/messages/${encodeURIComponent(message.id)}`, { method: "PATCH", body: JSON.stringify({ content }) }); await loadMessages(false); }
    catch (error) { toast(error.message, true); }
  }
  async function deleteMessage(id) {
    if (!confirm("Delete this message?")) return;
    try { await request(`/v1/messages/${encodeURIComponent(id)}`, { method: "DELETE" }); const row = $(`[data-message-id="${CSS.escape(id)}"]`); row?.remove(); await refreshConversations(); }
    catch (error) { toast(error.message, true); }
  }
  function setConnection(online) {
    const node = $("#connectionState"); node.classList.toggle("offline", !online); node.innerHTML = `<i class="status-dot ${online ? "" : "offline"}"></i>${online ? "Connected" : "Reconnecting"}`;
  }
  function connectSocket() {
    if (state.socket) { try { state.socket.close(); } catch {} }
    const url = activeBase().replace(/^http:/, "ws:").replace(/^https:/, "wss:");
    try { state.socket = new WebSocket(`${url}/v1/ws`); } catch { setConnection(false); scheduleReconnect(); return; }
    const socket = state.socket;
    socket.addEventListener("open", () => { if (socket === state.socket) socket.send(JSON.stringify({ token: state.token })); });
    socket.addEventListener("message", event => {
      if (socket !== state.socket) return;
      let data; try { data = JSON.parse(event.data); } catch { return; }
      if (data.type === "ready") { state.reconnect = 1200; setConnection(true); refreshConversations(); if (state.active) loadMessages(false); }
      else if (data.type === "session_revoked") { state.token = ""; state.user = null; state.active = null; if (state.socket) state.socket.close(); void (async () => { try { await invoke("credential_delete", { serverKey: await serverKey(activeBase()) }); } catch {} })(); show("authView"); toast("This session was revoked. Recover your account to sign in again.", true); }
      else if (data.type === "message" && data.message) { if (state.active?.id === data.message.conversation_id) { appendMessage(data.message); $("#messages").scrollTop = $("#messages").scrollHeight; } refreshConversations(); }
      else if (data.type === "deleted" || data.type === "refresh") { if (state.active) loadMessages(false); }
    });
    socket.addEventListener("close", () => { if (socket === state.socket) { setConnection(false); scheduleReconnect(); } });
    socket.addEventListener("error", () => { try { socket.close(); } catch {} });
  }
  function scheduleReconnect() {
    clearTimeout(connectSocket.timer); connectSocket.timer = setTimeout(() => { if (state.token) connectSocket(); }, state.reconnect);
    state.reconnect = Math.min(30000, state.reconnect * 1.7);
  }
  async function authenticateReply(reply) {
    await storeToken(reply.session_token); state.user = reply.user;
    if (reply.recovery_token) { await savePendingRecovery(reply.recovery_token); $("#recoveryValue").textContent = reply.recovery_token; $("#recoveryConfirm").checked = false; $("#continueAfterRecovery").disabled = true; show("recoveryView"); }
    else { await clearPendingRecovery(); await enterApp(); }
  }
  function setAuthTab(tab) {
    $$(".auth-tab").forEach(button => button.classList.toggle("active", button.dataset.authTab === tab));
    $("#createForm").classList.toggle("hidden", tab !== "create"); $("#recoverForm").classList.toggle("hidden", tab !== "recover");
  }

  $$(".auth-tab").forEach(button => button.addEventListener("click", () => setAuthTab(button.dataset.authTab)));
  $("#createForm").addEventListener("submit", async event => {
    event.preventDefault(); const form = new FormData(event.currentTarget); const button = event.currentTarget.querySelector("button[type=submit]"); button.disabled = true;
    try { const reply = await request("/v1/accounts", { method: "POST", body: JSON.stringify({ display_name: form.get("display_name"), username: form.get("username") || null }) }); await authenticateReply(reply); }
    catch (error) { toast(error.message, true); } finally { button.disabled = false; }
  });
  $("#recoverForm").addEventListener("submit", async event => {
    event.preventDefault(); const form = new FormData(event.currentTarget); const button = event.currentTarget.querySelector("button[type=submit]"); button.disabled = true;
    try { const reply = await request("/v1/sessions/recover", { method: "POST", body: JSON.stringify({ recovery_token: form.get("recovery_token").trim() }) }); await authenticateReply(reply); }
    catch (error) { toast(error.message, true); } finally { button.disabled = false; }
  });
  $("#copyRecovery").onclick = async () => { try { await navigator.clipboard.writeText($("#recoveryValue").textContent); toast("Recovery key copied."); } catch { toast("Copy was blocked. Select and copy the key manually.", true); } };
  $("#downloadRecovery").onclick = () => { const text = `GhostChat recovery key\n\n${$("#recoveryValue").textContent}\n\nKeep this key private. Anyone with it can recover the account. GhostChat cannot reset it.`; const url = URL.createObjectURL(new Blob([text], { type: "text/plain" })); const link = create("a"); link.href = url; link.download = "ghostchat-recovery.txt"; link.click(); URL.revokeObjectURL(url); };
  $("#recoveryConfirm").onchange = event => { $("#continueAfterRecovery").disabled = !event.target.checked; };
  $("#continueAfterRecovery").onclick = async () => { await clearPendingRecovery(); if (state.token) await enterApp(); else show("authView"); };
  $$(".rail-button[data-page]").forEach(button => button.addEventListener("click", () => setPage(button.dataset.page)));
  $("#sidebarAction").onclick = () => setPage("search");
  $("#searchInChat").onclick = () => { state.searchTab = "users"; setPage("searchMessages"); renderPage(); };
  $("#composer").addEventListener("submit", async event => { event.preventDefault(); const input = $("#messageInput"); const content = input.value; if (!content.trim()) return; input.value = ""; input.style.height = "auto"; await sendMessage(content); });
  $("#messageInput").addEventListener("keydown", event => { if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); $("#composer").requestSubmit(); } });
  $("#messageInput").addEventListener("input", event => { event.target.style.height = "auto"; event.target.style.height = `${Math.min(event.target.scrollHeight, 180)}px`; });
  $("#loadOlder").onclick = () => loadMessages(true);
  $("#settingsButton").onclick = () => { $("#serverAddressInput").value = state.base; $("#settingsDialog").showModal(); };
  $("#serverSettings").onclick = () => { $("#serverAddressInput").value = state.base; $("#settingsDialog").showModal(); };
  $("#saveSettings").onclick = async () => {
    const input = $("#serverAddressInput").value.trim();
    if (input) {
      let url; try { url = new URL(input); } catch { toast("Enter a valid server URL.", true); return; }
      if (url.protocol !== "https:" && !(url.protocol === "http:" && ["localhost", "127.0.0.1"].includes(url.hostname))) { toast("Remote servers must use HTTPS.", true); return; }
      const target = normalizedBase(url.toString());
      try { const response = await fetch(`${target}/health`); if (!response.ok) throw new Error(); }
      catch { toast("The server health check failed. Verify the address and try again.", true); return; }
      state.base = target; localStorage.setItem("ghostchat.server", target);
    } else { state.base = ""; localStorage.removeItem("ghostchat.server"); }
    const theme = $("#themeSelect").value; localStorage.setItem("ghostchat.theme", theme); document.body.classList.toggle("light", theme === "light");
    localStorage.setItem("ghostchat.textSize", $("#textSize").value); document.documentElement.style.fontSize = `${$("#textSize").value}px`;
    await restoreToken(); updateServerLabels(); $("#settingsDialog").close();
    if (state.token) await enterApp(); else show("authView");
    toast("Settings saved.");
  };
  $("#exportSettings").onclick = () => { const data = { server: state.base || "local", theme: localStorage.getItem("ghostchat.theme") || "dark", textSize: localStorage.getItem("ghostchat.textSize") || "15" }; const url = URL.createObjectURL(new Blob([JSON.stringify(data, null, 2)], { type: "application/json" })); const link = create("a"); link.href = url; link.download = "ghostchat-preferences.json"; link.click(); URL.revokeObjectURL(url); };
  $("#exportAccount").onclick = async () => { try { const data = await request("/v1/data/export"); const url = URL.createObjectURL(new Blob([JSON.stringify(data, null, 2)], { type: "application/json" })); const link = create("a"); link.href = url; link.download = "ghostchat-account-export.json"; link.click(); URL.revokeObjectURL(url); toast("Account data exported."); } catch (error) { toast(error.message, true); } };
  $("#deleteAccount").onclick = async () => { if (!confirm("This permanently deletes your account, sessions, friend links, conversations, and stored messages. This cannot be undone.")) return; if (prompt('Type DELETE to permanently erase this account:') !== "DELETE") return; try { await request("/v1/me/account", { method: "DELETE" }); if (state.socket) state.socket.close(); await clearToken(); await clearPendingRecovery(); state.user = null; state.active = null; state.friends = []; state.conversations = []; $("#settingsDialog").close(); show("authView"); toast("Account and data deleted."); } catch (error) { toast(error.message, true); } };
  $("#logoutButton").onclick = async () => { try { await request("/v1/sessions", { method: "DELETE" }); } catch {} if (state.socket) state.socket.close(); await clearToken(); state.user = null; state.active = null; state.conversations = []; show("authView"); toast("You are logged out."); };
  $("#profileButton").onclick = () => { $("#settingsDialog").showModal(); };
  $("#checkUpdates").onclick = () => checkForUpdates(false);
  $("#installUpdate").onclick = installUpdate;
  window.__TAURI__?.event?.listen?.("ghostchat:update-progress", event => {
    const percent = Number(event.payload);
    if (Number.isFinite(percent)) $("#updateStatus").textContent = `Downloading update… ${Math.min(100, Math.max(0, percent))}%.`;
  });
  document.addEventListener("keydown", event => { if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") { event.preventDefault(); setPage("search"); } if (event.key === "Escape" && !$("#settingsDialog").open) document.activeElement?.blur(); });

  async function boot() {
    setTheme();
    try { state.localBase = await invoke("api_base"); }
    catch { state.localBase = "http://127.0.0.1:38241"; }
    state.base = normalizedBase(localStorage.getItem("ghostchat.server") || ""); updateServerLabels();
    await restoreToken();
    try { state.pendingRecovery = await invoke("credential_get_pending_recovery", { serverKey: await serverKey(activeBase()) }) || ""; } catch {}
    if (state.pendingRecovery) {
      $("#recoveryValue").textContent = state.pendingRecovery;
      $("#recoveryConfirm").checked = false; $("#continueAfterRecovery").disabled = true; show("recoveryView");
    } else if (state.token) await enterApp(); else show("authView");
  }
  boot();
  setTimeout(() => checkForUpdates(true), 8000);
  setInterval(() => checkForUpdates(true), 6 * 60 * 60 * 1000);
})();
