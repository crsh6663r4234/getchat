# GhostChat design audit — 4.0 starting point

Date: 2026-10-30
Product target: two people using GhostChat regularly, with groups available as a secondary path.

## Evidence and limits

This first pass is based on the current HTML, CSS, and UI event handlers. The installed GhostChat app was started for a visual baseline, but Windows returned no app window, so this audit does not claim a runtime screenshot review. Capture working 4.0 screenshots before recording visual before/after results. The current session's plugin discovery reports Figma as not installed; the local design work continues, while editable Figma screens and prototypes remain outstanding.

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
- `node --check` passes for `main.js` and `group-voice-client.js`; `git diff --check` passes. Visual comparison, full string audit, and screenshots remain pending.
