# Design tokens and themes

GhostChat has two themes. The default is **Signage** (Tabela), a dark ground with
route lines and short motion. **Plain** (Sade) is neutral, without route lines and
without motion. Both are chosen in Settings → Theme and stored as `ghostchat.theme`
(older values fall back to Signage).

## Where the tokens live

- `crates/desktop/ui/wayfinding.css`: the Signage tokens on `:root`, then the Signage
  components and motion.
- `crates/desktop/ui/theme-plain.css`: the Plain tokens on `body[data-theme="plain"]`,
  then only what Plain changes.
- `scripts/test-theme-tokens.cjs` checks that both themes define the tokens the
  interface reads, and that Plain has no route colours or purple gradients.

## Tokens (both themes define these)

| Token | Role |
| --- | --- |
| `--bg` | Ground of the app and of every page area |
| `--surface` | Panels, message bodies, the active route |
| `--line-subtle` | Dividers and quiet borders |
| `--text` | Body text and headings |
| `--muted` | Timestamps, secondary lines, hints |
| `--here` | "You are here" marker and the primary action (Signage) |
| `--accent`, `--accent-ink` | Accent and its text colour (aliases of `--here` in Signage) |
| `--font-ui` | The interface face (Segoe UI) |
| `--radius-control`, `--radius-panel` | Corner radii |

Signage only: `--route-1` … `--route-5` (one line colour per conversation, assigned
by position), `--font-mono` (Consolas, for counts and labels).

## Rules

- The sign yellow (`--here`) is for "you are here" and the primary action only.
- Route colours are only for telling conversations apart. Their meaning is carried
  by the name and the line of text too, never by colour alone.
- Avatars without a photo use a neutral surface, never a generated gradient.
- Motion is switched off by the reduced-motion setting, the system preference, and
  by the Plain theme.

## Adding a theme

1. Add an `<option>` and a `data-theme-choice` button in Settings (`index.html`) and
   add the id to `THEMES` in `main.js`.
2. Create `theme-<name>.css` with `body[data-theme="<name>"]` tokens; include every
   token in the table above.
3. Add the file to `index.html` after the other theme files.
4. Run `node scripts/test-theme-tokens.cjs` and check the screens with
   `node scripts/visual-check.cjs`.
