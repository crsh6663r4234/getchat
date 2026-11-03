(() => {
  "use strict";
  const tr = () => document.documentElement.lang === "tr";
  let popup = null;
  const close = () => { popup?.owner?.setAttribute("aria-expanded", "false"); popup?.remove(); popup = null; };
  const selectors = new Map();
  function sync(select, button) {
    button.textContent = select.selectedOptions[0]?.textContent || (tr() ? "Seç" : "Choose");
    button.disabled = select.disabled;
    button.setAttribute("aria-label", `${select.closest("label")?.querySelector("span")?.textContent || ""}: ${button.textContent}`);
  }
  function enhanceSelect(select) {
    if (selectors.has(select) || select.multiple) return;
    const button = document.createElement("button");
    button.type = "button"; button.className = "desktop-select";
    button.setAttribute("aria-haspopup", "listbox"); button.setAttribute("aria-expanded", "false");
    select.classList.add("desktop-select-source"); select.tabIndex = -1;
    select.after(button); selectors.set(select, button); sync(select, button);
    select.addEventListener("change", () => sync(select, button));
    const open = () => {
      if (popup?.owner === button) { close(); button.setAttribute("aria-expanded", "false"); return; }
      close(); sync(select, button);
      if (select.disabled) return;
      const menu = document.createElement("div"); menu.className = "desktop-select-menu";
      menu.setAttribute("role", "listbox"); menu.setAttribute("aria-label", button.getAttribute("aria-label"));
      menu.owner = button; popup = menu;
      const options = [...select.options];
      for (const option of options) {
        if (option.hidden) continue;
        const item = document.createElement("button"); item.type = "button";
        item.textContent = option.textContent; item.setAttribute("role", "option");
        item.disabled = option.disabled || !!option.closest("optgroup")?.disabled;
        item.setAttribute("aria-selected", String(option.selected));
        item.onclick = event => {
          event.preventDefault(); event.stopPropagation();
          select.selectedIndex = option.index;
          select.dispatchEvent(new Event("input", { bubbles: true }));
          select.dispatchEvent(new Event("change", { bubbles: true }));
          sync(select, button); close(); button.setAttribute("aria-expanded", "false"); button.focus();
        };
        menu.append(item);
      }
      (select.closest("dialog[open]") || document.body).append(menu);
      const rect = button.getBoundingClientRect();
      menu.style.width = `${Math.min(Math.max(rect.width, 150), innerWidth - 20)}px`;
      menu.style.left = `${Math.max(8, Math.min(rect.left, innerWidth - menu.offsetWidth - 8))}px`;
      menu.style.maxHeight = `${Math.max(90, Math.min(280, innerHeight - 24))}px`;
      menu.style.top = `${Math.max(8, Math.min(rect.bottom + 5, innerHeight - menu.offsetHeight - 8))}px`;
      button.setAttribute("aria-expanded", "true");
      (menu.querySelector('[aria-selected="true"]:not(:disabled)') || menu.querySelector("button:not(:disabled)"))?.focus();
    };
    button.onclick = event => { event.preventDefault(); open(); };
    button.onkeydown = event => { if (["ArrowDown", "ArrowUp"].includes(event.key)) { event.preventDefault(); open(); } };
  }
  document.addEventListener("keydown", event => {
    if (!popup) return;
    if (event.key === "Escape") { const owner = popup.owner; close(); owner.setAttribute("aria-expanded", "false"); owner.focus(); event.preventDefault(); event.stopPropagation(); }
    else if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
      const items = [...popup.querySelectorAll("button:not(:disabled)")];
      const index = items.indexOf(document.activeElement);
      const next = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : (index + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
      items[next]?.focus(); event.preventDefault();
    } else if (event.key === "Tab") close();
  }, true);
  document.addEventListener("pointerdown", event => {
    for (const [select, button] of selectors) {
      if (!select.isConnected) { selectors.delete(select); continue; }
      sync(select, button);
    }
    if (popup && !popup.contains(event.target) && !popup.owner.contains(event.target)) close();
  }, true);
  window.addEventListener("resize", close);
  document.addEventListener("scroll", event => { if (popup && !popup.contains(event.target)) close(); }, true);
  // Preserve app-specific right-click actions while suppressing WebView's menu.
  document.addEventListener("contextmenu", event => {
    if (event.defaultPrevented) return;
    event.preventDefault();
    const input = event.target.closest("input,textarea");
    if (input && !input.disabled) {
      const menu = document.createElement("div"); menu.className = "desktop-select-menu";
      close(); popup = menu; menu.owner = input;
      const action = (label, run) => {
        const button = document.createElement("button"); button.type = "button"; button.textContent = label;
        button.onclick = () => { close(); input.focus(); void run(); }; menu.append(button);
      };
      action(tr() ? "Tümünü seç" : "Select all", () => input.select());
      action(tr() ? "Kopyala" : "Copy", () => navigator.clipboard.writeText(input.value.slice(input.selectionStart || 0, input.selectionEnd || 0)).catch(() => {}));
      if (!input.readOnly) action(tr() ? "Yapıştır" : "Paste", async () => {
        try { const text = await navigator.clipboard.readText(); input.setRangeText(text, input.selectionStart, input.selectionEnd, "end"); input.dispatchEvent(new Event("input", { bubbles: true })); } catch {}
      });
      (input.closest("dialog[open]") || document.body).append(menu);
      menu.style.left = `${Math.max(8, Math.min(event.clientX, innerWidth - menu.offsetWidth - 8))}px`;
      menu.style.top = `${Math.max(8, Math.min(event.clientY, innerHeight - menu.offsetHeight - 8))}px`;
    }
  });
  // In-app inline validation replaces the browser's floating validation bubble.
  document.addEventListener("invalid", event => {
    event.preventDefault();
    const input = event.target;
    input.setAttribute("aria-invalid", "true");
    let error = input.parentElement.querySelector(".desktop-field-error");
    if (!error) { error = document.createElement("small"); error.className = "desktop-field-error"; error.setAttribute("role", "alert"); input.after(error); }
    error.textContent = input.validity.valueMissing ? (tr() ? "Bu alanı doldur." : "Fill in this field.") : (tr() ? "Geçerli bir değer gir." : "Enter a valid value.");
    if (input.matches("select")) selectors.get(input)?.focus(); else input.focus();
  }, true);
  document.addEventListener("input", event => {
    if (event.target.validity?.valid) {
      event.target.removeAttribute("aria-invalid");
      event.target.parentElement.querySelector(".desktop-field-error")?.remove();
    }
  });
  document.addEventListener("keydown", event => {
    if (["F5", "F12"].includes(event.key) || (event.ctrlKey && ["r", "+", "=", "-", "0"].includes(event.key.toLowerCase())) || (event.ctrlKey && event.shiftKey && ["i", "j"].includes(event.key.toLowerCase())) || (event.altKey && ["ArrowLeft", "ArrowRight"].includes(event.key))) event.preventDefault();
  });
  document.addEventListener("wheel", event => { if (event.ctrlKey) event.preventDefault(); }, { passive: false });
  const players = new WeakSet();
  function enhanceVideo(video) {
    if (!video.controls || players.has(video)) return;
    players.add(video); video.controls = false;
    const wrapper = document.createElement("div"); wrapper.className = "desktop-player";
    video.before(wrapper); wrapper.append(video);
    const controls = document.createElement("div"); controls.className = "desktop-player-controls";
    const play = document.createElement("button"); play.type = "button";
    const seek = document.createElement("input"); seek.type = "range"; seek.min = "0"; seek.max = "100"; seek.value = "0"; seek.step = "0.1";
    seek.setAttribute("aria-label", tr() ? "Video konumu" : "Video position");
    const time = document.createElement("span");
    const mute = document.createElement("button"); mute.type = "button";
    const format = value => `${Math.floor((value || 0) / 60)}:${String(Math.floor((value || 0) % 60)).padStart(2, "0")}`;
    const update = () => {
      play.textContent = video.paused ? "▶" : "Ⅱ";
      play.setAttribute("aria-label", tr() ? (video.paused ? "Oynat" : "Duraklat") : (video.paused ? "Play" : "Pause"));
      mute.textContent = video.muted ? (tr() ? "Sesi aç" : "Unmute") : (tr() ? "Sessiz" : "Mute");
      mute.setAttribute("aria-pressed", String(video.muted));
      const duration = Number.isFinite(video.duration) ? video.duration : 0;
      seek.disabled = duration <= 0;
      seek.value = duration ? String(video.currentTime / duration * 100) : "0";
      time.textContent = `${format(video.currentTime)} / ${format(duration)}`;
    };
    play.onclick = () => { if (video.paused) void video.play().catch(() => {}); else video.pause(); };
    video.addEventListener("click", play.onclick);
    seek.oninput = () => { if (Number.isFinite(video.duration)) video.currentTime = video.duration * Number(seek.value) / 100; };
    mute.onclick = () => { video.muted = !video.muted; };
    for (const event of ["loadedmetadata", "timeupdate", "play", "pause", "ended", "volumechange"]) video.addEventListener(event, update);
    controls.append(play, seek, time, mute); wrapper.append(controls); update();
  }
  let tooltip = null;
  function hideTooltip() { tooltip?.remove(); tooltip = null; }
  function showTooltip(event) {
    const node = event.target.closest?.("[data-desktop-tooltip]");
    if (!node) return;
    hideTooltip(); tooltip = document.createElement("div"); tooltip.className = "desktop-tooltip";
    tooltip.textContent = node.dataset.desktopTooltip; tooltip.setAttribute("role", "tooltip");
    (node.closest("dialog[open]") || document.body).append(tooltip);
    const rect = node.getBoundingClientRect();
    tooltip.style.left = `${Math.max(6, Math.min(rect.left, innerWidth - tooltip.offsetWidth - 6))}px`;
    tooltip.style.top = `${Math.max(6, rect.top - tooltip.offsetHeight - 6)}px`;
  }
  document.addEventListener("pointerover", showTooltip);
  document.addEventListener("focusin", showTooltip);
  document.addEventListener("pointerout", hideTooltip);
  document.addEventListener("focusout", hideTooltip);
  document.addEventListener("pointerdown", hideTooltip);
  function scan() {
    document.querySelectorAll("select").forEach(enhanceSelect);
    for (const [select, button] of selectors) sync(select, button);
    document.querySelectorAll("video[controls]").forEach(enhanceVideo);
    document.querySelectorAll("[title]").forEach(node => {
      if (!node.title) return;
      node.dataset.desktopTooltip = node.title;
      if (!node.getAttribute("aria-label")) node.setAttribute("aria-label", node.title);
      node.removeAttribute("title");
    });
  }
  let pending = false;
  new MutationObserver(records => {
    if (pending || !records.some(record => record.attributeName === "title" && record.target.hasAttribute("title") || record.target.tagName === "SELECT" || record.target.tagName === "OPTION" || [...record.addedNodes].some(node => node.nodeType === 1 && (node.matches?.("select,video[controls],[title]") || node.querySelector?.("select,video[controls],[title]"))))) return;
    pending = true; queueMicrotask(() => { pending = false; scan(); });
  }).observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ["disabled", "selected", "title"] });
  scan();
})();
