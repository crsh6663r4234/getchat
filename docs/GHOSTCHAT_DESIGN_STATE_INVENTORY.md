# GhostChat 4.0 — route and state inventory

Date: 2026-10-31
Audience: two regular users; one-to-one chat, voice, and screen sharing are the primary journey.

This is a source-level inventory of the current Tauri UI. “Mapped in source” means the HTML and UI handlers contain the state or transition; it does not mean the state has been exercised against the live VDS. Offline screenshots use the shipped UI files and static fixture data. They cannot prove network, device permission, notification, tray, TURN, or screen-share transport behavior.

## Routes and shared surfaces

| Surface | Main states to review | Current evidence | Still needs |
| --- | --- | --- | --- |
| Startup update gate (startupUpdateGate) | Checking, update available/downloading, install/restart, retry after failure | UI and update handlers in index.html and main.js; no screenshot | Tauri update-feed pass/fail on Windows |
| Account (authView) | Create and recover tabs, required-field validation, server status, request failure, profile-photo selection/error | auth-compact.png, auth-compact-tr.png, auth-typical.png, auth-wide.png | Real server create/recover and keyboard/focus pass |
| Recovery (recoveryView) | Copy/save key, confirmation required, continue disabled/enabled, credential-storage warning | Source state in main.js; no dedicated capture | Tauri clipboard, file save, and Credential Manager |
| Messages (messages) | Welcome/empty list, selected DM/group, loading history, no messages, reconnecting, send/upload error, encryption status/change | direct-message.png, direct-message-compact.png | Group and failure/long-content captures; live server |
| Friends (friends) | No friends, no requests, incoming request, accept/ignore, DM action, request count badge | State branches in main.js; no screenshot | Two-account request lifecycle and narrow Turkish capture |
| People (search) | Initial private-search explanation, query, loading, no match, pending/sent, request error | State branches in main.js; no screenshot | Live API search and Turkish error states |
| Message search (searchMessages) | Empty query, local progress, results, no result, per-conversation partial failure | State branches in main.js; no screenshot | Long-result and Turkish capture |
| Settings | Profile, encryption, connection/server, appearance, notifications, desktop behavior, voice/devices, updates, privacy/recovery/export/delete | settings-wide.png, settings-compact.png | Tauri controls, destructive confirmation, Turkish and device-error captures |
| System tray | Window close to tray, restore, quit, startup toggle | Rust/Tauri and UI source only; no screenshot | Native Windows runtime exercise |

## Dialog inventory

| Dialog | Main states and risk points | Current evidence |
| --- | --- | --- |
| newConversationDialog | Choose private search, create group, or join with code; close/back returns to current route | Source only |
| groupDialog | Name required, select 2–15 accepted friends, no friends, submit/error | Source only |
| groupManageDialog | Rename, members/roles, invite friends/codes, code expiry/revoke, encrypted membership constraints, load/save errors | Source only |
| joinGroupDialog | Required invite code, trust reminder, joining, invalid/expired code, success | Source only |
| userSafetyDialog | Block/unblock, reason required, optional details, report success/failure, explicit server visibility warning | Source only |
| settingsDialog | Long single scroll surface; device empty/error, save, recovery reveal/replace, session sign-out, export/delete | Settings captures show layout only; actions not exercised |

## Voice and screen sharing

| Flow | States to preserve | Current evidence |
| --- | --- | --- |
| One-to-one call | Idle, outgoing, incoming accept/decline, connecting, connected, reconnecting, hangup, call failure | voice-call.png illustrates the connected layout only |
| Microphone | Available/unavailable, permission denied, join muted, mute/unmute, push-to-talk, input device change, input test | Connected layout shows muted/no-mic affordance; actual browser permission/device flows not exercised |
| Output | Speaker device change, local speaker mute, remote volume, mute the peer for yourself | UI source only |
| Screen share | System picker, permission denial, local sharing, incoming remote screen, refresh, stop, track ended/reconnect, optional system audio | voice-call.png shows the control layout only; no real stream tested |
| Group voice | Not configured, join/connected/reconnect, participants and speaker state, local/peer mute | Source only; requires LiveKit configuration and multi-client runtime |

## Accessibility and localization pass — 2026-10-31

- Navigation destinations, profile, conversation action, sign-out, search, composer actions, reply cancel, device refresh, dialog close, participant guidance, recovery key, microphone meter, and participant-region labels now carry English/Turkish accessible names.
- Friend navigation announces the pending-request count in the selected language.
- Call participant mute guidance is localized.
- Language switching updates title-based and label-based accessible names through applyLanguage().
- Offline Edge screenshots reviewed in the prior pass cover compact English/Turkish account entry, typical/wide English account entry, compact/typical English DM, typical English call, and compact/wide English settings.

## Prioritized unresolved verification

1. **P0 — Windows runtime:** open the built Tauri app and capture baseline/final screens. Confirm compact sidebar, modal focus, keyboard navigation, close-to-tray, Windows notification permission, update gate, and Turkish text in actual WebView2.
2. **P0 — two-device media:** exercise call accept/decline, no-microphone muted join, device switching, TURN fallback, screen-share picker/refresh/stop, and ended tracks with two real clients. Static screenshots cannot validate this.
3. **P1 — remaining visuals:** capture the friends, people search, local message search, recovery screen, and all six dialogs at compact/typical/wide as applicable, in both languages; compare text wrapping and scroll behavior.
4. **P1 — Figma:** create the editable source, token library, and linked interaction prototype. Figma execution tools are unavailable in this session, so no Figma file is claimed.
5. **P2 — broad accessibility review:** verify visible focus, keyboard-only dialog flows, contrast, and reduced motion on the running app.

## Visual evidence

- auth-compact.png, auth-compact-tr.png, auth-typical.png, auth-wide.png
- direct-message-compact.png, direct-message.png
- voice-call.png
- settings-compact.png, settings-wide.png

Every image above is a local/offline visual review artifact, not proof that the server, audio, permissions, notifications, encryption, updates, or screen-sharing transport works.
