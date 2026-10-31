# GhostChat design audit and rebuild

**Status: design audit started after the local GhostChat 4.0 candidate build.** Keep the release candidate isolated from the visual overhaul. The product is for two regular users, so prioritize a clear one-to-one conversation and call flow; groups remain available but should not dominate everyday navigation.

## Start gate

- The GhostChat 4.0 release candidate is complete and its remaining external validation limits are clearly recorded.
- Figma is connected. The editable working file is [GhostChat 4.0](https://www.figma.com/design/2rAq5vrWpVJIKgGqJhiBiP). Source tokens, reusable components, 16 initial conversation/call screens and 46 prototype links now exist. Account/settings/state coverage and source parity remain unfinished; see `design-review/README.md` and the saved node ledgers.
- Start from the current shipped/release-candidate app, not old screenshots or assumptions. Capture a new baseline before changing screens.

## Outcome

Audit the whole GhostChat desktop experience and rebuild its visual system and interaction flows around one coherent product design. Cover onboarding, everyday chats, social flows, media, voice/video, screen sharing, settings, recovery, and updates, including their loading, empty, failure, and permission states. Figma is the source of truth for screens, components, prototypes, and design tokens. Higgsfield supports visual exploration and produces original brand artwork where it improves the interface. The final design must remain practical to implement in the existing Tauri/WebView2 app and retain GhostChat's own identity.

## Work sequence

1. **Establish the baseline.** Capture the current app in Windows WebView2 at compact, typical, and wide window sizes. Record every route, screen, overlay/dialog, user role, key task, and empty/loading/error/permission state in a screen-and-flow inventory.
2. **Audit the experience.** Walk the inventory end to end: account creation and recovery, friends, direct and group chat, messages/media/files, voice and device controls, screen sharing, profiles, settings, updates, tray/close behavior, and safety/privacy controls. Mark confusing navigation, dead ends, excessive steps, overlapping or clipped content, inconsistent hierarchy, accessibility gaps, and English/Turkish layout failures. Rank findings by user impact and implementation dependency.
3. **Set the product and visual direction.** Establish the information architecture and a small set of design principles before styling screens. Use Higgsfield to explore a few original GhostChat visual directions and only create optional artwork that supports the product. Keep chat content legible and controls easy to find; do not copy Discord's branding or use generated artwork behind dense conversation content.
4. **Build the Figma source of truth.** Create one editable Figma file with the audit findings, route map, flows, design tokens, color roles, typography, spacing, icons, buttons, fields, dialogs, navigation, message rows, profile/avatar treatments, participant cards, and responsive behavior. Include hover, pressed, disabled, focus, loading, error, permission, dark, light, English, and Turkish variants.
5. **Prototype the complete journeys.** Redesign and link account creation/recovery, friend requests, direct messages, group creation/management, profile editing, media/file sharing, voice join/device controls, participant management, screen sharing, settings, updates, and account recovery. Make navigation, back behavior, privacy state, and high-impact actions explicit.
6. **Implement in cohesive passes.** Apply the system to the app shell and navigation first, then onboarding/social flows, chat and media, voice/screen sharing, and settings/account/update flows. Keep behavior intact while replacing the layout and styles; remove stale one-off styling as the new components take over.
7. **Compare, refine, and close.** Review Figma and the running Windows WebView2 app side by side at compact, typical, and wide sizes. Fix clipped Turkish text, focus visibility, keyboard behavior, contrast, motion, and scroll behavior. Update the inventory with resolved issues and capture final screenshots for every route and critical state.

## Initial code audit (2026-10-30)

- The source contains onboarding/recovery views, a three-column app shell, messages, people search, friends, group management, a call stage, screen sharing, and one large settings dialog.
- Main navigation is icon-only and uses Unicode symbols; the underlying labels are accessible, but the everyday destinations are not visibly named.
- The stylesheet is a highly compressed base plus many appended overrides, which makes overlapping rules and future adjustments hard to review.
- Settings combines profile, encryption, server, notifications, audio, updates, privacy/recovery, account export, and account deletion in one long dialog.
- Voice and screen-sharing controls are present, but several call-device labels are not connected to the existing English/Turkish translation attributes.
- The installed app did not expose a window after launch in this pass, so there are no honest before screenshots yet. Capture screenshots from a working 4.0 candidate before claiming a visual comparison.

## Two-user product direction

- Make the first useful path obvious: open a direct chat, see connection and encryption state, then call or share a screen from that same conversation.
- Keep both participant identities, avatars, mute state, and speaking state easy to scan during calls.
- Keep friend requests and group creation available but secondary to recent direct conversations.
- Make profile, device selection, recovery, server, and updates findable through clear settings sections with one scroll surface and no overlapping panels.
- Fit Turkish and English labels at compact desktop sizes; keep safety, privacy, and recovery warnings visible when relevant.

## Deliverables

- A complete screen/state inventory with prioritized audit findings and a route map.
- An editable Figma design system, core screens, and linked interaction prototypes.
- Higgsfield exploration results and any original assets actually used in the product.
- Implemented screens and components in the existing GhostChat app.
- Before/after Windows WebView2 captures and a completed issue checklist showing what was fixed or intentionally deferred.

## Acceptance checklist

- Every app route and dialog has an inventoried design and explicit empty/loading/error state.
- A reusable token/component system replaces scattered one-off UI styling.
- Chat, settings, voice, group management, and onboarding use the same visual language.
- English and Turkish fit at all supported window sizes without overlapping or hiding controls.
- Keyboard focus, reduced-motion behavior, color contrast, and control labels are reviewed.
- Final Figma screens and implemented Windows WebView2 screenshots are compared side by side.
- Higgsfield-generated artwork is original, lightweight, optional, and never obscures text or call controls.
- The inventory covers every current route, dialog, common journey, and relevant empty/loading/error/permission state; every high-impact audit finding is fixed or explicitly recorded with a reason.
- The Figma file and final implementation are linked from the design goal's final report so the work can be reviewed and continued.
