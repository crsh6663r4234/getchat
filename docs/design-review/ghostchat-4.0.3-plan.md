# GhostChat 4.0.3 — profile-first redesign

## Direction

Make GhostChat feel like a calm native desktop space for two people: midnight
obsidian surfaces, electric-lilac actions, and mint presence. The product should
put a person's profile, availability, voice state, and conversation context
ahead of dense controls.

## Locked experience changes

1. Three-area desktop layout: a 64px app rail, context list, and focused work
   stage.
2. Profile-first identity: richer avatar/presence treatment in the account bar,
   conversation header, participant cards, and profile settings.
3. Voice becomes a call room with participant cards and an anchored control bar.
   Existing device selection, mute, speaker, screen sharing, and right-click
   per-person mute behaviour stay intact.
4. Settings become navigable sections (Profile, Voice & video, Notifications,
   Privacy, Connection, Updates) instead of one visually undifferentiated form.
5. Motion is intentional and cheap: 160ms hover, 220ms panel transition, and
   320ms call-room entry; animate only opacity and transform. Reduced motion
   remains respected and gets an explicit preference.

## Delivery order

1. Figma: refresh tokens and reusable profile/presence, navigation,
   participant-card, and action-button components.
2. Figma: compose Conversation + Voice active and Profile + Settings screens.
3. App: consolidate the visual cascade and apply the 4.0.3 desktop shell,
   conversation, call-room, and settings treatments without changing API or
   media behaviour.
4. Validate keyboard focus, light mode, narrow layouts, reduced motion, and the
   existing audio/screen-sharing controls.
