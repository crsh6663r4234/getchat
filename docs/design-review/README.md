# GhostChat visual review evidence

## Editable Figma source

[GhostChat 4.0 — Two-person desktop](https://www.figma.com/design/2rAq5vrWpVJIKgGqJhiBiP)

The first design pass contains source-derived dark/light color collections (one mode per collection for the Starter plan), spacing/radius variables, Inter text styles, buttons with default/hover/focus/disabled variants, avatar/status/field components, and 16 composed conversation/call screens. The English and Turkish core flows have 46 navigation links. `figma-state.json` and the two ID ledgers preserve exact nodes, properties and links for targeted updates without recreating content.

The three visually reviewed compositions (`dm-en`, `call-tr`, `muted-tr-compact`) contain editable text, frames and component instances, with no image-filled nodes. Readback confirms Inter. The Figma file is an initial core-flow prototype: it is **not** a final claim of screen parity, full state coverage or production readiness. The account/state page is deliberately reserved and currently empty. The source declares Inter but presently renders a system fallback on this machine; bundle the font before claiming a font match.

## Local interactive review

From the repository root, run `node scripts/design-review-server.mjs`, then open `http://127.0.0.1:4177/?view=call-dock&lang=tr`.

The server binds only loopback. It serves production UI files with a preview-only adapter injected before `main.js`; that adapter is outside the Tauri UI bundle. Account data, requests, sockets and voice states are synthetic. Microphone/screen capture is disabled. Unknown/external requests fail. The preview does not touch the real account or VDS.

Available fixture routes include `auth`, `recovery`, `update-error`, `empty`, `welcome`, `dm`, `history-error`, `friends`, `people`, `message-search`, `call`, `incoming`, `call-dock`, `settings`, `new-conversation`, `create-group`, `join-group`, and `safety`. Use `lang=en` or `lang=tr`; route availability does not mean every fixture has been visually validated yet.

| Capture | CSS viewport | Evidence |
| --- | --- | --- |
| call-compact-en.jpg | 860×560 | Call controls and composer visible together; independent participant/device scroll |
| call-dock-compact-tr.jpg | 860×560 | Turkish return/microphone/hangup dock on Friends |
| dm-typical-en.jpg | 1280×820 | English direct-message layout |
| settings-typical-tr.jpg | 1280×820 | Turkish single-column settings and close action |
| call-wide-en.jpg | 1600×1000 | English wide call and message composition |
| dm-wide-tr.jpg | 1600×1000 | Turkish wide direct-message layout |

Observed interaction checks: microphone toggle updates dock label; return-to-call reveals the stage and hides the dock; hangup hides the dock; Friends header stays correct; closed dialogs have no visible controls or extra document height; settings opens/closes. These exercise real UI handlers against fixture responses, **not** device, server or media transports.

The final changes in this pass passed JavaScript syntax checks, `git diff --check`, and `cargo build --release --locked -p ghostchat-desktop` (optimized build completed successfully). Compilation does not replace a native runtime/media test.

The older `.png` captures are static offline fixtures from the earlier review. None of these artifacts prove WebView2 permissions, TURN fallback, two-party screen sharing, notification delivery, updater installation or tray behavior.

## Remaining design acceptance work

- Complete Figma account, settings, social/group, attachment, safety and empty/loading/error/permission states and their prototype links.
- Match the core prototype to the final implementation, including self-message alignment, speaker/output controls, share refresh, avatars and encryption status.
- Review all dialogs, focus movement, keyboard-only journeys, contrast, light theme and reduced motion.
- Launch the native Windows build and capture before/after evidence without exposing real account data.
- Exercise two real clients for media permissions, call acceptance and sharing transport. Treat these separately from visual acceptance.

No release tag or production deployment is implied by this design work.
