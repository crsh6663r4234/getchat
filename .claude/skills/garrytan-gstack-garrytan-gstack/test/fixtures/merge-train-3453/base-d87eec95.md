# Changelog

## [1.91.274.0] - 2026-10-27

**/design-html and /design-review now run the design lint, and its copy tier reports every content and ship-floor tell at its line (hallmark phase 2, GSTA-92; Garry 2026-10-26 "Do it", option a).** The two design skills still called `gstack-design-craft`, which counted tells without positions and kept accepted exceptions in finalized.json. The lint already carried the same ids, so the skills now call it, the copy tier has its own patterns with line numbers, and a page answers a tell with a waiver comment beside it instead of a side record.

### Changed
- **/design-html** Step 4 runs `gstack-design-lint <page> --render` after the Slop Gate and reads its exit code: 2 fixes every `blocks` line, 1 fixes or waives each `ask` line, 0 lists `polish` as notes. A missing `LINT_SUMMARY`, a `render` line or `render=skipped` is said plainly and the static lines stand.
- **Waivers live in the page.** A real customer named Nexus or a supplied 50% figure gets `<!-- gstack-lint-disable <rule-id>: <reason about this page, 24+ chars> -->`, written only for what the user supplied or confirmed. This supersedes #3422's persisted craft exceptions: `--accepted` and finalized.json's `craft_exceptions` are gone, and the hand-off lists every waiver.
- **Copy-tier tiers**: `placeholder-name`, `placeholder-brand`, `lorem`, `round-number` and `needs-proof` move from blocks to ask, so the waiver above can answer them. /design-review still files a waived `needs-proof` or `lorem`, which a live page cannot carry.
- **/design-review's craft audit** runs the lint on each saved page: `blocks` and `ask` lines are category 9 findings at their line, `waived` lines are listed with their reason, `empty-mount-node` sends the audit to the saved DOM. A saved live page has lost its linked CSS, so the audit files only its copy-tier (`craft`) lines there and judges the CSS rules by eye.
- **`lib/design-lint/rules/copy.ts`** holds the copy tier's own patterns (moved from `lib/design-craft.ts`), reporting each tell at its line with what matched. It reads visible text, script bodies and inline handlers (single-quoted included) and the parsed tags, so a hydration payload, commented-out markup or a `<title>` string inside a script no longer counts.

### Added
- **`empty-mount-node`** (ask, page only): a body with no text, a `<noscript>` notice aside, that loads a script is a client-rendered shell; the lint says it read none of its content instead of passing it.

### Removed
- `bin/gstack-design-craft`, `lib/design-craft.ts` and `test/design-craft.test.ts`. Their checks are in the lint's copy tier and its tests; the archetype handoff tests moved to `test/design-craft-resolvers.test.ts`.

### Tests
- `test/design-lint.test.ts`: the copy tier at its lines with the catalog's fixes, the clean page with inlined Pretext, the skip link and `aria-current`, visible-text reading, the four checker limits #3433 documented, and the shell check (noscript, image-only page, component scope). `test/design-lint-recorded.test.ts` reads the recorded page's buzzwords at lines 32 and 37.
- The `design-html-slop-gate` paid case looks for a `gstack-design-lint` call, and its touchfiles select on `lib/design-lint/**` and `bin/gstack-design-lint`.

