# GhostChat Design Senate

This branch uses a deliberately small review council before changing the desktop interface.

| Role | Temperament | Mandate |
| --- | --- | --- |
| Chief Engineer | Decisive | Own the design system, protect behaviour, approve the final build. |
| Product Vision | Optimistic | Find the clearest, most welcoming flow for direct messages and groups. |
| Risk & Access | Pessimistic | Catch contrast, keyboard, localization, density and failure-state regressions. |
| Delivery & Performance | Pragmatic | Keep the implementation maintainable and fast on ordinary Windows hardware. |

## Decision rules

1. Behavioural hooks, IDs and state classes are preserved unless a feature change explicitly requires otherwise.
2. The product uses one calm graphite surface system, one warm action accent, and one cool live-state colour.
3. Motion explains state changes only. It uses `transform` and `opacity`, and respects reduced-motion preferences.
4. Desktop density is earned: primary controls remain visible; secondary tools appear through existing menus, disclosure and hover states.
5. Every redesign pass is checked in dark, light, wallpaper, Turkish, keyboard and compact-width states.

## Current redesign contract

The implementation lives in `crates/desktop/ui/senate.css`, loaded after the legacy application stylesheet. It is visual-only: the HTML IDs, event hooks, active/hidden state classes and existing UI text remain intact.
