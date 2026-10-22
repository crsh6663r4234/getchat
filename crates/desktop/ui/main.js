(() => {
  const DEFAULT_REMOTE_SERVER = "https://ghost-cassper.duckdns.org";
  const LEGACY_DEFAULT_REMOTE_SERVER = "https://185.158.151.179";
  const LOCAL_SERVER_SENTINEL = "__ghostchat_local__";
  const $ = (selector, root = document) => root.querySelector(selector);
  const $$ = (selector, root = document) => [...root.querySelectorAll(selector)];
  const invoke = window.__TAURI__?.core?.invoke;
  const state = { localBase: "", base: "", token: "", user: null, pendingRecovery: "", page: "messages", conversations: [], friends: [], pending: [], seenFriendRequestIds: new Set(), active: null, older: true, reconnect: 1200, socket: null, searchTab: "users", updateVersion: null, updateNotifiedVersion: null };
  let signupAvatarData = null;
  let profileAvatarData = null;
  const trText = {
    "Create account": "Hesap oluştur", "Recover": "Kurtar", "Name": "Ad", "Display name": "Görünen ad", "Profile picture": "Profil fotoğrafı", "Choose": "Seç", "Create anonymous account": "Anonim hesap oluştur", "Recover account": "Hesabı kurtar", "Recovery key": "Kurtarma anahtarı", "Your recovery key": "Kurtarma anahtarın", "Copy key": "Anahtarı kopyala", "Save backup": "Yedek kaydet", "Continue to GhostChat": "GhostChat'e devam et", "Messages": "Mesajlar", "Friends": "Arkadaşlar", "Find people": "Kişi bul", "Search messages": "Mesajlarda ara", "Settings": "Ayarlar", "Profile": "Profil", "Connection": "Bağlantı", "Appearance": "Görünüm", "Notifications": "Bildirimler", "Voice and screen sharing": "Sesli sohbet ve ekran paylaşımı", "Updates": "Güncellemeler", "Privacy and data": "Gizlilik ve veriler", "Theme": "Tema", "Text size": "Yazı boyutu", "Dark": "Koyu", "Light": "Açık", "Compact": "Küçük", "Default": "Varsayılan", "Large": "Büyük", "Windows notifications": "Windows bildirimleri", "Send test notification": "Test bildirimi gönder", "Microphone": "Mikrofon", "Speaker": "Hoparlör", "System default": "Sistem varsayılanı", "Refresh audio devices": "Ses aygıtlarını yenile", "Echo cancellation": "Yankı engelleme", "Noise suppression": "Gürültü azaltma", "Automatic microphone gain": "Otomatik mikrofon seviyesi", "Push to talk (hold Space)": "Bas-konuş (Boşluk tuşunu basılı tut)", "Share system audio with screen": "Ekranla sistem sesini paylaş", "Save settings": "Ayarları kaydet", "Check now": "Şimdi kontrol et", "Install and restart": "Yükle ve yeniden başlat", "Save profile": "Profili kaydet", "Remove photo": "Fotoğrafı kaldır", "Your quiet corner": "Sana özel alan", "A little more room to talk.": "Rahatça konuşabileceğin bir alan.", "Find someone by username, send a friend request, then start a private conversation. Your account does not need an email or phone number.": "Etiketiyle kişi bul, arkadaşlık isteği gönder ve özel sohbete başla. Hesap için e-posta veya telefon gerekmez.", "View friends": "Arkadaşları gör", "Search": "Ara", "Search by username or user ID. GhostChat does not publish a public directory.": "Kullanıcı etiketiyle veya kullanıcı kimliğiyle ara. GhostChat herkese açık bir rehber yayınlamaz.", "No conversations yet. Find a friend to start a chat.": "Henüz sohbet yok. Sohbet başlatmak için arkadaş ekle.", "No pending requests.": "Bekleyen istek yok.", "YOUR FRIENDS": "ARKADAŞLARIN", "REQUESTS": "İSTEKLER", "DIRECT MESSAGES": "ÖZEL MESAJLAR", "No friends yet. Use Find people to add someone.": "Henüz arkadaşın yok. Kişi bul bölümünden arkadaş ekleyebilirsin.", "Manage requests and start a private conversation.": "İstekleri yönet ve özel sohbet başlat.", "All friends": "Tüm arkadaşlar", "Accept": "Kabul et", "Ignore": "Yoksay", "Message": "Mesaj", "Add friend": "Arkadaş ekle", "Request sent": "İstek gönderildi", "No friend requests right now.": "Şu anda arkadaşlık isteği yok.", "No matching messages.": "Eşleşen mesaj yok.", "Edited": "Düzenlendi", "Edit": "Düzenle", "Delete": "Sil", "Write a message…": "Mesaj yaz…", "Load earlier messages": "Önceki mesajları yükle", "Call": "Ara", "End call": "Aramayı bitir", "Settings saved.": "Ayarlar kaydedildi.", "Profile saved.": "Profil kaydedildi.", "Friend request accepted.": "Arkadaşlık isteği kabul edildi.", "Request removed.": "İstek kaldırıldı.", "Choose an image file.": "Bir resim dosyası seç.", "That photo is too detailed. Choose a smaller image.": "Fotoğraf çok detaylı. Daha küçük bir görsel seç.", "Could not use that photo.": "Fotoğraf kullanılamadı.", "Checking for updates": "Güncellemeler kontrol ediliyor", "Update required": "Güncelleme gerekli", "Downloading update": "Güncelleme indiriliyor", "Update failed": "Güncelleme başarısız", "Try again": "Yeniden dene", "Getting the latest version ready…": "En yeni sürüm hazırlanıyor…", "This app version is out of date.": "Bu uygulama sürümü güncel değil.", "Preparing the update…": "Güncelleme hazırlanıyor…"
  };
  Object.assign(trText, {
    "Choose a friend or find someone new.": "Bir arkadaş seç ya da yeni birini bul.", "Connected": "Bağlandı", "Reconnecting": "Yeniden bağlanıyor", "Voice ready": "Ses hazır", "Incoming call": "Gelen arama", "Calling…": "Aranıyor…", "Connecting…": "Bağlanıyor…", "In voice": "Sesli görüşmede", "Reconnecting…": "Yeniden bağlanıyor…", "Voice": "Ses", "Voice call": "Sesli arama", "Try microphone": "Mikrofonu dene", "Mic off": "Mikrofon kapalı", "Speaking": "Konuşuyor", "Hold Space": "Boşluk tuşunu basılı tut", "Turn mic on": "Mikrofonu aç", "Mute mic": "Mikrofonu kapat", "Sound off": "Ses kapalı", "Sound on": "Ses açık", "Starting…": "Başlatılıyor…", "Stop sharing": "Paylaşımı durdur", "Share screen": "Ekranı paylaş", "No microphone detected. Calls can still be joined muted so you can listen.": "Mikrofon bulunamadı. Yine de sessiz katılıp dinleyebilirsin.", "Microphone access is unavailable. You can stay muted or allow access and try again.": "Mikrofon erişimi yok. Sessiz kalabilir veya izin verip tekrar deneyebilirsin.", "No microphone found. You can still join calls muted.": "Mikrofon bulunamadı. Aramalara sessiz katılabilirsin.", "Audio devices refreshed.": "Ses aygıtları yenilendi.", "Checking voice relay support…": "Ses aktarım desteği kontrol ediliyor…", "Server address": "Sunucu adresi", "Selected server": "Seçili sunucu", "Local server": "Yerel sunucu", "Search @username or user ID": "@etiket veya kullanıcı kimliği ara", "Search people": "Kişi ara", "Search in your messages": "Mesajlarında ara", "Search for a username or user ID. Users are not listed publicly.": "Kullanıcı etiketiyle veya kimliğiyle ara. Kullanıcılar herkese açık listelenmez.", "Search messages from conversations you belong to.": "Üyesi olduğun sohbetlerdeki mesajları ara.", "No users found. Check the spelling and try again.": "Kullanıcı bulunamadı. Yazımı kontrol edip tekrar dene.", "Searching…": "Aranıyor…", "You are logged out.": "Oturum kapatıldı.", "Call audio is still blocked by Windows or the audio device.": "Arama sesi Windows veya ses aygıtı tarafından engelleniyor."
  });
  const tr = () => (localStorage.getItem("ghostchat.language") || "en") === "tr";
  function translate(value) { return tr() ? (trText[value] || value) : value; }
  function applyLanguage() {
    const language = tr() ? "tr" : "en";
    document.documentElement.lang = language;
    $$("[data-en][data-tr]").forEach(node => { node.textContent = node.dataset[language]; });
    $$("[data-en-placeholder][data-tr-placeholder]").forEach(node => { node.placeholder = node.getAttribute(`data-${language}-placeholder`); });
    for (const selector of ["#languageSelect", "#authLanguageSelect"]) { const select = $(selector); if (select) select.value = language; }
  }
  let voiceClient = null;
  let voiceUiState = { status: "idle" };

  function voiceSettings() {
    return {
      microphoneId: localStorage.getItem("ghostchat.voice.microphone") || "",
      speakerId: localStorage.getItem("ghostchat.voice.speaker") || "",
      echoCancellation: localStorage.getItem("ghostchat.voice.echo") !== "false",
      noiseSuppression: localStorage.getItem("ghostchat.voice.noise") !== "false",
      autoGainControl: localStorage.getItem("ghostchat.voice.gain") !== "false",
      pushToTalk: localStorage.getItem("ghostchat.voice.ptt") === "true",
      shareSystemAudio: localStorage.getItem("ghostchat.voice.systemAudio") === "true",
      relayOnly: localStorage.getItem("ghostchat.voice.relayOnly") === "true",
      volume: Number(localStorage.getItem("ghostchat.voice.volume") || "100"),
      speakerMuted: localStorage.getItem("ghostchat.voice.speakerMuted") === "true",
    };
  }

  function persistVoiceSettings() {
    localStorage.setItem("ghostchat.voice.microphone", $("#voiceMicSelect").value);
    localStorage.setItem("ghostchat.voice.speaker", $("#voiceSpeakerSelect").value);
    localStorage.setItem("ghostchat.voice.echo", String($("#voiceEchoCancellation").checked));
    localStorage.setItem("ghostchat.voice.noise", String($("#voiceNoiseSuppression").checked));
    localStorage.setItem("ghostchat.voice.gain", String($("#voiceAutoGain").checked));
    localStorage.setItem("ghostchat.voice.ptt", String($("#voicePushToTalk").checked));
    localStorage.setItem("ghostchat.voice.systemAudio", String($("#voiceShareAudio").checked));
    localStorage.setItem("ghostchat.voice.relayOnly", String($("#voiceRelayOnly").checked));
    localStorage.setItem("ghostchat.voice.volume", $("#voiceVolume").value);
  }

  function populateVoiceSettings() {
    const settings = voiceSettings();
    $("#voiceEchoCancellation").checked = settings.echoCancellation;
    $("#voiceNoiseSuppression").checked = settings.noiseSuppression;
    $("#voiceAutoGain").checked = settings.autoGainControl;
    $("#voicePushToTalk").checked = settings.pushToTalk;
    $("#voiceShareAudio").checked = settings.shareSystemAudio;
    $("#voiceRelayOnly").checked = settings.relayOnly;
    $("#voiceVolume").value = String(Math.max(0, Math.min(100, settings.volume)));
    const micSelects = [$("#voiceMicSelect"), $("#voiceCallMicSelect")];
    const speakerSelects = [$("#voiceSpeakerSelect"), $("#voiceCallSpeakerSelect")];
    const canSelectSpeaker = typeof $("#remoteAudio").setSinkId === "function";
    speakerSelects.forEach(select => { select.disabled = !canSelectSpeaker; });
    for (const select of micSelects) {
      if (settings.microphoneId && ![...select.options].some(option => option.value === settings.microphoneId)) select.add(new Option("Previously selected microphone", settings.microphoneId));
      select.value = settings.microphoneId;
    }
    for (const select of speakerSelects) {
      if (settings.speakerId && ![...select.options].some(option => option.value === settings.speakerId)) select.add(new Option("Previously selected speaker", settings.speakerId));
      select.value = settings.speakerId;
    }
    void checkVoiceRelaySupport();
  }

  async function checkVoiceRelaySupport() {
    const status = $("#voiceNetworkStatus");
    const relay = $("#voiceRelayOnly");
    status.textContent = "Checking voice relay support…";
    try {
      const config = await request("/v1/voice/ice");
      const enabled = !!config.turn_available;
      relay.disabled = !enabled;
      status.textContent = enabled
        ? "TURN relay is configured. Relay-only keeps your address from the other caller; the relay still handles encrypted media traffic."
        : "TURN relay is not configured. Direct calls can reveal your network address to the other caller and may fail on restrictive networks.";
    } catch (error) {
      relay.disabled = true;
      status.textContent = error.status === 404
        ? "This server is outdated for voice and screen sharing. Restart it with the latest GhostChat server build."
        : "Voice relay settings are unavailable. The selected server may need the latest GhostChat server build.";
    }
  }

  async function refreshAudioDevices({ requestPermission = true, quiet = false } = {}) {
    const buttons = [$("#refreshAudioDevices"), $("#voiceRefreshDevices")].filter(Boolean);
    buttons.forEach(button => { button.disabled = true; });
    try {
      if (!navigator.mediaDevices?.enumerateDevices) throw new Error("Audio devices are not available in this app context.");
      let permissionError = "";
      if (requestPermission && navigator.mediaDevices.getUserMedia) {
        try {
          const temporaryStream = await navigator.mediaDevices.getUserMedia({ audio: true, video: false });
          temporaryStream.getTracks().forEach(track => track.stop());
        } catch (error) { permissionError = error.message || "Microphone permission is unavailable."; }
      }
      const devices = await navigator.mediaDevices.enumerateDevices();
      const selectedMic = voiceSettings().microphoneId; const selectedSpeaker = voiceSettings().speakerId;
      const microphones = devices.filter(device => device.kind === "audioinput");
      function fill(select, kind, selected, fallback) {
        if (!select) return;
        const options = devices.filter(device => device.kind === kind);
        select.replaceChildren(new Option(translate("System default"), ""));
        options.forEach((device, index) => select.add(new Option(device.label || `${fallback} ${index + 1}`, device.deviceId)));
        select.value = options.some(device => device.deviceId === selected) ? selected : "";
      }
      for (const id of ["#voiceMicSelect", "#voiceCallMicSelect"]) fill($(id), "audioinput", selectedMic, "Microphone");
      for (const id of ["#voiceSpeakerSelect", "#voiceCallSpeakerSelect"]) fill($(id), "audiooutput", selectedSpeaker, "Speaker");
      const notice = $("#voiceDeviceNotice");
      if (!microphones.length) notice.textContent = "No microphone detected. Calls can still be joined muted so you can listen.";
      else if (permissionError && voiceUiState.status !== "idle") notice.textContent = "Microphone access is unavailable. You can stay muted or allow access and try again.";
      if (!quiet) {
        if (!microphones.length) toast("No microphone found. You can still join calls muted.");
        else toast("Audio devices refreshed.");
      }
    } catch (error) {
      if (!quiet) toast(error.message || "Could not read audio devices.", true);
    } finally { buttons.forEach(button => { button.disabled = false; }); }
  }

  async function applyPlaybackSettings() {
    const audio = $("#remoteAudio");
    if (!audio) return;
    const settings = voiceSettings();
    audio.volume = Math.max(0, Math.min(100, settings.volume)) / 100;
    audio.muted = settings.speakerMuted;
    if (typeof audio.setSinkId === "function") {
      try { await audio.setSinkId(settings.speakerId); }
      catch {
        try { await audio.setSinkId(""); } catch {}
        localStorage.setItem("ghostchat.voice.speaker", "");
        $("#voiceSpeakerSelect").value = "";
        $("#voiceCallSpeakerSelect").value = "";
        toast("That speaker is unavailable. The system default will be used.", true);
      }
    }
  }

  function toast(message, error = false) {
    const node = $("#toast"); node.textContent = translate(message); node.classList.toggle("error", error); node.classList.add("show");
    clearTimeout(toast.timer); toast.timer = setTimeout(() => node.classList.remove("show"), 3000);
  }
  function friendRequestsStorageKey() { return `ghostchat.seenFriendRequests.${state.base || "local"}.${state.user?.id || ""}`; }
  async function sendDesktopNotification(title, body) {
    if (localStorage.getItem("ghostchat.desktopNotifications") === "false") return;
    try { await invoke("show_desktop_notification", { title, body }); }
    catch (error) { console.warn("Windows notification could not be shown", error); }
  }
  function updateFriendRequestBadge() {
    const badge = $("#friendRequestBadge");
    if (!badge) return;
    const count = state.pending?.length || 0;
    badge.classList.toggle("hidden", count === 0);
    badge.textContent = count > 9 ? "9+" : String(count);
    $("#friendsNavButton")?.setAttribute("aria-label", count ? `Friends, ${count} pending request${count === 1 ? "" : "s"}` : "Friends");
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
  async function startupUpdateCheck() {
    const gate = $("#startupUpdateGate");
    const title = $("#startupUpdateTitle");
    const status = $("#startupUpdateStatus");
    const retry = $("#retryStartupUpdate");
    gate.classList.remove("hidden"); retry.classList.add("hidden");
    title.textContent = translate("Checking for updates"); status.textContent = translate("Getting the latest version ready…");
    $("#startupUpdateProgress").style.width = "24%";
    try {
      if (!invoke) { gate.classList.add("hidden"); return true; }
      const result = await invoke("check_for_update");
      if (!result.configured || !result.available) { gate.classList.add("hidden"); return true; }
      state.updateVersion = result.version;
      title.textContent = translate("Update required"); status.textContent = `${translate("This app version is out of date.")} GhostChat ${result.version} ${translate("is downloading")}…`;
      $("#startupUpdateProgress").style.width = "42%";
      await invoke("install_update", { expectedVersion: result.version });
      return false;
    } catch (error) {
      title.textContent = translate("Update failed"); status.textContent = error.message || translate("Could not check or install the required update.");
      $("#startupUpdateProgress").style.width = "100%"; retry.classList.remove("hidden");
      return false;
    }
  }
  $("#retryStartupUpdate").onclick = () => { $("#retryStartupUpdate").disabled = true; void boot().finally(() => { $("#retryStartupUpdate").disabled = false; }); };
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
  function isLocalServer(value) {
    try {
      const { hostname } = new URL(value);
      return hostname === "localhost" || hostname === "::1" || /^127(?:\.\d{1,3}){3}$/.test(hostname);
    } catch { return false; }
  }
  function parseServerAddress(value) {
    const input = value.trim();
    if (!input) return "";
    const hasScheme = /^[a-z][a-z\d+.-]*:\/\//i.test(input);
    const isLocal = /^(localhost|127(?:\.\d{1,3}){3})(?::\d+)?(?:\/|$)/i.test(input);
    const url = new URL(hasScheme ? input : `${isLocal ? "http" : "https"}://${input}`);
    if (url.username || url.password || url.search || url.hash || (url.pathname !== "/" && url.pathname !== "")) throw new Error("Enter only the server address, without a path or extra URL details.");
    if (url.protocol !== "https:" && !(url.protocol === "http:" && ["localhost", "127.0.0.1"].includes(url.hostname))) throw new Error("Remote servers must use HTTPS.");
    return normalizedBase(url.origin);
  }
  function isLegacyDefaultServer(value) {
    try { return parseServerAddress(value) === LEGACY_DEFAULT_REMOTE_SERVER; }
    catch { return false; }
  }
  function activeBase() { return state.base || state.localBase; }
  async function serverKey(base) {
    const data = new TextEncoder().encode(normalizedBase(base));
    const digest = await crypto.subtle.digest("SHA-256", data);
    return [...new Uint8Array(digest)].map(byte => byte.toString(16).padStart(2, "0")).join("");
  }
  async function migrateServerCredentials(from, to) {
    try {
      const oldKey = await serverKey(from), newKey = await serverKey(to);
      const [oldToken, newToken] = await Promise.all([
        invoke("credential_get", { serverKey: oldKey }), invoke("credential_get", { serverKey: newKey })
      ]);
      if (oldToken && !newToken) await invoke("credential_set", { serverKey: newKey, token: oldToken });
      const [oldRecovery, newRecovery] = await Promise.all([
        invoke("credential_get_pending_recovery", { serverKey: oldKey }), invoke("credential_get_pending_recovery", { serverKey: newKey })
      ]);
      if (oldRecovery && !newRecovery) await invoke("credential_set_pending_recovery", { serverKey: newKey, token: oldRecovery });
    } catch (error) { console.warn("Could not migrate saved credentials to the default server", error); }
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
    if (!response.ok) {
      const error = new Error(body.error || `Request failed (${response.status}).`);
      error.status = response.status;
      throw error;
    }
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
    $("#serverModeLabel").textContent = remote ? "Selected server" : "Local server";
    $("#serverLabel").textContent = label; $("#serverAddressInput").value = state.base;
  }
  function avatarText(value = "G") { return (value.trim().replace(/^@/, "")[0] || "G").toUpperCase(); }
  function setAvatar(node, name = "G", image = null) {
    if (!node) return;
    node.replaceChildren();
    if (image) { const img = document.createElement("img"); img.src = image; img.alt = ""; img.draggable = false; node.append(img); }
    else node.textContent = avatarText(name);
  }
  function create(tag, className, text) { const node = document.createElement(tag); if (className) node.className = className; if (text !== undefined) node.textContent = translate(text); return node; }
  async function compressAvatar(file) {
    if (!file || !file.type.startsWith("image/")) throw new Error(translate("Choose an image file."));
    const bitmap = await createImageBitmap(file);
    const side = Math.min(bitmap.width, bitmap.height);
    const canvas = document.createElement("canvas"); canvas.width = 256; canvas.height = 256;
    const context = canvas.getContext("2d", { alpha: false }); context.fillStyle = "#36313f"; context.fillRect(0, 0, 256, 256);
    context.drawImage(bitmap, (bitmap.width - side) / 2, (bitmap.height - side) / 2, side, side, 0, 0, 256, 256); bitmap.close();
    for (const quality of [0.82, 0.72, 0.62, 0.52, 0.42]) {
      const data = canvas.toDataURL("image/jpeg", quality);
      if (data.length <= 46_000) return data;
    }
    throw new Error(translate("That photo is too detailed. Choose a smaller image."));
  }
  async function handleAvatarFile(input, preview, save) {
    const file = input.files?.[0]; if (!file) return;
    try { const image = await compressAvatar(file); save(image); setAvatar($(preview), state.user?.display_name || $("#createForm [name=display_name]")?.value || "G", image); }
    catch (error) { toast(error.message || translate("Could not use that photo."), true); }
    finally { input.value = ""; }
  }
  function setSignupAvatar(value) { signupAvatarData = value; }
  function setTheme() {
    applyLanguage();
    const theme = localStorage.getItem("ghostchat.theme") || "dark";
    document.body.classList.toggle("light", theme === "light"); $("#themeSelect").value = theme;
    const size = localStorage.getItem("ghostchat.textSize") || "15";
    document.documentElement.style.fontSize = `${size}px`; $("#textSize").value = size;
  }
  function updateIdentity() {
    $("#accountName").textContent = state.user.display_name;
    $("#accountTag").textContent = `@${state.user.username}`;
    setAvatar($("#accountAvatar"), state.user.display_name, state.user.avatar_data);
    setAvatar($("#profileButton"), state.user.display_name, state.user.avatar_data);
    $("#profileTagLabel").textContent = `@${state.user.username}`;
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
    $("#chatTitle").textContent = translate("Your quiet corner"); $("#chatSubtitle").textContent = translate("Choose a friend or find someone new.");
    setAvatar($("#chatAvatar"), "G"); $("#messagePanel").classList.add("hidden"); $("#pageContent").classList.remove("hidden");
    $("#pageContent").innerHTML = "";
    const card = create("div", "welcome-card");
    card.append(create("div", "welcome-orb", "◌"), create("h2", "", "A little more room to talk."), create("p", "", "Find someone by username, send a friend request, then start a private conversation. Your account does not need an email or phone number."));
    const actions = create("div", "welcome-actions");
    const find = create("button", "button primary", "Find people"); find.onclick = () => setPage("search");
    const friends = create("button", "button secondary", "View friends"); friends.onclick = () => setPage("friends");
    actions.append(find, friends); card.append(actions); $("#pageContent").append(card); renderVoiceControls();
  }
  function setPage(page) {
    state.page = page; state.active = page === "messages" ? state.active : null;
    if (page === "friends") state.searchTab = state.pending?.length ? "pending" : "all";
    $$(".rail-button[data-page]").forEach(button => button.classList.toggle("active", button.dataset.page === page));
    const titles = { messages: "Messages", friends: "Friends", search: "Find people", searchMessages: "Search messages" };
    $("#sidebarTitle").textContent = translate(titles[page]); $("#sidebarAction").title = translate(page === "messages" ? "Start a conversation" : "Find people");
    if (page !== "messages") { $("#messagePanel").classList.add("hidden"); $("#pageContent").classList.remove("hidden"); renderPage(); }
    else if (state.active) openConversation(state.active.id, false);
    else showWelcome();
    renderSidebar(); renderVoiceControls();
  }
  function renderSidebar() {
    const root = $("#sidebarContent"); root.replaceChildren();
    if (state.page === "messages") {
      if (!state.conversations.length) { root.append(create("div", "empty-list", "No conversations yet. Find a friend to start a chat.")); return; }
      const label = create("div", "section-label", "DIRECT MESSAGES"); root.append(label);
      for (const convo of state.conversations) {
        const row = create("button", `list-row${state.active?.id === convo.id ? " active" : ""}`); row.onclick = () => openConversation(convo.id);
        const avatar = create("div", "mini-avatar"); setAvatar(avatar, convo.display_name, convo.avatar_data); row.append(avatar);
        const copy = create("div", "list-copy"); copy.append(create("strong", "", convo.display_name), create("span", "", convo.last_message || `@${convo.username}`)); row.append(copy); root.append(row);
      }
      return;
    }
    if (state.page === "friends") {
      const requestsLabel = create("div", "section-label", "REQUESTS"); root.append(requestsLabel);
      for (const request of state.pending || []) { const row = create("button", "list-row"); const avatar = create("div", "mini-avatar"); setAvatar(avatar, request.display_name, request.avatar_data); row.append(avatar); const copy = create("div", "list-copy"); copy.append(create("strong", "", request.display_name), create("span", "", `@${request.username}`)); row.append(copy); root.append(row); }
      if (!(state.pending || []).length) root.append(create("div", "empty-list", "No pending requests."));
      root.append(create("div", "section-label", "YOUR FRIENDS"));
      for (const friend of state.friends) { const row = create("button", "list-row"); row.onclick = () => startConversation(friend.id); const avatar = create("div", "mini-avatar"); setAvatar(avatar, friend.display_name, friend.avatar_data); row.append(avatar); const copy = create("div", "list-copy"); copy.append(create("strong", "", friend.display_name), create("span", "", `@${friend.username}`)); row.append(copy); root.append(row); }
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
      const form = create("form", "search-form"); const input = create("input"); input.placeholder = translate("Search @username or user ID"); input.setAttribute("aria-label", translate("Search people")); input.minLength = 2;
      const submit = create("button", "button primary", "Search"); submit.type = "submit"; form.append(input, submit); page.append(form);
      const results = create("div", "search-results"); page.append(results);
      form.onsubmit = async event => { event.preventDefault(); if (input.value.trim().length < 2) return; results.replaceChildren(create("div", "empty-list", "Searching…")); try { const users = await request(`/v1/users/search?q=${encodeURIComponent(input.value.trim())}`); results.replaceChildren(); if (!users.length) results.append(create("div", "empty-list", "No users found. Check the spelling and try again.")); for (const user of users) { const card = create("div", "result-card"); const avatar = create("div", "mini-avatar"); setAvatar(avatar, user.display_name, user.avatar_data); card.append(avatar); const copy = create("div", "list-copy"); copy.append(create("strong", "", user.display_name), create("span", "", `@${user.username} · ${user.id.slice(0, 8)}`)); card.append(copy); const add = create("button", "button secondary", "Add friend"); add.onclick = async () => { try { await request("/v1/friends", { method: "POST", body: JSON.stringify({ user_id: user.id }) }); add.textContent = translate("Request sent"); add.disabled = true; toast(translate("Friend request sent.")); } catch (error) { toast(error.message, true); } }; card.append(add); results.append(card); } } catch (error) { results.replaceChildren(create("div", "empty-list", error.message)); } };
      setTimeout(() => input.focus(), 20);
    } else if (state.page === "friends") {
      page.append(create("h2", "", "Friends"), create("p", "muted", "Manage requests and start a private conversation."));
      const tabs = create("div", "tab-row");
      for (const [key, label] of [["all", "All friends"], ["pending", `Requests${(state.pending || []).length ? ` · ${(state.pending || []).length}` : ""}`]]) { const button = create("button", state.searchTab === key ? "active" : "", label); button.onclick = () => { state.searchTab = key; renderPage(); }; tabs.append(button); }
      page.append(tabs);
      const list = create("div", "friend-results"); page.append(list);
      if (state.searchTab === "pending") {
        if (!(state.pending || []).length) list.append(create("div", "empty-list", "No friend requests right now."));
        for (const item of state.pending || []) { const row = create("div", "request-card"); const avatar = create("div", "mini-avatar"); setAvatar(avatar, item.display_name, item.avatar_data); row.append(avatar); const copy = create("div", "list-copy"); copy.append(create("strong", "", item.display_name), create("span", "", `@${item.username}`)); row.append(copy); const actions = create("div", "request-actions"); const accept = create("button", "button primary", "Accept"); accept.onclick = () => respondFriend(item.id, "accept"); const reject = create("button", "button secondary", "Ignore"); reject.onclick = () => respondFriend(item.id, "reject"); actions.append(accept, reject); row.append(actions); list.append(row); }
      } else {
        if (!state.friends.length) list.append(create("div", "empty-list", "No friends yet. Use Find people to add someone."));
        for (const friend of state.friends) { const row = create("div", "result-card"); const avatar = create("div", "mini-avatar"); setAvatar(avatar, friend.display_name, friend.avatar_data); row.append(avatar); const copy = create("div", "list-copy"); copy.append(create("strong", "", friend.display_name), create("span", "", `@${friend.username}`)); row.append(copy); const message = create("button", "button primary", "Message"); message.onclick = () => startConversation(friend.id); row.append(message); list.append(row); }
      }
    } else if (state.page === "searchMessages") {
      page.append(create("h2", "", "Search messages"), create("p", "muted", "Search messages from conversations you belong to."));
      const form = create("form", "search-form"); const input = create("input"); input.placeholder = translate("Search in your messages"); input.minLength = 2; const go = create("button", "button primary", "Search"); go.type = "submit"; form.append(input, go); page.append(form); const results = create("div", "search-results"); page.append(results);
      form.onsubmit = async event => { event.preventDefault(); try { const rows = await request(`/v1/messages/search?q=${encodeURIComponent(input.value.trim())}`); results.replaceChildren(); if (!rows.length) results.append(create("div", "empty-list", "No matching messages.")); for (const row of rows) { const card = create("button", "result-card"); card.style.textAlign = "left"; const copy = create("div", "list-copy"); copy.append(create("strong", "", row.sender_name), create("span", "", row.content)); card.append(copy); card.onclick = () => { setPage("messages"); openConversation(row.conversation_id); }; results.append(card); } } catch (error) { toast(error.message, true); } };
      setTimeout(() => input.focus(), 20);
    }
    root.append(page); $("#messagePanel").classList.add("hidden");
  }
  async function refreshFriends({ notifyNew = true } = {}) {
    try {
      const [friends, pending] = await Promise.all([request("/v1/friends"), request("/v1/friends/requests")]);
      const key = friendRequestsStorageKey();
      let seen = new Set(state.seenFriendRequestIds);
      try { for (const id of JSON.parse(localStorage.getItem(key) || "[]")) seen.add(id); } catch {}
      const newRequests = pending.filter(item => !seen.has(item.id));
      state.friends = friends;
      state.pending = pending;
      state.seenFriendRequestIds = new Set(pending.map(item => item.id));
      localStorage.setItem(key, JSON.stringify([...state.seenFriendRequestIds]));
      if (notifyNew && newRequests.length) {
        if (state.page === "friends") state.searchTab = "pending";
        for (const item of newRequests) {
          toast(`${item.display_name} sent you a friend request.`);
          void sendDesktopNotification("New friend request", `${item.display_name} sent you a friend request.`);
        }
      }
    } catch (error) { console.warn(error); }
    updateFriendRequestBadge();
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
    $("#chatTitle").textContent = state.active.display_name; $("#chatSubtitle").textContent = `@${state.active.username}`; setAvatar($("#chatAvatar"), state.active.display_name, state.active.avatar_data);
    renderVoiceControls();
    await loadMessages(false);
  }
  function timeLabel(value) {
    const date = new Date(value); if (Number.isNaN(date.getTime())) return "";
    return new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" }).format(date);
  }
  function appendMessage(message, prepend = false) {
    const container = $("#messages"); if ($(`[data-message-id="${CSS.escape(message.id)}"]`, container)) return;
    const row = create("article", "message"); row.dataset.messageId = message.id; row.dataset.sequence = String(message.sequence);
    const avatar = create("div", "mini-avatar"); setAvatar(avatar, message.sender_name, message.sender_avatar); row.append(avatar);
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
    const node = $("#connectionState"); node.classList.toggle("offline", !online); node.innerHTML = `<i class="status-dot ${online ? "" : "offline"}"></i>${translate(online ? "Connected" : "Reconnecting")}`;
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
      if (data.type === "ready") { state.reconnect = 1200; setConnection(true); void refreshFriends(); refreshConversations(); if (state.active) loadMessages(false); }
      else if (data.type === "session_revoked") { voiceClient?.hangup(); state.token = ""; state.user = null; state.active = null; if (state.socket) state.socket.close(); void (async () => { try { await invoke("credential_delete", { serverKey: await serverKey(activeBase()) }); } catch {} })(); show("authView"); toast("This session was revoked. Recover your account to sign in again.", true); }
      else if (data.type === "message" && data.message) {
        const message = data.message;
        if (state.active?.id === message.conversation_id) { appendMessage(message); $("#messages").scrollTop = $("#messages").scrollHeight; }
        if (message.sender_id !== state.user?.id && (state.active?.id !== message.conversation_id || !document.hasFocus())) {
          void sendDesktopNotification(`Message from ${message.sender_name}`, "You received a new message in GhostChat.");
        }
        refreshConversations();
      }
      else if (data.type === "friend_request") { void refreshFriends(); }
      else if (data.type === "profile_updated") { void refreshFriends({ notifyNew: false }); void refreshConversations(); if (state.active) void loadMessages(false); }
      else if (data.type === "friend_request_accepted") {
        void refreshFriends();
        if (data.by_user_id !== state.user?.id) {
          toast("Your friend request was accepted.");
          void sendDesktopNotification("Friend request accepted", "You can now chat with your new friend.");
        }
      }
      else if (data.type === "voice_signal") { void voiceClient?.handleSignal(data); }
      else if (data.type === "deleted" || data.type === "refresh") { if (state.active) loadMessages(false); }
    });
    socket.addEventListener("close", () => { if (socket === state.socket) { setConnection(false); scheduleReconnect(); } });
    socket.addEventListener("error", () => { try { socket.close(); } catch {} });
  }
  function scheduleReconnect() {
    clearTimeout(connectSocket.timer); connectSocket.timer = setTimeout(() => { if (state.token) connectSocket(); }, state.reconnect);
    state.reconnect = Math.min(30000, state.reconnect * 1.7);
  }
  function renderVoiceControls() {
    if (!voiceClient) return;
    const voice = voiceClient.getState();
    voiceUiState = voice;
    const activeCall = !["idle", "incoming"].includes(voice.status);
    const visible = !!state.active || voice.status !== "idle";
    $("#voiceTools").classList.toggle("hidden", !visible);
    $("#voiceCallButton").classList.toggle("hidden", !state.active || voice.status !== "idle");
    $("#voiceCallButton").disabled = !state.active;
    const labels = { idle: "Voice ready", incoming: "Incoming call", calling: "Calling…", connecting: "Connecting…", connected: "In voice", reconnecting: "Reconnecting…" };
    $("#voiceStatus").textContent = translate(labels[voice.status] || "Voice");
    const conversation = state.conversations.find(item => item.id === voice.conversationId);
    const peer = conversation?.display_name || "Your friend";
    $("#voicePeerName").textContent = peer;
    setAvatar($("#voicePeerAvatar"), peer, conversation?.avatar_data);
    $("#voiceStageStatus").textContent = translate(voice.status === "incoming" ? "Incoming call" : (labels[voice.status] || "Voice call"));
    $("#voiceStageDot").classList.toggle("connecting", !["connected", "incoming"].includes(voice.status));
    $("#voiceStageDot").classList.toggle("offline", voice.status === "reconnecting");
    $("#voiceDeviceRow").classList.toggle("hidden", !activeCall);
    $("#voiceStageMicButton").classList.toggle("hidden", !activeCall);
    $("#voiceStageMicButton").classList.toggle("is-muted", voice.micMuted && (!voice.pushToTalk || voice.micManuallyMuted || !voice.micAvailable));
    $("#voiceStageMicButton").setAttribute("aria-pressed", String(voice.micMuted));
    $("#voiceStageMicIcon").textContent = voice.micMuted ? "🎙" : "🎤";
    $("#voiceStageMicLabel").textContent = translate(!voice.micAvailable ? "Try microphone" : voice.pushToTalk ? (voice.micManuallyMuted ? "Mic off" : (voice.pushToTalkPressed ? "Speaking" : "Hold Space")) : (voice.micMuted ? "Turn mic on" : "Mute mic"));
    $("#voiceStageMicButton").title = !voice.micAvailable ? "Try to connect a microphone; if none is available you can keep listening muted" : (voice.micMuted ? "Turn microphone on" : "Mute microphone");
    $("#voiceOutputButton").classList.toggle("hidden", !activeCall);
    $("#voiceOutputButton").classList.toggle("is-muted", voice.speakerMuted);
    $("#voiceOutputButton").setAttribute("aria-pressed", String(voice.speakerMuted));
    $("#voiceOutputIcon").textContent = voice.speakerMuted ? "🔇" : "🔊";
    $("#voiceOutputLabel").textContent = translate(voice.speakerMuted ? "Sound off" : "Sound on");
    $("#voiceStageShareButton").classList.toggle("hidden", !activeCall || !["connected", "reconnecting"].includes(voice.status));
    $("#voiceStageShareIcon").textContent = voice.screenSharing ? "■" : "▣";
    $("#voiceStageShareLabel").textContent = translate(voice.screenStarting ? "Starting…" : (voice.screenSharing ? "Stop sharing" : "Share screen"));
    $("#voiceStageShareButton").disabled = !!voice.screenStarting;
    $("#voiceStageAccept").classList.toggle("hidden", voice.status !== "incoming");
    $("#voiceStageDecline").classList.toggle("hidden", voice.status !== "incoming");
    $("#voiceHangupButton").classList.toggle("hidden", !activeCall);
    $("#voiceDeviceNotice").textContent = voice.status === "incoming"
      ? "Join the call when you are ready. If no microphone is available, you can still listen muted."
      : !voice.micAvailable && activeCall
        ? (voice.micUnavailableReason || "No microphone found. You joined muted and can still listen.")
        : voice.pushToTalk
          ? "Push to talk is on. Hold Space to speak; your microphone stays muted otherwise."
          : voice.micMuted && activeCall
            ? "Your microphone is off. You can still hear the call."
            : activeCall
              ? "Your microphone is on. Use the controls above to switch devices or mute."
              : "Choose your microphone and speaker, or join muted and listen.";
    const hasRemoteVideo = !!voice.remoteVideoStream?.getVideoTracks().some(track => track.readyState === "live");
    $("#remoteScreen").classList.toggle("hidden", !hasRemoteVideo);
    $("#voiceStage").classList.toggle("hidden", voice.status === "idle");
    if (voice.status === "idle") {
      $("#remoteAudio").srcObject = null;
      $("#remoteScreen").srcObject = null;
      $("#enableCallAudio").classList.add("hidden");
    }
  }

  function onVoiceChange(voice) {
    voiceUiState = voice;
    renderVoiceControls();
    const audio = $("#remoteAudio");
    const video = $("#remoteScreen");
    if (audio.srcObject !== voice.remoteAudioStream) {
      audio.srcObject = voice.remoteAudioStream;
      if (voice.remoteAudioStream) void audio.play().then(() => $("#enableCallAudio").classList.add("hidden")).catch(() => $("#enableCallAudio").classList.remove("hidden"));
    }
    if (video.srcObject !== voice.remoteVideoStream) {
      video.srcObject = voice.remoteVideoStream;
      if (voice.remoteVideoStream) void video.play().catch(() => {});
    }
    void applyPlaybackSettings();
  }

  function setupVoiceClient() {
    voiceClient = window.GhostChatVoice.create({
      getSettings: voiceSettings,
      getUserId: () => state.user?.id || "",
      getIceConfiguration: async () => {
        try { return await request("/v1/voice/ice"); }
        catch (error) {
          if (error.status === 404) {
            throw new Error("This server is outdated for voice and screen sharing. Restart it with the latest GhostChat server build.");
          }
          throw error;
        }
      },
      sendSignal: ({ conversationId, peerId, kind, payload }) => {
        if (!state.socket || state.socket.readyState !== WebSocket.OPEN) throw new Error("The chat server is reconnecting. Wait until it is connected and try again.");
        state.socket.send(JSON.stringify({ type: "voice_signal", conversation_id: conversationId, to_user_id: peerId, kind, payload }));
      },
      onChange: onVoiceChange,
      onIncoming: ({ conversationId }) => {
        const convo = state.conversations.find(item => item.id === conversationId);
        if (convo) {
          void openConversation(conversationId);
          $("#sidebarTitle").textContent = "Messages";
          $("#sidebarAction").title = "Start a conversation";
          $("#pageContent").classList.add("hidden");
          renderSidebar();
        }
        const caller = convo?.display_name || "Your friend";
        toast(`${caller} is calling.`);
        void sendDesktopNotification("Incoming GhostChat call", `${caller} is calling you.`);
      },
      onError: message => toast(message, true),
    });
    renderVoiceControls();
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
  $("#signupAvatar").addEventListener("change", event => { void handleAvatarFile(event.currentTarget, "#signupAvatarPreview", setSignupAvatar); });
  $("#profileAvatarInput").addEventListener("change", event => { void handleAvatarFile(event.currentTarget, "#profileAvatarPreview", value => { profileAvatarData = value; }); });
  $("#createForm").addEventListener("submit", async event => {
    event.preventDefault(); const form = new FormData(event.currentTarget); const button = event.currentTarget.querySelector("button[type=submit]"); button.disabled = true;
    try { const reply = await request("/v1/accounts", { method: "POST", body: JSON.stringify({ display_name: form.get("display_name"), avatar_data: signupAvatarData }) }); await authenticateReply(reply); }
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
  function openSettings() {
    $$(".settings-section").forEach(section => { if (section.querySelector("#profileNameInput")) section.classList.toggle("hidden", !state.user); });
    $("#serverAddressInput").value = state.base;
    $("#profileNameInput").value = state.user?.display_name || "";
    profileAvatarData = state.user?.avatar_data || null;
    setAvatar($("#profileAvatarPreview"), state.user?.display_name, profileAvatarData);
    $("#languageSelect").value = tr() ? "tr" : "en";
    $("#desktopNotificationsEnabled").checked = localStorage.getItem("ghostchat.desktopNotifications") !== "false";
    populateVoiceSettings();
    $("#settingsDialog").showModal();
  }
  $("#settingsButton").onclick = openSettings;
  $("#serverSettings").onclick = () => {
    openSettings();
    $("#connectionSettingsSection").open = true;
    $("#serverAddressInput").focus();
  };
  $("#profileButton").onclick = openSettings;
  for (const selector of ["#languageSelect", "#authLanguageSelect"]) $(selector).onchange = event => { localStorage.setItem("ghostchat.language", event.target.value); applyLanguage(); renderSidebar(); if (["friends", "search", "searchMessages"].includes(state.page)) renderPage(); else if (!state.active) showWelcome(); renderVoiceControls(); };
  $("#removeProfileAvatar").onclick = () => { profileAvatarData = null; setAvatar($("#profileAvatarPreview"), state.user?.display_name); };
  $("#saveProfile").onclick = async event => {
    const button = event.currentTarget; button.disabled = true;
    try {
      state.user = await request("/v1/me/profile", { method: "PATCH", body: JSON.stringify({ display_name: $("#profileNameInput").value, avatar_data: profileAvatarData }) });
      updateIdentity(); await refreshFriends({ notifyNew: false }); await refreshConversations();
      if (state.active) { await loadMessages(false); }
      toast(translate("Profile saved."));
    } catch (error) { toast(error.message, true); }
    finally { button.disabled = false; }
  };
  $("#desktopNotificationsEnabled").onchange = event => localStorage.setItem("ghostchat.desktopNotifications", String(event.target.checked));
  $("#testDesktopNotification").onclick = async () => {
    const button = $("#testDesktopNotification"); button.disabled = true;
    try {
      await invoke("show_desktop_notification", { title: "GhostChat notifications", body: "Windows notifications are working." });
      toast("Test notification sent. If it is hidden, check Windows notification settings.");
    } catch (error) { toast(error.message || "Windows could not show the test notification.", true); }
    finally { button.disabled = false; }
  };
  $("#refreshAudioDevices").onclick = refreshAudioDevices;
  $("#voiceRefreshDevices").onclick = () => void refreshAudioDevices();
  function bindVoiceDeviceSelect(selectId, companionId, storageKey, kind) {
    $(selectId).addEventListener("change", async event => {
      const deviceId = event.currentTarget.value;
      localStorage.setItem(storageKey, deviceId);
      const companion = $(companionId);
      if (companion && [...companion.options].some(option => option.value === deviceId)) companion.value = deviceId;
      try {
        if (kind === "microphone" && ["calling", "connecting", "connected", "reconnecting"].includes(voiceClient.getState().status)) {
          await voiceClient.setMicrophoneDevice(deviceId);
        } else if (kind === "speaker") await applyPlaybackSettings();
      } catch (error) { toast(error.message || "Could not change the audio device.", true); }
      renderVoiceControls();
    });
  }
  bindVoiceDeviceSelect("#voiceMicSelect", "#voiceCallMicSelect", "ghostchat.voice.microphone", "microphone");
  bindVoiceDeviceSelect("#voiceCallMicSelect", "#voiceMicSelect", "ghostchat.voice.microphone", "microphone");
  bindVoiceDeviceSelect("#voiceSpeakerSelect", "#voiceCallSpeakerSelect", "ghostchat.voice.speaker", "speaker");
  bindVoiceDeviceSelect("#voiceCallSpeakerSelect", "#voiceSpeakerSelect", "ghostchat.voice.speaker", "speaker");
  $("#voiceCallButton").onclick = () => {
    if (state.active) {
      $("#voiceCallButton").disabled = true;
      void voiceClient.start(state.active).catch(error => toast(error.message, true)).finally(renderVoiceControls);
    }
  };
  $("#voiceStageAccept").onclick = () => {
    $("#voiceStageAccept").disabled = true;
    void voiceClient.accept().catch(error => toast(error.message, true)).finally(() => { $("#voiceStageAccept").disabled = false; renderVoiceControls(); });
  };
  $("#voiceStageDecline").onclick = () => voiceClient.decline();
  $("#voiceHangupButton").onclick = () => voiceClient.hangup();
  $("#voiceStageMicButton").onclick = () => void voiceClient.toggleMicrophone().catch(error => toast(error.message, true));
  $("#voiceOutputButton").onclick = () => {
    localStorage.setItem("ghostchat.voice.speakerMuted", String(!voiceSettings().speakerMuted));
    void applyPlaybackSettings();
    renderVoiceControls();
  };
  $("#voiceStageShareButton").onclick = () => void voiceClient.toggleScreenSharing().catch(error => toast(error.message, true));
  $("#enableCallAudio").onclick = () => void $("#remoteAudio").play().then(() => $("#enableCallAudio").classList.add("hidden")).catch(() => toast("Call audio is still blocked by Windows or the audio device.", true));
  $("#voiceVolume").addEventListener("input", () => { $("#remoteAudio").volume = Number($("#voiceVolume").value) / 100; });
  $("#changeServerButton").onclick = async event => {
    const button = event.currentTarget;
    let target = "";
    const input = $("#serverAddressInput").value.trim();
    if (input) {
      try { target = parseServerAddress(input); }
      catch (error) { toast(error.message || "Enter a valid server address.", true); return; }
    }
    if (target === state.base) { toast(tr() ? "Bu sunucu zaten seçili." : "This server is already selected.", true); return; }
    button.disabled = true;
    try {
      state.base = target;
      localStorage.setItem("ghostchat.server", target || LOCAL_SERVER_SENTINEL);
      voiceClient?.hangup();
      const oldSocket = state.socket; state.socket = null; oldSocket?.close();
      state.user = null; state.active = null; state.friends = []; state.conversations = []; state.pending = [];
      updateServerLabels(); $("#settingsDialog").close();
      await restoreToken();
      try { state.pendingRecovery = await invoke("credential_get_pending_recovery", { serverKey: await serverKey(activeBase()) }) || ""; } catch { state.pendingRecovery = ""; }
      if (state.pendingRecovery) {
        $("#recoveryValue").textContent = state.pendingRecovery;
        $("#recoveryConfirm").checked = false; $("#continueAfterRecovery").disabled = true; show("recoveryView");
      } else if (state.token) await enterApp(); else show("authView");
      if (target) {
        const controller = new AbortController(); const timeout = setTimeout(() => controller.abort(), 4000);
        let reachable = false;
        try { const response = await fetch(`${target}/health`, { signal: controller.signal }); reachable = response.ok; }
        catch {} finally { clearTimeout(timeout); }
        toast(reachable ? `Server address saved: ${new URL(target).host}` : `Server address saved: ${new URL(target).host}. The server is not reachable yet.`, !reachable);
      } else toast(tr() ? "Yerel sunucu seçildi." : "Server changed to the local server.");
    } catch (error) { toast(error.message || "Could not change the server.", true); }
    finally { button.disabled = false; }
  };
  $("#saveSettings").onclick = async () => {
    const theme = $("#themeSelect").value; localStorage.setItem("ghostchat.theme", theme); document.body.classList.toggle("light", theme === "light");
    localStorage.setItem("ghostchat.textSize", $("#textSize").value); document.documentElement.style.fontSize = `${$("#textSize").value}px`;
    applyLanguage();
    persistVoiceSettings(); await applyPlaybackSettings();
    $("#settingsDialog").close(); toast("Settings saved.");
  };
  $("#exportSettings").onclick = () => { const data = { server: state.base || "local", theme: localStorage.getItem("ghostchat.theme") || "dark", textSize: localStorage.getItem("ghostchat.textSize") || "15" }; const url = URL.createObjectURL(new Blob([JSON.stringify(data, null, 2)], { type: "application/json" })); const link = create("a"); link.href = url; link.download = "ghostchat-preferences.json"; link.click(); URL.revokeObjectURL(url); };
  $("#exportAccount").onclick = async () => { try { const data = await request("/v1/data/export"); const url = URL.createObjectURL(new Blob([JSON.stringify(data, null, 2)], { type: "application/json" })); const link = create("a"); link.href = url; link.download = "ghostchat-account-export.json"; link.click(); URL.revokeObjectURL(url); toast("Account data exported."); } catch (error) { toast(error.message, true); } };
  $("#deleteAccount").onclick = async () => { if (!confirm("This permanently deletes your account, sessions, friend links, conversations, and stored messages. This cannot be undone.")) return; if (prompt('Type DELETE to permanently erase this account:') !== "DELETE") return; try { await request("/v1/me/account", { method: "DELETE" }); voiceClient?.hangup(); if (state.socket) state.socket.close(); await clearToken(); await clearPendingRecovery(); state.user = null; state.active = null; state.friends = []; state.conversations = []; $("#settingsDialog").close(); show("authView"); toast("Account and data deleted."); } catch (error) { toast(error.message, true); } };
  $("#logoutButton").onclick = async () => { try { await request("/v1/sessions", { method: "DELETE" }); } catch {} voiceClient?.hangup(); if (state.socket) state.socket.close(); await clearToken(); state.user = null; state.active = null; state.conversations = []; show("authView"); toast("You are logged out."); };
  $("#checkUpdates").onclick = () => checkForUpdates(false);
  $("#installUpdate").onclick = installUpdate;
  window.__TAURI__?.event?.listen?.("ghostchat:update-progress", event => {
    const percent = Number(event.payload);
    if (Number.isFinite(percent)) {
      $("#updateStatus").textContent = `${translate("Downloading update")}… ${Math.min(100, Math.max(0, percent))}%.`;
      $("#startupUpdateStatus").textContent = `${translate("Downloading update")}… ${Math.min(100, Math.max(0, percent))}%.`;
      $("#startupUpdateProgress").style.width = `${Math.min(100, Math.max(0, percent))}%`;
    }
  });
  document.addEventListener("keydown", event => {
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") { event.preventDefault(); setPage("search"); }
    if (event.key === "Escape" && !$("#settingsDialog").open) document.activeElement?.blur();
    if (event.code === "Space" && !event.repeat && voiceSettings().pushToTalk && !event.target.closest("input, textarea, select, [contenteditable=true], button")) { event.preventDefault(); voiceClient?.setPushToTalk(true); }
  });
  document.addEventListener("keyup", event => { if (event.code === "Space") voiceClient?.setPushToTalk(false); });
  window.addEventListener("blur", () => voiceClient?.setPushToTalk(false));
  setupVoiceClient();
  void refreshAudioDevices({ requestPermission: false, quiet: true });
  navigator.mediaDevices?.addEventListener?.("devicechange", () => { void refreshAudioDevices({ requestPermission: false, quiet: true }); });

  async function boot() {
    setTheme();
    try { state.localBase = await invoke("api_base"); }
    catch { state.localBase = "http://127.0.0.1:38241"; }
    if (!await startupUpdateCheck()) return;
    const savedServer = localStorage.getItem("ghostchat.server");
    if (savedServer === LOCAL_SERVER_SENTINEL) {
      state.base = "";
    } else if (!savedServer || isLocalServer(savedServer) || isLegacyDefaultServer(savedServer)) {
      const migrateLegacyCredentials = isLegacyDefaultServer(savedServer);
      state.base = DEFAULT_REMOTE_SERVER;
      localStorage.setItem("ghostchat.server", state.base);
      if (migrateLegacyCredentials) await migrateServerCredentials(LEGACY_DEFAULT_REMOTE_SERVER, DEFAULT_REMOTE_SERVER);
    } else {
      state.base = normalizedBase(savedServer);
    }
    updateServerLabels();
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
