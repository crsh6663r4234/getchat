# GhostChat Industrial UI system

GhostChat uses a single final visual layer, `crates/desktop/ui/industrial-ui.css`. It intentionally overrides legacy feature-specific sheets rather than duplicating view logic in JavaScript.

## Foundations

| Token family | Purpose |
| --- | --- |
| `--ui-canvas`, `--ui-surface`, `--ui-raised` | Neutral charcoal canvas and frosted surfaces |
| `--ui-text`, `--ui-muted` | Readable primary and supporting copy |
| `--ui-accent`, `--ui-accent-soft` | A restrained user/theme accent, never a full-screen pattern |
| `--ui-line` | Low-contrast structural edges |
| `--ui-radius-*` | 10 / 16 / 22px control, panel, and modal radii |
| `--ui-ease`, `--ui-spring` | Fast interaction and entrance curves |

Typography is Inter first, then SF Pro Text and native system fonts. Headings use tight tracking; metadata and labels use muted color and avoid all-caps except compact navigation labels.

## Motion rules

Only `transform` and `opacity` are animated for entrance and ambience. Hover/press treatments are bounded to individual controls. The rail selection has one physical indicator moved with `translate3d`, so navigation does not jump between independent markers.

- Modal entrance: 300ms spring scale/fade.
- Content transitions: existing lightweight entry transition.
- Voice activity: the existing speaking state drives one animated ring around the participant avatar.
- Sakura and Aurora: two low-opacity, blurred gradient auras—not a dot-grid or repeated overlay.

`prefers-reduced-motion` and the in-app reduced-motion preference disable all nonessential motion. No animation is required to understand state.

## Layering and wallpaper safety

The app shell, header, rail, sidebar and dialogs are glass layers with `blur(24px)`. Their backgrounds remain opaque enough to keep controls readable over a custom wallpaper. Aura layers are fixed, non-interactive, behind the document, and cannot cover buttons or inputs.

## Component rules

- Messages remain content-first: reactions/actions are contextual, not permanently displayed.
- Inputs use the same compact 38px minimum height and a clear focus halo.
- Direct and group voice use the same cards, device row and control treatment.
- Buttons have a consistent primary gradient, secondary glass surface, subtle hover lift, and press scale.
