(() => {
  "use strict";
  // Original four-note instrumental motif: C5 E5 G5 E5, soft sine timbre,
  // 120 BPM, 4.8-second repeat. No remote media, microphone or network needed.
  let context = null;
  let timer = null;
  let wanted = false;
  const playing = new Set();
  function stopNotes() {
    for (const oscillator of playing) { try { oscillator.stop(); } catch {} }
    playing.clear();
  }
  function phrase() {
    if (!wanted || context?.state !== "running") return;
    [523.25, 659.25, 783.99, 659.25].forEach((frequency, index) => {
      const oscillator = context.createOscillator();
      const gain = context.createGain();
      const at = context.currentTime + index * 0.23;
      oscillator.type = "sine";
      oscillator.frequency.value = frequency;
      gain.gain.setValueAtTime(0, at);
      gain.gain.linearRampToValueAtTime(0.12, at + 0.018);
      gain.gain.exponentialRampToValueAtTime(0.001, at + 0.34);
      oscillator.connect(gain); gain.connect(context.destination);
      playing.add(oscillator);
      oscillator.onended = () => { playing.delete(oscillator); oscillator.disconnect(); gain.disconnect(); };
      oscillator.start(at); oscillator.stop(at + 0.36);
    });
  }
  async function unlock() {
    try {
      const Audio = window.AudioContext || window.webkitAudioContext;
      if (!Audio) return;
      context ||= new Audio();
      if (context.state !== "running") await context.resume();
      if (wanted && !playing.size) phrase();
    } catch (error) { console.debug("Call ringtone audio is unavailable.", error); }
  }
  function setIncoming(incoming) {
    const next = !!incoming;
    if (wanted === next) return;
    wanted = next;
    if (timer) clearInterval(timer);
    timer = null;
    stopNotes();
    if (wanted) {
      void unlock();
      timer = setInterval(phrase, 4800);
    }
  }
  document.addEventListener("pointerdown", unlock, { passive: true });
  document.addEventListener("keydown", unlock, { passive: true });
  window.addEventListener("pagehide", () => setIncoming(false));
  window.GhostChatRingtone = { setIncoming };
})();
