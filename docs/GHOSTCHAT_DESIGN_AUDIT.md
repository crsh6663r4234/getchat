# GhostChat design audit — 4.0 starting point

Date: 2026-10-30
Product target: two people using GhostChat regularly, with groups available as a secondary path.

## Evidence and limits

The source audit covers the current HTML, CSS, and UI event handlers. Offline Edge screenshots of the real UI files now exist under `docs/design-review/`. Authenticated server behavior, the Tauri-native window, real audio devices, and a real screen-share stream are not represented by the offline fixture. Figma plugin discovery reports Figma as not installed in this session, so editable Figma screens and interactive prototypes remain outstanding.

## Route and state inventory

| Area | Current route or surface | States to preserve and design |
| --- | --- | --- |
| Startup | Update gate | Checking, up to date, update available, downloading/installing, retryable failure |
| Account | Create / recover tabs | Empty form, validation, server reachability, request failure, recovery success |
| Recovery | New recovery-key screen | Copy/save, acknowledgement required, continue disabled/enabled |
| Messages | Welcome, direct chat, group chat | No conversations, loading, empty history, message list, reconnecting, send failure, encryption setup/verified/key changed |
| Friends | All friends / requests | Empty states, incoming request, accept, ignore, pending badge |
| People | Private username search | Initial, loading, no result, add request, already pending, server error |
| Local search | Recent messages | Query entry, local search progress, result, empty, partial load failure |
| Group actions | Create, join, manage dialogs | Validation, selected friends, invite expiry/revoke, role changes, encrypted-group restrictions, member removal/leave |
| Voice | One-to-one call stage | Outgoing/ringing, incoming accept/decline, connected, reconnecting/failure, muted join when microphone is absent, local/remote mute, speaker output |
| Screen share | Call stage media tiles | Picker, sharing, remote screen, refresh, stop, permission denial, track ended/reconnect |
| Group call | LiveKit participant/media area | Joining, connected speakers, reconnecting, muted states, removed member, unavailable service |
| Settings | Profile, encryption, server, appearance, notifications, desktop behavior, voice, updates, privacy/data | Saved/unsaved values, device list empty/error, update ready/failure, recovery key hidden/revealed, destructive-action confirmation |
| Safety | Block/report dialog | Blocked/unblocked, report validation, sent, server failure |
| Global | Toasts, native dialogs, tray | Success/error, close-to-tray, quit, notification permission/result |

## First-pass findings

1. **Everyday navigation is hard to scan.** The rail showed only symbolic Unicode icons. It now shows named destinations on wide windows, with a compact icon rail at smaller widths.
2. **Compact windows hid conversation navigation.** The sidebar's mobile CSS depended on a `sidebar-open` class that had no UI handler. A menu button and open/close behavior are now wired; selecting a conversation closes the overlay.
3. **Settings were visually dense.** Nine categories were stacked in one narrow dialog. They now share one scroll surface, with related cards arranged in two columns on wide windows and one column on compact windows.
4. **Styles were difficult to change safely.** The base stylesheet was compressed and followed by many appended overrides. New design tokens and a final, separable stylesheet now form the first migration layer; old styles still need to be retired as components move over.
5. **Some call device labels were not localized.** Microphone and speaker labels and system-default choices now have English/Turkish attributes.
6. **Runtime review remains incomplete.** The installed client did not expose a window during this pass; compact, typical, and wide screenshots and actual keyboard/focus checks are still required.

## Two-person design principles

- Put recent private conversations first; a user should reach the other person in one action.
- Keep call, microphone, output, and screen-share controls attached to the active conversation.
- Show both participants' names, avatars, speaking state, and mute state at a glance.
- Keep groups, discovery, recovery, safety, and server administration accessible without making them the primary navigation.
- Keep privacy and encryption status explicit at the point where it affects a conversation.
- Use one predictable scroll surface for long settings and media views; avoid nested scroll traps and overlapping panes.
- Localize every visible label and status in English and Turkish; preserve visible keyboard focus and reduced-motion preferences.

## First implementation pass

- Added `design-system.css` after the existing stylesheets as a reversible migration layer.
- Reworked the app rail, conversation list, chat header, message area, composer, dialogs, settings cards, call stage, and compact sidebar behavior.
- Added visible English/Turkish nav labels and localized call microphone/speaker pickers.
- Added `aria-current` updates for page navigation and an accessible compact-window sidebar toggle.
- `node --check` passes for `main.js` and `group-voice-client.js`; `git diff --check` passes. Full string audit and Tauri runtime screenshots remain pending.

## Offline visual review — 2026-10-30

The preview uses the production `index.html` and stylesheets with local sample content and network access disabled. Main-app preview content is a static two-person fixture; it does not authenticate or connect to the user's server.

- `design-review/auth-wide.png` and `design-review/auth-typical.png` show the English account entry screen at wide and typical widths.
- `design-review/auth-compact.png` and `design-review/auth-compact-tr.png` show compact English and Turkish account entry screens. The 390 px layout fits without horizontal clipping; the initial Turkish capture exposed an untranslated privacy notice, which is now localized.
- `design-review/direct-message.png` and `design-review/direct-message-compact.png` show a two-person conversation at typical and compact widths, including both message identities and the compact conversation toggle.
- `design-review/voice-call.png` shows the two participant cards, speaking state, missing-microphone state, input/output selection, share control, and share-refresh control.
- `design-review/settings-wide.png` and `design-review/settings-compact.png` show the scrollable settings surface at two widths. The visual check found overlapping cards on wide layout; grid rows now size to their content, and columns collapse to one below 820 px.
- Compact navigation now closes the sidebar after navigation and before opening Settings.

The offline samples are visual evidence only. A real Tauri window still needs a layout pass for settings, recovery, dialogs, Turkish conversation text, actual call setup, and screen sharing. Do not use these images as evidence that the server, microphone, TURN, LiveKit, or screen-share transport works.
